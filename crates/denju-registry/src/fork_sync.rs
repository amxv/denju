use std::str::FromStr;

use denju_core::RevisionId;
use denju_wire::{ApiError, ApiErrorCode, ForkSyncIntent};
use uuid::Uuid;

use crate::{access::user_can_fork_revision, ingest_storage::decode_32, internal_api_error};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ValidatedForkSync {
    pub(crate) expected_base: RevisionId,
    pub(crate) upstream_revision: RevisionId,
}

pub(crate) fn parse_fork_sync_intent(
    intent: &ForkSyncIntent,
) -> Result<ValidatedForkSync, ApiError> {
    let expected_base = RevisionId::from_str(&intent.expected_sync_base_revision_id)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    let upstream_revision = RevisionId::from_str(&intent.upstream_revision_id)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    if expected_base == upstream_revision {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "fork sync target is already the recorded upstream base",
        ));
    }
    Ok(ValidatedForkSync {
        expected_base,
        upstream_revision,
    })
}

pub(crate) async fn validate_fork_sync(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    namespace_id: Uuid,
    fork_resource_id: Uuid,
    sync: ValidatedForkSync,
) -> Result<(), ApiError> {
    let fork = sqlx::query_as::<_, (Uuid, Vec<u8>)>(
        "SELECT upstream_resource_id,sync_base_revision_id FROM skill_forks \
         WHERE resource_id=$1 FOR UPDATE",
    )
    .bind(fork_resource_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::InvalidRequest, "skill is not a fork"))?;
    let stored_base = decode_32(&fork.1, "stored fork sync base revision ID")?;
    if stored_base != *sync.expected_base.as_bytes() {
        return Err(ApiError::new(
            ApiErrorCode::GenerationConflict,
            "fork upstream base advanced; refresh the fork before retrying sync",
        ));
    }

    if !user_can_fork_revision(
        tx,
        user_id,
        namespace_id,
        fork.0,
        sync.upstream_revision.as_bytes(),
    )
    .await?
    {
        return Err(ApiError::new(
            ApiErrorCode::NotFound,
            "fork upstream revision is unavailable",
        ));
    }

    let base_is_ancestor = sqlx::query_scalar::<_, bool>(
        "WITH RECURSIVE ancestry(revision_id) AS ( \
             SELECT $1::bytea \
             UNION \
             SELECT rp.parent_revision_id FROM revision_parents rp \
             JOIN ancestry a ON rp.revision_id=a.revision_id \
         ) SELECT EXISTS(SELECT 1 FROM ancestry WHERE revision_id=$2)",
    )
    .bind(sync.upstream_revision.as_bytes().as_slice())
    .bind(sync.expected_base.as_bytes().as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    if !base_is_ancestor {
        return Err(ApiError::new(
            ApiErrorCode::GenerationConflict,
            "selected upstream revision does not descend from the fork sync base",
        ));
    }
    Ok(())
}

pub(crate) async fn require_pending_fork_promotion(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
) -> Result<(), ApiError> {
    let pending = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM skill_forks WHERE resource_id=$1 AND promotion_pending=TRUE)",
    )
    .bind(resource_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    if !pending {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "historical skill-name validation is only available while promoting a local fork",
        ));
    }
    Ok(())
}
