use std::collections::BTreeMap;

use denju_wire::{
    ApiError, ApiErrorCode, HistoryPruneResponse, ResourceLifecycleRequest, UsageResponse,
    history_prune_request_hash,
};
use sqlx::Row;

use crate::{
    Registry, internal_api_error,
    lifecycle::{
        ensure_generation, ensure_owner, generation_u64, lock_active_owned_skill, next_generation,
        record_lifecycle_operation, validate_resource_lifecycle_request,
    },
    release::enqueue_resource_wake,
};

impl Registry {
    pub async fn usage(&self, bearer: &str) -> Result<UsageResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:read").await?;
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        let storage_used = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(sum(cb.size_bytes),0)::bigint FROM namespace_blob_reachability nbr \
             JOIN canonical_blobs cb ON cb.blob_id=nbr.blob_id WHERE nbr.namespace_id=$1",
        )
        .bind(authority.namespace_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let active_resources = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM resources WHERE owner_namespace_id=$1 AND deleted_at IS NULL",
        )
        .bind(authority.namespace_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let private_revisions = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM resource_revision_snapshots rrs JOIN resources r ON r.id=rrs.resource_id \
             WHERE r.owner_namespace_id=$1 AND NOT EXISTS (SELECT 1 FROM skill_releases sr \
             WHERE sr.resource_id=rrs.resource_id AND sr.revision_id=rrs.revision_id)",
        )
        .bind(authority.namespace_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let prunable_revisions = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM resource_revision_snapshots rrs JOIN resources r ON r.id=rrs.resource_id \
             JOIN skill_private_workspaces w ON w.resource_id=r.id \
             WHERE r.owner_namespace_id=$1 AND rrs.revision_id<>w.revision_id \
             AND NOT EXISTS (SELECT 1 FROM skill_releases sr WHERE sr.resource_id=rrs.resource_id AND sr.revision_id=rrs.revision_id) \
             AND NOT EXISTS (SELECT 1 FROM skill_workspace_conflicts swc WHERE swc.resource_id=rrs.resource_id \
                 AND (swc.base_revision_id=rrs.revision_id OR swc.head_a_revision_id=rrs.revision_id \
                      OR swc.head_b_revision_id=rrs.revision_id OR swc.resolution_revision_id=rrs.revision_id)) \
             AND NOT EXISTS (SELECT 1 FROM skill_proposals sp WHERE sp.source_resource_id=rrs.resource_id \
                 AND sp.closed_revision_id=rrs.revision_id) \
             AND NOT EXISTS (SELECT 1 FROM pack_revision_members prm WHERE prm.skill_resource_id=rrs.resource_id \
                 AND prm.resolved_revision_id=rrs.revision_id)",
        )
        .bind(authority.namespace_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let prunable_bytes = sqlx::query_scalar::<_, i64>(
            "WITH eligible AS ( \
                SELECT rrs.revision_id FROM resource_revision_snapshots rrs \
                JOIN resources r ON r.id=rrs.resource_id JOIN skill_private_workspaces w ON w.resource_id=r.id \
                WHERE r.owner_namespace_id=$1 AND rrs.revision_id<>w.revision_id \
                AND NOT EXISTS (SELECT 1 FROM skill_releases sr WHERE sr.resource_id=rrs.resource_id AND sr.revision_id=rrs.revision_id) \
                AND NOT EXISTS (SELECT 1 FROM skill_workspace_conflicts swc WHERE swc.resource_id=rrs.resource_id \
                    AND (swc.base_revision_id=rrs.revision_id OR swc.head_a_revision_id=rrs.revision_id \
                         OR swc.head_b_revision_id=rrs.revision_id OR swc.resolution_revision_id=rrs.revision_id)) \
                AND NOT EXISTS (SELECT 1 FROM skill_proposals sp WHERE sp.source_resource_id=rrs.resource_id \
                    AND sp.closed_revision_id=rrs.revision_id) \
                AND NOT EXISTS (SELECT 1 FROM pack_revision_members prm WHERE prm.skill_resource_id=rrs.resource_id \
                    AND prm.resolved_revision_id=rrs.revision_id) \
             ), prune_refs AS ( \
                SELECT rbr.blob_id,count(*)::bigint AS refs FROM revision_blob_reachability rbr \
                JOIN eligible e ON e.revision_id=rbr.revision_id GROUP BY rbr.blob_id \
             ) \
             SELECT COALESCE(sum(cb.size_bytes),0)::bigint FROM prune_refs pr \
             JOIN namespace_blob_reachability nbr ON nbr.namespace_id=$1 AND nbr.blob_id=pr.blob_id \
             JOIN canonical_blobs cb ON cb.blob_id=pr.blob_id WHERE nbr.reference_count<=pr.refs",
        )
        .bind(authority.namespace_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        tx.commit().await.map_err(internal_api_error)?;
        let used = nonnegative_u64(storage_used, "namespace usage")?;
        let limit = self.limits.namespace_storage_bytes;
        Ok(UsageResponse {
            storage_limit_bytes: limit,
            storage_used_bytes: used,
            storage_available_bytes: limit.saturating_sub(used),
            active_resources: nonnegative_u64(active_resources, "active resource count")?,
            private_revisions: nonnegative_u64(private_revisions, "private revision count")?,
            prunable_private_revisions: nonnegative_u64(
                prunable_revisions,
                "prunable revision count",
            )?,
            prunable_bytes: nonnegative_u64(prunable_bytes, "prunable byte count")?,
        })
    }

    pub async fn prune_skill_history(
        &self,
        bearer: &str,
        request: &ResourceLifecycleRequest,
    ) -> Result<HistoryPruneResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let (operation_id, resource_id, request_hash) =
            validate_resource_lifecycle_request(request, history_prune_request_hash)?;
        if let Some(outcome) = self
            .replay_lifecycle_operation::<HistoryPruneResponse>(
                authority.user_id,
                operation_id,
                request_hash,
                "history_prune",
                resource_id.as_uuid(),
            )
            .await?
        {
            return Ok(outcome);
        }
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        let locked = lock_active_owned_skill(&mut tx, resource_id.as_uuid()).await?;
        ensure_owner(&locked, authority.namespace_id)?;
        ensure_generation(&locked, request.expected_generation)?;
        let eligible = sqlx::query(
            "SELECT rrs.revision_id FROM resource_revision_snapshots rrs \
             JOIN skill_private_workspaces w ON w.resource_id=rrs.resource_id \
             WHERE rrs.resource_id=$1 AND rrs.revision_id<>w.revision_id \
             AND NOT EXISTS (SELECT 1 FROM skill_releases sr WHERE sr.resource_id=$1 AND sr.revision_id=rrs.revision_id) \
             AND NOT EXISTS (SELECT 1 FROM skill_workspace_conflicts swc WHERE swc.resource_id=$1 \
                 AND (swc.base_revision_id=rrs.revision_id OR swc.head_a_revision_id=rrs.revision_id \
                      OR swc.head_b_revision_id=rrs.revision_id OR swc.resolution_revision_id=rrs.revision_id)) \
             AND NOT EXISTS (SELECT 1 FROM skill_proposals sp WHERE sp.source_resource_id=$1 \
                 AND sp.closed_revision_id=rrs.revision_id) \
             AND NOT EXISTS (SELECT 1 FROM pack_revision_members prm WHERE prm.skill_resource_id=$1 \
                 AND prm.resolved_revision_id=rrs.revision_id) \
             ORDER BY rrs.created_at,rrs.revision_id",
        )
        .bind(resource_id.as_uuid())
        .fetch_all(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let revision_ids = eligible
            .iter()
            .map(|row| row.get::<Vec<u8>, _>(0))
            .collect::<Vec<_>>();
        let mut prune_refs = BTreeMap::<Vec<u8>, i64>::new();
        for revision_id in &revision_ids {
            let blobs = sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT blob_id FROM revision_blob_reachability WHERE revision_id=$1",
            )
            .bind(revision_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            for blob in blobs {
                *prune_refs.entry(blob).or_default() += 1;
            }
        }
        let mut reclaimed_bytes = 0_u64;
        let mut gc_candidates = 0_u64;
        for (blob, refs) in &prune_refs {
            let row = sqlx::query_as::<_, (i64, i64, i64)>(
                "SELECT rbr.reference_count,nbr.reference_count,cb.size_bytes \
                 FROM resource_blob_reachability rbr \
                 JOIN namespace_blob_reachability nbr ON nbr.namespace_id=$2 AND nbr.blob_id=rbr.blob_id \
                 JOIN canonical_blobs cb ON cb.blob_id=rbr.blob_id \
                 WHERE rbr.resource_id=$1 AND rbr.blob_id=$3",
            )
            .bind(resource_id.as_uuid())
            .bind(authority.namespace_id)
            .bind(blob)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            decrement_reachability(
                &mut tx,
                resource_id.as_uuid(),
                authority.namespace_id,
                blob,
                *refs,
                row.0,
                row.1,
            )
            .await?;
            if row.1 <= *refs {
                reclaimed_bytes = reclaimed_bytes
                    .checked_add(nonnegative_u64(row.2, "blob size")?)
                    .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "usage overflow"))?;
            }
            let still_reachable = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM resource_blob_reachability WHERE blob_id=$1) \
                 OR EXISTS(SELECT 1 FROM namespace_blob_reachability WHERE blob_id=$1)",
            )
            .bind(blob)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if !still_reachable {
                sqlx::query(
                    "INSERT INTO canonical_blob_gc (blob_id,eligible_after) \
                     VALUES ($1,now()+($2 * interval '1 second')) \
                     ON CONFLICT(blob_id) DO UPDATE SET marked_at=now(),eligible_after=excluded.eligible_after",
                )
                .bind(blob)
                .bind(i64::try_from(self.gc_grace.as_secs()).unwrap_or(i64::MAX))
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
                gc_candidates += 1;
            }
        }
        for revision_id in &revision_ids {
            sqlx::query(
                "DELETE FROM resource_revision_snapshots WHERE resource_id=$1 AND revision_id=$2",
            )
            .bind(resource_id.as_uuid())
            .bind(revision_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            let still_snapshot = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM resource_revision_snapshots WHERE revision_id=$1) \
                 OR EXISTS(SELECT 1 FROM skill_releases WHERE revision_id=$1)",
            )
            .bind(revision_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if !still_snapshot {
                sqlx::query("DELETE FROM revision_blob_reachability WHERE revision_id=$1")
                    .bind(revision_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(internal_api_error)?;
            }
        }
        let stale_staging_keys = sqlx::query_scalar::<_, String>(
            "SELECT prs.staging_key FROM private_revision_staging prs \
             JOIN private_revision_operations pro ON pro.user_id=prs.user_id AND pro.operation_id=prs.operation_id \
             WHERE pro.resource_id=$1 AND pro.state='prepared' ORDER BY prs.staging_key",
        )
        .bind(resource_id.as_uuid())
        .fetch_all(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "DELETE FROM private_revision_operations WHERE resource_id=$1 AND state='prepared'",
        )
        .bind(resource_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let next = next_generation(locked.generation)?;
        sqlx::query("UPDATE resources SET generation=$1 WHERE id=$2")
            .bind(next)
            .bind(resource_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("UPDATE skill_private_workspaces SET generation=$1 WHERE resource_id=$2")
            .bind(next)
            .bind(resource_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        let generation = generation_u64(next)?;
        let outcome = HistoryPruneResponse {
            resource_id: resource_id.to_string(),
            locator: format!("@{}/{}", locked.owner, locked.name),
            generation,
            pruned_revisions: u64::try_from(revision_ids.len()).unwrap_or(u64::MAX),
            reclaimed_bytes,
            gc_candidates,
        };
        record_lifecycle_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            request_hash,
            resource_id.as_uuid(),
            "history_prune",
            &outcome,
        )
        .await?;
        enqueue_resource_wake(&mut tx, resource_id.as_uuid(), generation).await?;
        tx.commit().await.map_err(internal_api_error)?;
        for key in stale_staging_keys {
            let _ = self.objects.delete(&key).await;
        }
        // Snapshot archives are deterministic derived objects, but active releases/workspaces
        // still reference their keys directly. Pruning therefore removes only the DB reference
        // owned by the pruned revision. A separate derived-cache eviction pass may delete an
        // archive only after proving no remaining workspace/release/snapshot row references it.
        let _ = self.drain_blob_gc(64).await;
        let _ = self.drain_outbox(64).await;
        Ok(outcome)
    }

    pub async fn drain_blob_gc(&self, limit: u32) -> Result<usize, ApiError> {
        let blobs = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT blob_id FROM canonical_blob_gc WHERE eligible_after<=now() ORDER BY eligible_after,blob_id LIMIT $1",
        )
        .bind(i64::from(limit.clamp(1, 256)))
        .fetch_all(&self.worker_pool)
        .await
        .map_err(internal_api_error)?;
        let mut deleted = 0;
        for blob in blobs {
            let mut tx = self.begin_worker_tx().await?;
            let object_key = sqlx::query_scalar::<_, String>(
                "SELECT cb.object_key FROM canonical_blob_gc gc JOIN canonical_blobs cb ON cb.blob_id=gc.blob_id \
                 WHERE gc.blob_id=$1 AND gc.eligible_after<=now() FOR UPDATE OF cb,gc",
            )
            .bind(&blob)
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            let Some(object_key) = object_key else {
                tx.commit().await.map_err(internal_api_error)?;
                continue;
            };
            let reachable = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM resource_blob_reachability WHERE blob_id=$1) \
                 OR EXISTS(SELECT 1 FROM namespace_blob_reachability WHERE blob_id=$1)",
            )
            .bind(&blob)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if reachable {
                sqlx::query("DELETE FROM canonical_blob_gc WHERE blob_id=$1")
                    .bind(&blob)
                    .execute(&mut *tx)
                    .await
                    .map_err(internal_api_error)?;
                tx.commit().await.map_err(internal_api_error)?;
                continue;
            }
            self.objects
                .delete(&object_key)
                .await
                .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
            sqlx::query("DELETE FROM canonical_blobs WHERE blob_id=$1")
                .bind(&blob)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            tx.commit().await.map_err(internal_api_error)?;
            deleted += 1;
        }
        Ok(deleted)
    }
}

