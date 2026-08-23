use denju_wire::{
    AccountDeleteRequest, AccountDeleteResponse, ApiError, ApiErrorCode, IdentityMutationDomain,
};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    Registry,
    identity_support::{
        IdentityOperationActor, invalid_credentials, record_identity_operation, require_session,
        validate_operation_id, validate_request_hash, verify_password,
    },
    internal_api_error,
    teams::remove_team_workspaces_for_user,
};

impl Registry {
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
        let mut tx = self.begin_actor_tx(user_id).await?;
        let row = sqlx::query(
            "SELECT u.namespace_id,denju_actor_password_hash(u.id),n.slug FROM users u \
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
        let owned_team = sqlx::query_scalar::<_, String>(
            "SELECT n.slug FROM team_memberships tm \
             JOIN namespaces n ON n.id=tm.team_namespace_id \
             WHERE tm.user_id=$1 AND tm.role='owner' ORDER BY n.slug LIMIT 1 FOR UPDATE OF tm",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if let Some(team) = owned_team {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                format!(
                    "account owns @{team}; team ownership succession must be completed before deleting the account"
                ),
            ));
        }
        let joined_teams = sqlx::query_scalar::<_, Uuid>(
            "SELECT team_namespace_id FROM team_memberships \
             WHERE user_id=$1 ORDER BY team_namespace_id FOR UPDATE",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        for team_id in joined_teams {
            remove_team_workspaces_for_user(&mut tx, team_id, user_id).await?;
        }
        sqlx::query("DELETE FROM team_memberships WHERE user_id=$1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query(
            "UPDATE team_invites SET revoked_at=now() \
             WHERE created_by_user_id=$1 AND used_at IS NULL AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let _resource_wakes = self
            .tombstone_owned_resources_for_account_delete(&mut tx, namespace_id, &username)
            .await?;
        sqlx::query("DELETE FROM account_subscriptions WHERE user_id=$1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("DELETE FROM user_follows WHERE follower_user_id=$1 OR followed_user_id=$1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("SELECT denju_remove_actor_stars()")
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("UPDATE resource_reports SET reporter_user_id=NULL WHERE reporter_user_id=$1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("DELETE FROM social_operations WHERE user_id=$1")
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
            "DELETE FROM installation_subscriptions WHERE installation_id IN \
             (SELECT id FROM installations WHERE user_id=$1)",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query("DELETE FROM private_import_operations WHERE user_id=$1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("DELETE FROM private_revision_operations WHERE user_id=$1")
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
            "UPDATE users SET namespace_id=NULL,password_hash=NULL,recovery_secret_hash=NULL,bio=NULL,deleted_at=now() WHERE id=$1",
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
        let _ = self.drain_outbox(256).await;
        Ok(outcome)
    }
}
