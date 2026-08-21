use std::str::FromStr;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use denju_core::{OperationId, ResourceLocator};
use denju_wire::{
    ApiError, ApiErrorCode, IdentityMutationDomain, RequestHash, identity_mutation_request_hash,
};
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use crate::{Registry, internal_api_error};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthActor {
    Installation {
        installation_id: Uuid,
    },
    Session {
        session_id: Uuid,
        user_id: Uuid,
        installation_id: Uuid,
    },
    Automation {
        user_id: Uuid,
        scopes: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct UserAuthority {
    pub user_id: Uuid,
    pub namespace_id: Uuid,
    pub namespace_slug: String,
    pub author_principal_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdentityOperationActor {
    Installation(Uuid),
    User(Uuid),
}

impl IdentityOperationActor {
    pub(crate) const fn kind(self) -> &'static str {
        match self {
            Self::Installation(_) => "installation",
            Self::User(_) => "user",
        }
    }

    pub(crate) const fn id(self) -> Uuid {
        match self {
            Self::Installation(id) | Self::User(id) => id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriptionSubject {
    Installation(Uuid),
    User(Uuid),
}

impl Registry {
    pub(crate) async fn user_authority(
        &self,
        bearer: &str,
        automation_scope: &str,
    ) -> Result<UserAuthority, ApiError> {
        let user_id = match self.authenticate_actor(bearer).await? {
            AuthActor::Session { user_id, .. } => user_id,
            AuthActor::Automation { user_id, scopes }
                if scopes.iter().any(|scope| {
                    scope == automation_scope || scope == "skills:*" || scope == "*"
                }) =>
            {
                user_id
            }
            AuthActor::Automation { .. } => {
                return Err(ApiError::new(
                    ApiErrorCode::Unauthorized,
                    format!("automation credential requires scope {automation_scope}"),
                ));
            }
            AuthActor::Installation { .. } => {
                return Err(ApiError::new(
                    ApiErrorCode::Unauthorized,
                    "a claimed user identity is required",
                ));
            }
        };
        let row = sqlx::query(
            "SELECT u.namespace_id,n.slug,u.author_principal_id FROM users u \
             JOIN namespaces n ON n.id=u.namespace_id \
             WHERE u.id=$1 AND u.deleted_at IS NULL",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(invalid_credentials)?;
        Ok(UserAuthority {
            user_id,
            namespace_id: row.get(0),
            namespace_slug: row.get(1),
            author_principal_id: row.get(2),
        })
    }
}

pub(crate) async fn record_identity_operation<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: IdentityOperationActor,
    operation_id: OperationId,
    request_hash: RequestHash,
    operation_kind: &str,
    secret_verifier: Option<&str>,
    outcome: &T,
) -> Result<(), ApiError> {
    let outcome = serde_json::to_value(outcome)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    sqlx::query(
        "INSERT INTO identity_operations \
         (actor_kind,actor_id,operation_id,request_hash,secret_verifier,operation_kind,outcome_json) \
         VALUES ($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(actor.kind())
    .bind(actor.id())
    .bind(operation_id.as_uuid())
    .bind(request_hash.as_bytes().as_slice())
    .bind(secret_verifier)
    .bind(operation_kind)
    .bind(outcome)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

pub(crate) fn require_session(actor: AuthActor) -> Result<(Uuid, Uuid), ApiError> {
    match actor {
        AuthActor::Session {
            session_id,
            user_id,
            ..
        } => Ok((session_id, user_id)),
        _ => Err(ApiError::new(
            ApiErrorCode::Unauthorized,
            "a user session is required",
        )),
    }
}

pub(crate) async fn link_installation_to_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    installation_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let existing = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT user_id FROM installations WHERE id=$1 AND revoked_at IS NULL FOR UPDATE",
    )
    .bind(installation_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::Unauthorized, "installation is unavailable"))?;
    match existing {
        Some(existing) if existing != user_id => {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                "this installation is already attributed to another identity",
            ));
        }
        Some(_) => {}
        None => {
            sqlx::query("UPDATE installations SET user_id=$1 WHERE id=$2")
                .bind(user_id)
                .bind(installation_id)
                .execute(&mut **tx)
                .await
                .map_err(internal_api_error)?;
        }
    }
    Ok(())
}

pub(crate) async fn adopt_installation_subscriptions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    installation_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO account_subscriptions (user_id,resource_id,pinned_release_version,retain_on_delete) \
         SELECT $1,resource_id,pinned_release_version,retain_on_delete FROM installation_subscriptions WHERE installation_id=$2 \
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(installation_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    sqlx::query("DELETE FROM installation_subscriptions WHERE installation_id=$1")
        .bind(installation_id)
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    Ok(())
}

pub(crate) async fn link_installation_author_to_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    installation_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let author =
        sqlx::query_scalar::<_, Uuid>("SELECT author_principal_id FROM installations WHERE id=$1")
            .bind(installation_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(internal_api_error)?;
    sqlx::query(
        "INSERT INTO author_principal_users (author_principal_id,user_id) VALUES ($1,$2) \
         ON CONFLICT(author_principal_id) DO UPDATE SET user_id=excluded.user_id",
    )
    .bind(author)
    .bind(user_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

pub(crate) async fn insert_session(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session_id: Uuid,
    user_id: Uuid,
    installation_id: Uuid,
    token_hash: &[u8; 32],
    device_name: &str,
) -> Result<(), ApiError> {
    let device_name = if device_name.trim().is_empty() {
        "device"
    } else {
        device_name.trim()
    };
    sqlx::query(
        "INSERT INTO sessions (id,user_id,installation_id,token_hash,device_name) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(session_id)
    .bind(user_id)
    .bind(installation_id)
    .bind(token_hash.as_slice())
    .bind(device_name)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

pub(crate) fn username_slug(value: &str) -> Result<String, ApiError> {
    let locator = ResourceLocator::from_str(&format!("{value}/identity"))
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    Ok(locator.owner().to_owned())
}

pub(crate) fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.is_empty() {
        Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "password cannot be empty",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt_bytes: [u8; 16] = rand::random();
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| {
            ApiError::new(
                ApiErrorCode::Internal,
                format!("password hashing failed: {error}"),
            )
        })
}

pub(crate) fn operation_secret_bundle(parts: &[&str]) -> Vec<u8> {
    let mut bundled = b"denju:identity-operation-secret:v1\0".to_vec();
    for part in parts {
        let bytes = part.as_bytes();
        let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        bundled.extend_from_slice(&len.to_be_bytes());
        bundled.extend_from_slice(bytes);
    }
    bundled
}

pub(crate) fn hash_operation_secret(secret: &[u8]) -> Result<String, ApiError> {
    let salt_bytes: [u8; 16] = rand::random();
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    Argon2::default()
        .hash_password(secret, &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| {
            ApiError::new(
                ApiErrorCode::Internal,
                format!("operation secret hashing failed: {error}"),
            )
        })
}

pub(crate) fn operation_secret_matches(stored: Option<&str>, supplied: Option<&[u8]>) -> bool {
    match (stored, supplied) {
        (None, None) => true,
        (Some(encoded), Some(secret)) => PasswordHash::new(encoded)
            .ok()
            .is_some_and(|parsed| Argon2::default().verify_password(secret, &parsed).is_ok()),
        _ => false,
    }
}

pub(crate) fn verify_password(password: &str, encoded: &str) -> Result<(), ApiError> {
    let parsed = PasswordHash::new(encoded).map_err(|_| invalid_credentials())?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| invalid_credentials())
}

pub(crate) fn decode_secret_hash(value: &str, field: &str) -> Result<[u8; 32], ApiError> {
    let bytes = hex::decode(value).map_err(|_| {
        ApiError::new(
            ApiErrorCode::InvalidRequest,
            format!("{field} must be hexadecimal"),
        )
    })?;
    bytes.try_into().map_err(|_| {
        ApiError::new(
            ApiErrorCode::InvalidRequest,
            format!("{field} must encode 32 bytes"),
        )
    })
}

pub(crate) fn decode_secret_value(value: &str, field: &str) -> Result<[u8; 32], ApiError> {
    decode_secret_hash(value, field)
}

pub(crate) fn validate_operation_id(value: &str) -> Result<OperationId, ApiError> {
    OperationId::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
}

pub(crate) fn validate_request_hash<T: Serialize>(
    operation_id: &str,
    domain: IdentityMutationDomain,
    safe_payload: &T,
    supplied: &str,
) -> Result<RequestHash, ApiError> {
    let supplied = RequestHash::from_str(supplied)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
    let expected = identity_mutation_request_hash(operation_id, domain, safe_payload)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
    if supplied == expected {
        Ok(supplied)
    } else {
        Err(ApiError::new(
            ApiErrorCode::InvalidRequestHash,
            "request_hash does not match the canonical non-secret request payload",
        ))
    }
}

pub(crate) fn invalid_credentials() -> ApiError {
    ApiError::new(ApiErrorCode::Unauthorized, "invalid identity credential")
}
