use std::str::FromStr;

use denju_core::{OperationId, ResourceId};
use denju_wire::{
    ApiError, ApiErrorCode, RequestHash, ShareMutationKind, ShareSkillRequest, ShareSkillResponse,
    share_skill_request_hash,
};
use sqlx::Row;
use uuid::Uuid;

use crate::{Registry, identity_support::username_slug, internal_api_error};

impl Registry {
    pub async fn mutate_private_share(
        &self,
        bearer: &str,
        kind: ShareMutationKind,
        request: &ShareSkillRequest,
    ) -> Result<ShareSkillResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let resource_id = ResourceId::from_str(&request.resource_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let recipient_slug = username_slug(&request.recipient)?;
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let expected_hash = share_skill_request_hash(
            kind,
            &request.operation_id,
            &request.resource_id,
            &request.recipient,
        )
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        if supplied_hash != expected_hash {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequestHash,
                "request_hash does not match the canonical sharing intent",
            ));
        }

        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        if let Some(row) = sqlx::query(
            "SELECT request_hash,resource_id,outcome_json FROM private_share_operations \
             WHERE user_id=$1 AND operation_id=$2",
        )
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        {
            let stored_hash: Vec<u8> = row.get(0);
            let stored_resource: Uuid = row.get(1);
            if stored_hash.as_slice() != supplied_hash.as_bytes()
                || stored_resource != resource_id.as_uuid()
            {
                return Err(ApiError::new(
                    ApiErrorCode::OperationConflict,
                    "operation_id was already used with different sharing intent",
                ));
            }
            return serde_json::from_value(row.get(2))
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()));
        }

        let resource = sqlx::query_as::<_, (Uuid, String, String, i64, String)>(
            "SELECT r.owner_namespace_id,n.slug,r.slug,r.generation,n.kind FROM resources r \
             JOIN namespaces n ON n.id=r.owner_namespace_id \
             WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL FOR UPDATE OF r",
        )
        .bind(resource_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "owned skill not found"))?;
        if resource.4 == "team" {
            if kind == ShareMutationKind::Share {
                return Err(ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "team skills use team membership; new per-skill private shares are unavailable",
                ));
            }
            let role = sqlx::query_scalar::<_, String>(
                "SELECT role FROM team_memberships WHERE team_namespace_id=$1 AND user_id=$2",
            )
            .bind(resource.0)
            .bind(authority.user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if !role
                .as_deref()
                .is_some_and(|role| matches!(role, "owner" | "maintainer"))
            {
                return Err(ApiError::new(
                    ApiErrorCode::Unauthorized,
                    "only team owners and maintainers may revoke inherited private shares",
                ));
            }
        } else if resource.4 != "user" || resource.0 != authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "only the skill owner can change private shares",
            ));
        }
        let recipient = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT u.id,n.slug FROM users u JOIN namespaces n ON n.id=u.namespace_id \
             WHERE n.slug=$1 AND u.deleted_at IS NULL",
        )
        .bind(&recipient_slug)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "recipient user not found"))?;
        if recipient.0 == authority.user_id {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "a user cannot privately share a skill with themselves",
            ));
        }

        let shared = kind == ShareMutationKind::Share;
        if shared {
            sqlx::query(
                "INSERT INTO private_skill_shares (resource_id,recipient_user_id) VALUES ($1,$2) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(resource_id.as_uuid())
            .bind(recipient.0)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        } else {
            sqlx::query(
                "DELETE FROM private_skill_shares WHERE resource_id=$1 AND recipient_user_id=$2",
            )
            .bind(resource_id.as_uuid())
            .bind(recipient.0)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        }
        let locator = format!("@{}/{}", resource.1, resource.2);
        let outcome = ShareSkillResponse {
            resource_id: resource_id.to_string(),
            locator: locator.clone(),
            recipient: format!("@{}", recipient.1),
            shared,
            subscribe_command: shared.then(|| format!("denju subscribe {locator}")),
        };
        sqlx::query(
            "INSERT INTO private_share_operations \
             (user_id,operation_id,request_hash,resource_id,recipient_user_id,shared,outcome_json) \
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .bind(supplied_hash.as_bytes().as_slice())
        .bind(resource_id.as_uuid())
        .bind(recipient.0)
        .bind(shared)
        .bind(
            serde_json::to_value(&outcome)
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?,
        )
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        crate::release::enqueue_resource_wake(
            &mut tx,
            resource_id.as_uuid(),
            u64::try_from(resource.3)
                .map_err(|_| ApiError::new(ApiErrorCode::Internal, "generation is invalid"))?,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.drain_outbox(64).await;
        Ok(outcome)
    }
}
