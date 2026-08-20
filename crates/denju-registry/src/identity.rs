use std::str::FromStr;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use denju_core::{OperationId, ResourceLocator};
use denju_wire::{
    AccountDeleteRequest, AccountDeleteResponse, ApiError, ApiErrorCode,
    AutomationTokenCreateRequest, AutomationTokenCreateResponse, AutomationTokenInfo,
    AutomationTokenList, AutomationTokenRevokeRequest, AutomationTokenRevokeResponse,
    ClaimIdentityRequest, DeviceInfo, DeviceList, DeviceRevokeRequest, DeviceRevokeResponse,
    IdentityBackupRequest, IdentityInfo, IdentityMutationDomain, IdentitySessionResponse,
    LoginRequest, RecoveryResetRequest, RequestHash, identity_mutation_request_hash,
};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::{Registry, internal_api_error};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AuthActor {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityOperationActor {
    Installation(Uuid),
    User(Uuid),
}

impl IdentityOperationActor {
    const fn kind(self) -> &'static str {
        match self {
            Self::Installation(_) => "installation",
            Self::User(_) => "user",
        }
    }

    const fn id(self) -> Uuid {
        match self {
            Self::Installation(id) | Self::User(id) => id,
        }
    }
}

impl Registry {
    pub async fn claim_identity(
        &self,
        installation_bearer: &str,
        request: &ClaimIdentityRequest,
    ) -> Result<IdentitySessionResponse, ApiError> {
        let operation_id = validate_operation_id(&request.operation_id)?;
        let username = username_slug(&request.username)?;
        let session_hash = decode_secret_hash(&request.session_token_hash, "session_token_hash")?;
        let recovery_hash =
            decode_secret_hash(&request.recovery_secret_hash, "recovery_secret_hash")?;
        let request_hash = validate_request_hash(
            &request.operation_id,
            IdentityMutationDomain::Claim,
            &(
                &request.username,
                &request.session_token_hash,
                &request.recovery_secret_hash,
                &request.device_name,
            ),
            &request.request_hash,
        )?;
        if let Some(installation_id) = self
            .installation_id_from_bearer_any(installation_bearer)
            .await?
            && let Some(outcome) = self
                .replay_identity_operation(
                    IdentityOperationActor::Installation(installation_id),
                    operation_id,
                    request_hash,
                    "claim",
                    Some(request.password.as_bytes()),
                )
                .await?
        {
            return Ok(outcome);
        }
        let installation_id = self.authenticate_installation(installation_bearer).await?;
        validate_password(&request.password)?;
        let password_hash = hash_password(&request.password)?;

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let existing_user = sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT user_id FROM installations WHERE id=$1 FOR UPDATE",
        )
        .bind(installation_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if existing_user.is_some() {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                "this installation is already linked to an identity",
            ));
        }
        if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM namespaces WHERE slug=$1")
            .bind(&username)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?
            != 0
        {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                format!("@{username} is already claimed or reserved"),
            ));
        }

        let namespace_id = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let user_author_id = Uuid::now_v7();
        let session_id = Uuid::now_v7();
        sqlx::query("INSERT INTO namespaces (id, slug, kind) VALUES ($1,$2,'user')")
            .bind(namespace_id)
            .bind(&username)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("INSERT INTO author_principals (id, kind) VALUES ($1,'user')")
            .bind(user_author_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO users (id,namespace_id,author_principal_id,password_hash,recovery_secret_hash) \
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(user_id)
        .bind(namespace_id)
        .bind(user_author_id)
        .bind(&password_hash)
        .bind(recovery_hash.as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        link_installation_to_user(&mut tx, installation_id, user_id).await?;
        sqlx::query(
            "INSERT INTO author_principal_users (author_principal_id,user_id) VALUES ($1,$2), \
             ((SELECT author_principal_id FROM installations WHERE id=$3),$2)",
        )
        .bind(user_author_id)
        .bind(user_id)
        .bind(installation_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        adopt_installation_subscriptions(&mut tx, installation_id, user_id).await?;
        insert_session(
            &mut tx,
            session_id,
            user_id,
            installation_id,
            &session_hash,
            &request.device_name,
        )
        .await?;
        let outcome = IdentitySessionResponse {
            user_id: user_id.to_string(),
            namespace_id: namespace_id.to_string(),
            username: format!("@{username}"),
            session_id: session_id.to_string(),
        };
        record_identity_operation(
            &mut tx,
            IdentityOperationActor::Installation(installation_id),
            operation_id,
            request_hash,
            "claim",
            Some(&password_hash),
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn login(
        &self,
        installation_bearer: &str,
        request: &LoginRequest,
    ) -> Result<IdentitySessionResponse, ApiError> {
        let operation_id = validate_operation_id(&request.operation_id)?;
        let username = username_slug(&request.username)?;
        let session_hash = decode_secret_hash(&request.session_token_hash, "session_token_hash")?;
        let request_hash = validate_request_hash(
            &request.operation_id,
            IdentityMutationDomain::Login,
            &(
                &request.username,
                &request.session_token_hash,
                &request.device_name,
            ),
            &request.request_hash,
        )?;
        if let Some(installation_id) = self
            .installation_id_from_bearer_any(installation_bearer)
            .await?
            && let Some(outcome) = self
                .replay_identity_operation(
                    IdentityOperationActor::Installation(installation_id),
                    operation_id,
                    request_hash,
                    "login",
                    Some(request.password.as_bytes()),
                )
                .await?
        {
            return Ok(outcome);
        }
        let installation_id = self.authenticate_installation(installation_bearer).await?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let row = sqlx::query(
            "SELECT u.id,u.namespace_id,u.password_hash FROM users u \
             JOIN namespaces n ON n.id=u.namespace_id WHERE n.slug=$1 AND u.deleted_at IS NULL FOR UPDATE",
        )
        .bind(&username)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(invalid_credentials)?;
        let user_id: Uuid = row.get(0);
        let namespace_id: Uuid = row.get(1);
        let password_hash: String = row.get(2);
        verify_password(&request.password, &password_hash)?;
        link_installation_to_user(&mut tx, installation_id, user_id).await?;
        let installation_author = sqlx::query_scalar::<_, Uuid>(
            "SELECT author_principal_id FROM installations WHERE id=$1",
        )
        .bind(installation_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO author_principal_users (author_principal_id,user_id) VALUES ($1,$2) \
             ON CONFLICT(author_principal_id) DO UPDATE SET user_id=excluded.user_id",
        )
        .bind(installation_author)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        adopt_installation_subscriptions(&mut tx, installation_id, user_id).await?;
        let session_id = Uuid::now_v7();
        insert_session(
            &mut tx,
            session_id,
            user_id,
            installation_id,
            &session_hash,
            &request.device_name,
        )
        .await?;
        let outcome = IdentitySessionResponse {
            user_id: user_id.to_string(),
            namespace_id: namespace_id.to_string(),
            username: format!("@{username}"),
            session_id: session_id.to_string(),
        };
        record_identity_operation(
            &mut tx,
            IdentityOperationActor::Installation(installation_id),
            operation_id,
            request_hash,
            "login",
            Some(&password_hash),
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn recovery_reset(
        &self,
        installation_bearer: &str,
        request: &RecoveryResetRequest,
    ) -> Result<IdentitySessionResponse, ApiError> {
        let operation_id = validate_operation_id(&request.operation_id)?;
        let username = username_slug(&request.username)?;
        let replacement_recovery_hash = decode_secret_hash(
            &request.replacement_recovery_secret_hash,
            "replacement_recovery_secret_hash",
        )?;
        let session_hash = decode_secret_hash(&request.session_token_hash, "session_token_hash")?;
        let request_hash = validate_request_hash(
            &request.operation_id,
            IdentityMutationDomain::RecoveryReset,
            &(
                &request.username,
                &request.session_token_hash,
                &request.replacement_recovery_secret_hash,
                &request.device_name,
            ),
            &request.request_hash,
        )?;
        let operation_secret = operation_secret_bundle(&[
            request.recovery_secret.as_str(),
            request.new_password.as_str(),
        ]);
        if let Some(installation_id) = self
            .installation_id_from_bearer_any(installation_bearer)
            .await?
            && let Some(outcome) = self
                .replay_identity_operation(
                    IdentityOperationActor::Installation(installation_id),
                    operation_id,
                    request_hash,
                    "recovery_reset",
                    Some(&operation_secret),
                )
                .await?
        {
            return Ok(outcome);
        }
        let installation_id = self.authenticate_installation(installation_bearer).await?;
        validate_password(&request.new_password)?;
        let supplied_recovery = decode_secret_value(&request.recovery_secret, "recovery_secret")?;
        let supplied_recovery_hash: [u8; 32] = Sha256::digest(supplied_recovery).into();
        let new_password_hash = hash_password(&request.new_password)?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let row = sqlx::query(
            "SELECT u.id,u.namespace_id,u.recovery_secret_hash FROM users u \
             JOIN namespaces n ON n.id=u.namespace_id WHERE n.slug=$1 AND u.deleted_at IS NULL FOR UPDATE",
        )
        .bind(&username)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(invalid_credentials)?;
        let user_id: Uuid = row.get(0);
        let namespace_id: Uuid = row.get(1);
        let stored_recovery: Vec<u8> = row.get(2);
        if stored_recovery.as_slice() != supplied_recovery_hash {
            return Err(invalid_credentials());
        }
        let operation_secret_verifier = hash_operation_secret(&operation_secret)?;
        sqlx::query("UPDATE users SET password_hash=$1,recovery_secret_hash=$2 WHERE id=$3")
            .bind(new_password_hash)
            .bind(replacement_recovery_hash.as_slice())
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        link_installation_to_user(&mut tx, installation_id, user_id).await?;
        link_installation_author_to_user(&mut tx, installation_id, user_id).await?;
        adopt_installation_subscriptions(&mut tx, installation_id, user_id).await?;
        let session_id = Uuid::now_v7();
        insert_session(
            &mut tx,
            session_id,
            user_id,
            installation_id,
            &session_hash,
            &request.device_name,
        )
        .await?;
        let outcome = IdentitySessionResponse {
            user_id: user_id.to_string(),
            namespace_id: namespace_id.to_string(),
            username: format!("@{username}"),
            session_id: session_id.to_string(),
        };
        record_identity_operation(
            &mut tx,
            IdentityOperationActor::Installation(installation_id),
            operation_id,
            request_hash,
            "recovery_reset",
            Some(&operation_secret_verifier),
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn identity_backup(
        &self,
        bearer: &str,
        request: &IdentityBackupRequest,
    ) -> Result<(), ApiError> {
        let operation_id = validate_operation_id(&request.operation_id)?;
        let replacement = decode_secret_hash(
            &request.replacement_recovery_secret_hash,
            "replacement_recovery_secret_hash",
        )?;
        let request_hash = validate_request_hash(
            &request.operation_id,
            IdentityMutationDomain::Backup,
            &request.replacement_recovery_secret_hash,
            &request.request_hash,
        )?;
        if let Some(user_id) = self.session_user_from_bearer_any(bearer).await?
            && let Some(outcome) = self
                .replay_identity_operation::<()>(
                    IdentityOperationActor::User(user_id),
                    operation_id,
                    request_hash,
                    "identity_backup",
                    Some(request.password.as_bytes()),
                )
                .await?
        {
            return Ok(outcome);
        }
        let actor = self.authenticate_actor(bearer).await?;
        let user_id = require_session(actor)?.1;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let password_hash = sqlx::query_scalar::<_, String>(
            "SELECT password_hash FROM users WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(invalid_credentials)?;
        verify_password(&request.password, &password_hash)?;
        sqlx::query("UPDATE users SET recovery_secret_hash=$1 WHERE id=$2")
            .bind(replacement.as_slice())
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        record_identity_operation(
            &mut tx,
            IdentityOperationActor::User(user_id),
            operation_id,
            request_hash,
            "identity_backup",
            Some(&password_hash),
            &(),
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(())
    }

    pub async fn whoami(&self, bearer: &str) -> Result<IdentityInfo, ApiError> {
        let actor = self.authenticate_actor(bearer).await?;
        let user_id = match actor {
            AuthActor::Session { user_id, .. } => user_id,
            AuthActor::Automation { user_id, scopes } => {
                if scopes.is_empty() {
                    return Err(ApiError::new(
                        ApiErrorCode::Unauthorized,
                        "automation credential has no active scopes",
                    ));
                }
                user_id
            }
            AuthActor::Installation { .. } => {
                return Err(ApiError::new(
                    ApiErrorCode::Unauthorized,
                    "a claimed identity session is required",
                ));
            }
        };
        self.identity_info(user_id).await
    }

    pub async fn devices(&self, bearer: &str) -> Result<DeviceList, ApiError> {
        let actor = self.authenticate_actor(bearer).await?;
        let (current_session, user_id) = require_session(actor)?;
        let rows = sqlx::query(
            "SELECT id,installation_id,device_name,(extract(epoch from created_at)*1000)::bigint \
             FROM sessions WHERE user_id=$1 AND revoked_at IS NULL ORDER BY created_at,id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(internal_api_error)?;
        Ok(DeviceList {
            devices: rows
                .into_iter()
                .map(|row| {
                    let id: Uuid = row.get(0);
                    DeviceInfo {
                        session_id: id.to_string(),
                        installation_id: row.get::<Uuid, _>(1).to_string(),
                        device_name: row.get(2),
                        created_at_unix_ms: row.get(3),
                        current: id == current_session,
                    }
                })
                .collect(),
        })
    }

    pub async fn revoke_device(
        &self,
        bearer: &str,
        request: &DeviceRevokeRequest,
    ) -> Result<DeviceRevokeResponse, ApiError> {
        let operation_id = validate_operation_id(&request.operation_id)?;
        let session_id = Uuid::parse_str(&request.session_id)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid session_id"))?;
        let request_hash = validate_request_hash(
            &request.operation_id,
            IdentityMutationDomain::DeviceRevoke,
            &request.session_id,
            &request.request_hash,
        )?;
        if let Some(user_id) = self.session_user_from_bearer_any(bearer).await?
            && let Some(outcome) = self
                .replay_identity_operation(
                    IdentityOperationActor::User(user_id),
                    operation_id,
                    request_hash,
                    "device_revoke",
                    None,
                )
                .await?
        {
            return Ok(outcome);
        }
        let actor = self.authenticate_actor(bearer).await?;
        let (_, user_id) = require_session(actor)?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let changed = sqlx::query(
            "UPDATE sessions SET revoked_at=now() WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL",
        )
        .bind(session_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .rows_affected();
        let outcome = DeviceRevokeResponse {
            session_id: session_id.to_string(),
            revoked: changed == 1,
        };
        record_identity_operation(
            &mut tx,
            IdentityOperationActor::User(user_id),
            operation_id,
            request_hash,
            "device_revoke",
            None,
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn create_automation_token(
        &self,
        bearer: &str,
        request: &AutomationTokenCreateRequest,
    ) -> Result<AutomationTokenCreateResponse, ApiError> {
        let operation_id = validate_operation_id(&request.operation_id)?;
        let token_hash = decode_secret_hash(&request.token_hash, "token_hash")?;
        if request.scopes.is_empty()
            || request.scopes.iter().any(|scope| scope.trim().is_empty())
            || request.expires_in_seconds == 0
            || request.expires_in_seconds > 31 * 24 * 60 * 60
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "automation tokens require non-empty scopes and a TTL up to 31 days",
            ));
        }
        let request_hash = validate_request_hash(
            &request.operation_id,
            IdentityMutationDomain::TokenCreate,
            &(
                &request.token_hash,
                &request.scopes,
                request.expires_in_seconds,
            ),
            &request.request_hash,
        )?;
        if let Some(user_id) = self.session_user_from_bearer_any(bearer).await?
            && let Some(outcome) = self
                .replay_identity_operation(
                    IdentityOperationActor::User(user_id),
                    operation_id,
                    request_hash,
                    "token_create",
                    None,
                )
                .await?
        {
            return Ok(outcome);
        }
        let actor = self.authenticate_actor(bearer).await?;
        let (_, user_id) = require_session(actor)?;
        let token_id = Uuid::now_v7();
        let seconds = i64::try_from(request.expires_in_seconds)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "TTL is too large"))?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let expires_at_unix_ms = sqlx::query_scalar::<_, i64>(
            "INSERT INTO automation_tokens (id,user_id,token_hash,scopes,expires_at) \
             VALUES ($1,$2,$3,$4,now() + ($5 * interval '1 second')) \
             RETURNING (extract(epoch from expires_at)*1000)::bigint",
        )
        .bind(token_id)
        .bind(user_id)
        .bind(token_hash.as_slice())
        .bind(serde_json::to_value(&request.scopes).map_err(|e| {
            ApiError::new(
                ApiErrorCode::Internal,
                format!("scope serialization failed: {e}"),
            )
        })?)
        .bind(seconds)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let outcome = AutomationTokenCreateResponse {
            token_id: token_id.to_string(),
            scopes: request.scopes.clone(),
            expires_at_unix_ms,
        };
        record_identity_operation(
            &mut tx,
            IdentityOperationActor::User(user_id),
            operation_id,
            request_hash,
            "token_create",
            None,
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn automation_tokens(&self, bearer: &str) -> Result<AutomationTokenList, ApiError> {
        let actor = self.authenticate_actor(bearer).await?;
        let (_, user_id) = require_session(actor)?;
        let rows = sqlx::query(
            "SELECT id,scopes,(extract(epoch from created_at)*1000)::bigint, \
                    (extract(epoch from expires_at)*1000)::bigint \
             FROM automation_tokens \
             WHERE user_id=$1 AND revoked_at IS NULL AND expires_at > now() \
             ORDER BY created_at,id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(internal_api_error)?;
        let mut tokens = Vec::with_capacity(rows.len());
        for row in rows {
            let scopes: serde_json::Value = row.get(1);
            let scopes = serde_json::from_value(scopes)
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
            tokens.push(AutomationTokenInfo {
                token_id: row.get::<Uuid, _>(0).to_string(),
                scopes,
                created_at_unix_ms: row.get(2),
                expires_at_unix_ms: row.get(3),
            });
        }
        Ok(AutomationTokenList { tokens })
    }

    pub async fn revoke_automation_token(
        &self,
        bearer: &str,
        request: &AutomationTokenRevokeRequest,
    ) -> Result<AutomationTokenRevokeResponse, ApiError> {
        let operation_id = validate_operation_id(&request.operation_id)?;
        let token_id = Uuid::parse_str(&request.token_id)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid token_id"))?;
        let request_hash = validate_request_hash(
            &request.operation_id,
            IdentityMutationDomain::TokenRevoke,
            &request.token_id,
            &request.request_hash,
        )?;
        if let Some(user_id) = self.session_user_from_bearer_any(bearer).await?
            && let Some(outcome) = self
                .replay_identity_operation(
                    IdentityOperationActor::User(user_id),
                    operation_id,
                    request_hash,
                    "token_revoke",
                    None,
                )
                .await?
        {
            return Ok(outcome);
        }
        let actor = self.authenticate_actor(bearer).await?;
        let (_, user_id) = require_session(actor)?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let changed = sqlx::query(
            "UPDATE automation_tokens SET revoked_at=now() \
             WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL",
        )
        .bind(token_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .rows_affected();
        let outcome = AutomationTokenRevokeResponse {
            token_id: token_id.to_string(),
            revoked: changed == 1,
        };
        record_identity_operation(
            &mut tx,
            IdentityOperationActor::User(user_id),
            operation_id,
            request_hash,
            "token_revoke",
            None,
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn delete_account(
        &self,
        bearer: &str,
        request: &AccountDeleteRequest,
    ) -> Result<AccountDeleteResponse, ApiError> {
        let operation_id = validate_operation_id(&request.operation_id)?;
        let request_hash = validate_request_hash(
            &request.operation_id,
            IdentityMutationDomain::AccountDelete,
            &(),
            &request.request_hash,
        )?;
        if let Some(user_id) = self.session_user_from_bearer_any(bearer).await?
            && let Some(outcome) = self
                .replay_identity_operation(
                    IdentityOperationActor::User(user_id),
                    operation_id,
                    request_hash,
                    "account_delete",
                    Some(request.password.as_bytes()),
                )
                .await?
        {
            return Ok(outcome);
        }
        let actor = self.authenticate_actor(bearer).await?;
        let (_, user_id) = require_session(actor)?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let row = sqlx::query(
            "SELECT u.namespace_id,u.password_hash,n.slug FROM users u \
             JOIN namespaces n ON n.id=u.namespace_id WHERE u.id=$1 AND u.deleted_at IS NULL FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(invalid_credentials)?;
        let namespace_id: Uuid = row.get(0);
        let password_hash: String = row.get(1);
        let username: String = row.get(2);
        verify_password(&request.password, &password_hash)?;
        let owned_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM resources WHERE owner_namespace_id=$1",
        )
        .bind(namespace_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if owned_count != 0 {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                "delete owned resources before deleting this account",
            ));
        }
        sqlx::query("DELETE FROM account_subscriptions WHERE user_id=$1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("UPDATE sessions SET revoked_at=coalesce(revoked_at,now()) WHERE user_id=$1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query(
            "UPDATE automation_tokens SET revoked_at=coalesce(revoked_at,now()) WHERE user_id=$1",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "UPDATE installations SET revoked_at=coalesce(revoked_at,now()) WHERE user_id=$1",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "UPDATE users SET namespace_id=NULL,password_hash=NULL,recovery_secret_hash=NULL,deleted_at=now() WHERE id=$1",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "UPDATE author_principals SET kind='deleted_user' \
             WHERE id IN (SELECT author_principal_id FROM author_principal_users WHERE user_id=$1)",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query("DELETE FROM namespaces WHERE id=$1")
            .bind(namespace_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        let outcome = AccountDeleteResponse {
            deleted: true,
            username: format!("@{username}"),
        };
        record_identity_operation(
            &mut tx,
            IdentityOperationActor::User(user_id),
            operation_id,
            request_hash,
            "account_delete",
            Some(&password_hash),
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub(super) async fn authenticate_actor(&self, bearer: &str) -> Result<AuthActor, ApiError> {
        let raw = decode_secret_value(bearer, "bearer token")?;
        let token_hash: [u8; 32] = Sha256::digest(raw).into();
        if let Some((session_id, user_id, installation_id)) =
            sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
                "SELECT id,user_id,installation_id FROM sessions WHERE token_hash=$1 AND revoked_at IS NULL",
            )
            .bind(token_hash.as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(internal_api_error)?
        {
            return Ok(AuthActor::Session {
                session_id,
                user_id,
                installation_id,
            });
        }
        if let Some((user_id, scopes)) = sqlx::query_as::<_, (Uuid, serde_json::Value)>(
            "SELECT user_id,scopes FROM automation_tokens \
             WHERE token_hash=$1 AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(token_hash.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        {
            let scopes = serde_json::from_value(scopes)
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
            return Ok(AuthActor::Automation { user_id, scopes });
        }
        if let Ok(installation_id) = self.authenticate_installation(bearer).await {
            return Ok(AuthActor::Installation { installation_id });
        }
        Err(ApiError::new(
            ApiErrorCode::Unauthorized,
            "invalid or revoked credential",
        ))
    }

    pub(super) async fn subscription_subject(
        &self,
        bearer: &str,
    ) -> Result<SubscriptionSubject, ApiError> {
        match self.authenticate_actor(bearer).await? {
            AuthActor::Installation { installation_id } => {
                Ok(SubscriptionSubject::Installation(installation_id))
            }
            AuthActor::Session { user_id, .. } => Ok(SubscriptionSubject::User(user_id)),
            AuthActor::Automation { .. } => Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "automation credentials cannot manage direct subscriptions",
            )),
        }
    }

    async fn installation_id_from_bearer_any(
        &self,
        bearer: &str,
    ) -> Result<Option<Uuid>, ApiError> {
        let raw = decode_secret_value(bearer, "bearer token")?;
        let token_hash: [u8; 32] = Sha256::digest(raw).into();
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM installations WHERE credential_hash=$1")
            .bind(token_hash.as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(internal_api_error)
    }

    async fn session_user_from_bearer_any(&self, bearer: &str) -> Result<Option<Uuid>, ApiError> {
        let raw = decode_secret_value(bearer, "bearer token")?;
        let token_hash: [u8; 32] = Sha256::digest(raw).into();
        sqlx::query_scalar::<_, Uuid>("SELECT user_id FROM sessions WHERE token_hash=$1")
            .bind(token_hash.as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(internal_api_error)
    }

    async fn replay_identity_operation<T: DeserializeOwned>(
        &self,
        actor: IdentityOperationActor,
        operation_id: OperationId,
        request_hash: RequestHash,
        operation_kind: &str,
        secret_material: Option<&[u8]>,
    ) -> Result<Option<T>, ApiError> {
        let row = sqlx::query(
            "SELECT request_hash,operation_kind,outcome_json,secret_verifier FROM identity_operations \
             WHERE actor_kind=$1 AND actor_id=$2 AND operation_id=$3",
        )
        .bind(actor.kind())
        .bind(actor.id())
        .bind(operation_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored_hash: Vec<u8> = row.get(0);
        let stored_kind: String = row.get(1);
        if stored_hash.as_slice() != request_hash.as_bytes() || stored_kind != operation_kind {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                "operation_id was already used with different request content",
            ));
        }
        let outcome: serde_json::Value = row.get(2);
        let stored_secret_verifier: Option<String> = row.get(3);
        if !operation_secret_matches(stored_secret_verifier.as_deref(), secret_material) {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                "operation_id was already used with different secret input",
            ));
        }
        serde_json::from_value(outcome)
            .map(Some)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
    }

    async fn identity_info(&self, user_id: Uuid) -> Result<IdentityInfo, ApiError> {
        let row = sqlx::query(
            "SELECT u.namespace_id,n.slug FROM users u JOIN namespaces n ON n.id=u.namespace_id \
             WHERE u.id=$1 AND u.deleted_at IS NULL",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(invalid_credentials)?;
        Ok(IdentityInfo {
            user_id: user_id.to_string(),
            namespace_id: row.get::<Uuid, _>(0).to_string(),
            username: format!("@{}", row.get::<String, _>(1)),
        })
    }
}

async fn record_identity_operation<T: Serialize>(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SubscriptionSubject {
    Installation(Uuid),
    User(Uuid),
}

fn require_session(actor: AuthActor) -> Result<(Uuid, Uuid), ApiError> {
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

async fn link_installation_to_user(
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

async fn adopt_installation_subscriptions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    installation_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO account_subscriptions (user_id,resource_id) \
         SELECT $1,resource_id FROM installation_subscriptions WHERE installation_id=$2 \
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

async fn link_installation_author_to_user(
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

async fn insert_session(
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

fn username_slug(value: &str) -> Result<String, ApiError> {
    let locator = ResourceLocator::from_str(&format!("{value}/identity"))
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    Ok(locator.owner().to_owned())
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.is_empty() {
        Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "password cannot be empty",
        ))
    } else {
        Ok(())
    }
}

fn hash_password(password: &str) -> Result<String, ApiError> {
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

fn operation_secret_bundle(parts: &[&str]) -> Vec<u8> {
    let mut bundled = b"denju:identity-operation-secret:v1\0".to_vec();
    for part in parts {
        let bytes = part.as_bytes();
        let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        bundled.extend_from_slice(&len.to_be_bytes());
        bundled.extend_from_slice(bytes);
    }
    bundled
}

fn hash_operation_secret(secret: &[u8]) -> Result<String, ApiError> {
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

fn operation_secret_matches(stored: Option<&str>, supplied: Option<&[u8]>) -> bool {
    match (stored, supplied) {
        (None, None) => true,
        (Some(encoded), Some(secret)) => PasswordHash::new(encoded)
            .ok()
            .is_some_and(|parsed| Argon2::default().verify_password(secret, &parsed).is_ok()),
        _ => false,
    }
}

fn verify_password(password: &str, encoded: &str) -> Result<(), ApiError> {
    let parsed = PasswordHash::new(encoded).map_err(|_| invalid_credentials())?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| invalid_credentials())
}

fn decode_secret_hash(value: &str, field: &str) -> Result<[u8; 32], ApiError> {
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

fn decode_secret_value(value: &str, field: &str) -> Result<[u8; 32], ApiError> {
    decode_secret_hash(value, field)
}

fn validate_operation_id(value: &str) -> Result<OperationId, ApiError> {
    OperationId::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
}

fn validate_request_hash<T: Serialize>(
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

fn invalid_credentials() -> ApiError {
    ApiError::new(ApiErrorCode::Unauthorized, "invalid identity credential")
}
