use std::str::FromStr;

use denju_core::{OperationId, ResourceId};
use denju_wire::{
    ApiError, ApiErrorCode, RequestHash, ResourceTransferRequest, ResourceTransferResponse,
    resource_transfer_request_hash,
};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    Registry, internal_api_error, pack_storage::resolve_all_members,
    team_access::authorize_namespace_publish,
};

impl Registry {
    pub async fn transfer_resource(
        &self,
        bearer: &str,
        request: &ResourceTransferRequest,
    ) -> Result<ResourceTransferResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let resource_id = ResourceId::from_str(&request.resource_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let expected_hash = resource_transfer_request_hash(
            &request.operation_id,
            &request.resource_id,
            request.expected_generation,
            &request.destination_team,
        )
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        if supplied_hash != expected_hash {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequestHash,
                "request_hash does not match the canonical resource transfer payload",
            ));
        }

        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        if let Some((stored_hash, stored_resource, outcome)) =
            sqlx::query_as::<_, (Vec<u8>, Uuid, serde_json::Value)>(
                "SELECT request_hash,resource_id,outcome_json FROM resource_transfer_operations \
                 WHERE user_id=$1 AND operation_id=$2",
            )
            .bind(authority.user_id)
            .bind(operation_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_api_error)?
        {
            if stored_hash.as_slice() != supplied_hash.as_bytes()
                || stored_resource != resource_id.as_uuid()
            {
                return Err(ApiError::new(
                    ApiErrorCode::OperationConflict,
                    "operation_id was already used with different transfer content",
                ));
            }
            let outcome = serde_json::from_value(outcome)
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }

        let destination =
            authorize_namespace_publish(&mut tx, &authority, &request.destination_team).await?;
        if !destination.is_team {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "resources may be transferred only into a team namespace",
            ));
        }
        let row = sqlx::query(
            "SELECT r.owner_namespace_id,n.slug,r.slug,r.kind,r.generation,r.visibility \
             FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
             WHERE r.id=$1 AND r.deleted_at IS NULL FOR UPDATE OF r",
        )
        .bind(resource_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "resource not found"))?;
        let source_namespace_id: Uuid = row.get(0);
        let source_owner: String = row.get(1);
        let slug: String = row.get(2);
        let kind: String = row.get(3);
        let generation: i64 = row.get(4);
        let visibility: String = row.get(5);
        if source_namespace_id != authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "only personally owned resources may be transferred",
            ));
        }
        let expected_generation = i64::try_from(request.expected_generation).map_err(|_| {
            ApiError::new(
                ApiErrorCode::InvalidRequest,
                "generation exceeds database range",
            )
        })?;
        if generation != expected_generation {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("resource advanced to generation {generation}"),
            ));
        }
        let collision = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE owner_namespace_id=$1 AND kind=$2 AND slug=$3 AND deleted_at IS NULL)",
        )
        .bind(destination.namespace_id)
        .bind(&kind)
        .bind(&slug)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if collision {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("destination already contains {slug}"),
            ));
        }

        if kind == "pack" {
            // The transfer changes the private pack's audience from one personal owner to the
            // entire destination team. Re-resolve authored intent under that audience before
            // moving the stable resource; a skill shared only with this user must not become an
            // accidental team capability.
            let _ = resolve_all_members(
                &mut tx,
                authority.user_id,
                destination.namespace_id,
                visibility == "public",
                true,
                resource_id.as_uuid(),
            )
            .await?;
        }

        enforce_transfer_destination_quota(
            self,
            &mut tx,
            resource_id.as_uuid(),
            destination.namespace_id,
        )
        .await?;
        move_namespace_blob_reachability(
            &mut tx,
            resource_id.as_uuid(),
            source_namespace_id,
            destination.namespace_id,
        )
        .await?;
        let next_generation = generation
            .checked_add(1)
            .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "generation overflow"))?;
        sqlx::query(
            "INSERT INTO resource_redirects (namespace_id,kind,old_slug,target_resource_id) \
             VALUES ($1,$2,$3,$4) ON CONFLICT(namespace_id,kind,old_slug) DO UPDATE SET target_resource_id=excluded.target_resource_id",
        )
        .bind(source_namespace_id)
        .bind(&kind)
        .bind(&slug)
        .bind(resource_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query("UPDATE resources SET owner_namespace_id=$1,generation=$2 WHERE id=$3")
            .bind(destination.namespace_id)
            .bind(next_generation)
            .bind(resource_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        let old_locator = if kind == "pack" {
            format!("@{source_owner}/packs/{slug}")
        } else {
            format!("@{source_owner}/{slug}")
        };
        let new_locator = if kind == "pack" {
            format!("@{}/packs/{slug}", destination.namespace_slug)
        } else {
            format!("@{}/{slug}", destination.namespace_slug)
        };
        let outcome = ResourceTransferResponse {
            resource_id: resource_id.to_string(),
            old_locator,
            new_locator,
            generation: u64::try_from(next_generation).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "stored generation is invalid")
            })?,
        };
        sqlx::query(
            "INSERT INTO resource_transfer_operations \
             (user_id,operation_id,request_hash,resource_id,destination_namespace_id,outcome_json) \
             VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .bind(supplied_hash.as_bytes().as_slice())
        .bind(resource_id.as_uuid())
        .bind(destination.namespace_id)
        .bind(
            serde_json::to_value(&outcome)
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?,
        )
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        crate::outbox::enqueue_resource_wake(&mut tx, resource_id.as_uuid(), outcome.generation)
            .await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.drain_outbox(64).await;
        Ok(outcome)
    }
}

