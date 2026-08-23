use denju_wire::{ApiError, ApiErrorCode};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    Registry,
    identity_support::{AuthActor, SubscriptionSubject, decode_secret_value},
    internal_api_error,
};

impl Registry {
    pub(super) async fn authenticate_actor(&self, bearer: &str) -> Result<AuthActor, ApiError> {
        let raw = decode_secret_value(bearer, "bearer token")?;
        let token_hash: [u8; 32] = Sha256::digest(raw).into();
        if let Some((session_id, user_id, installation_id)) =
            sqlx::query_as::<_, (Uuid, Uuid, Uuid)>(
                "SELECT session_id,user_id,installation_id FROM denju_authenticate_session($1)",
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
            "SELECT user_id,scopes FROM denju_authenticate_automation($1)",
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
}