async fn decrement_reachability(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: uuid::Uuid,
    namespace_id: uuid::Uuid,
    blob: &[u8],
    prune_refs: i64,
    resource_refs: i64,
    namespace_refs: i64,
) -> Result<(), ApiError> {
    if resource_refs <= prune_refs {
        sqlx::query("DELETE FROM resource_blob_reachability WHERE resource_id=$1 AND blob_id=$2")
            .bind(resource_id)
            .bind(blob)
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
    } else {
        sqlx::query(
            "UPDATE resource_blob_reachability SET reference_count=reference_count-$1 WHERE resource_id=$2 AND blob_id=$3",
        )
        .bind(prune_refs)
        .bind(resource_id)
        .bind(blob)
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    }
    if namespace_refs <= prune_refs {
        sqlx::query("DELETE FROM namespace_blob_reachability WHERE namespace_id=$1 AND blob_id=$2")
            .bind(namespace_id)
            .bind(blob)
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
    } else {
        sqlx::query(
            "UPDATE namespace_blob_reachability SET reference_count=reference_count-$1 WHERE namespace_id=$2 AND blob_id=$3",
        )
        .bind(prune_refs)
        .bind(namespace_id)
        .bind(blob)
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    }
    Ok(())
}

fn nonnegative_u64(value: i64, field: &str) -> Result<u64, ApiError> {
    u64::try_from(value)
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, format!("stored {field} is invalid")))
}