async fn enforce_transfer_destination_quota(
    registry: &Registry,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    destination_namespace_id: Uuid,
) -> Result<(), ApiError> {
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(sum(cb.size_bytes),0)::bigint FROM namespace_blob_reachability nbr \
         JOIN canonical_blobs cb ON cb.blob_id=nbr.blob_id WHERE nbr.namespace_id=$1",
    )
    .bind(destination_namespace_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let additional = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(sum(cb.size_bytes),0)::bigint FROM resource_blob_reachability rbr \
         JOIN canonical_blobs cb ON cb.blob_id=rbr.blob_id \
         WHERE rbr.resource_id=$1 AND NOT EXISTS( \
           SELECT 1 FROM namespace_blob_reachability nbr \
           WHERE nbr.namespace_id=$2 AND nbr.blob_id=rbr.blob_id \
         )",
    )
    .bind(resource_id)
    .bind(destination_namespace_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let current = u64::try_from(current).map_err(|_| {
        ApiError::new(
            ApiErrorCode::Internal,
            "destination namespace logical usage is invalid",
        )
    })?;
    let additional = u64::try_from(additional).map_err(|_| {
        ApiError::new(
            ApiErrorCode::Internal,
            "transferred resource logical usage is invalid",
        )
    })?;
    let projected = current.checked_add(additional).ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::QuotaExceeded,
            "destination namespace logical usage overflow",
        )
    })?;
    if projected > registry.limits.namespace_storage_bytes {
        return Err(ApiError::new(
            ApiErrorCode::QuotaExceeded,
            format!(
                "namespace storage quota exceeded: {projected} > {} bytes",
                registry.limits.namespace_storage_bytes
            ),
        ));
    }
    Ok(())
}

async fn move_namespace_blob_reachability(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    source_namespace_id: Uuid,
    destination_namespace_id: Uuid,
) -> Result<(), ApiError> {
    let mismatch = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS( \
           SELECT 1 FROM resource_blob_reachability rbr \
           LEFT JOIN namespace_blob_reachability nbr \
             ON nbr.namespace_id=$2 AND nbr.blob_id=rbr.blob_id \
           WHERE rbr.resource_id=$1 AND COALESCE(nbr.reference_count,0)<rbr.reference_count \
         )",
    )
    .bind(resource_id)
    .bind(source_namespace_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    if mismatch {
        return Err(ApiError::new(
            ApiErrorCode::Internal,
            "namespace blob reachability is inconsistent with the transferred resource",
        ));
    }
    sqlx::query(
        "INSERT INTO namespace_blob_reachability (namespace_id,blob_id,reference_count) \
         SELECT $2,blob_id,reference_count FROM resource_blob_reachability WHERE resource_id=$1 \
         ON CONFLICT(namespace_id,blob_id) DO UPDATE SET \
           reference_count=namespace_blob_reachability.reference_count+excluded.reference_count",
    )
    .bind(resource_id)
    .bind(destination_namespace_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    sqlx::query(
        "DELETE FROM namespace_blob_reachability nbr USING resource_blob_reachability rbr \
         WHERE rbr.resource_id=$1 AND nbr.namespace_id=$2 AND nbr.blob_id=rbr.blob_id \
           AND nbr.reference_count=rbr.reference_count",
    )
    .bind(resource_id)
    .bind(source_namespace_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    sqlx::query(
        "UPDATE namespace_blob_reachability nbr SET reference_count=nbr.reference_count-rbr.reference_count \
         FROM resource_blob_reachability rbr WHERE rbr.resource_id=$1 AND nbr.namespace_id=$2 \
           AND nbr.blob_id=rbr.blob_id AND nbr.reference_count>rbr.reference_count",
    )
    .bind(resource_id)
    .bind(source_namespace_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}
