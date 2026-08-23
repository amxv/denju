use std::{collections::BTreeMap, str::FromStr};

use denju_core::{BlobId, OperationId, ResourceId, RevisionId};
use denju_wire::{
    ApiError, ApiErrorCode, PublicSkillManifest, RequestHash, ResourceLifecycleRequest,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{internal_api_error, lifecycle_hash::validate_lifecycle_hash};

use super::LockedResourceRow;

pub(crate) async fn lock_active_owned_skill(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
) -> Result<LockedResourceRow, ApiError> {
    sqlx::query_as::<_, LockedResourceRow>(
        "SELECT r.owner_namespace_id,n.slug AS owner,r.slug AS name,r.visibility,r.generation,r.latest_release_version \
         FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
         WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL FOR UPDATE OF r",
    )
    .bind(resource_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "owned skill not found"))
}

pub(crate) fn ensure_owner(row: &LockedResourceRow, namespace_id: Uuid) -> Result<(), ApiError> {
    if row.owner_namespace_id == namespace_id {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::Unauthorized,
            "owned skill is unavailable",
        ))
    }
}

pub(crate) fn ensure_generation(row: &LockedResourceRow, expected: u64) -> Result<(), ApiError> {
    if row.generation == generation_i64(expected)? {
        Ok(())
    } else {
        Err(generation_conflict(row.generation))
    }
}

pub(crate) fn validate_resource_lifecycle_request(
    request: &ResourceLifecycleRequest,
    hash: fn(&str, &str, u64) -> Result<RequestHash, denju_wire::RequestHashError>,
) -> Result<(OperationId, ResourceId, RequestHash), ApiError> {
    let operation_id = OperationId::from_str(&request.operation_id)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    let resource_id = ResourceId::from_str(&request.resource_id)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    let request_hash = validate_lifecycle_hash(
        &request.request_hash,
        hash(
            &request.operation_id,
            &request.resource_id,
            request.expected_generation,
        ),
    )?;
    Ok((operation_id, resource_id, request_hash))
}

pub(crate) struct RevisionPersistence<'a> {
    pub(crate) revision_id: RevisionId,
    pub(crate) root_tree: denju_core::TreeId,
    pub(crate) author: Uuid,
    pub(crate) operation_id: OperationId,
    pub(crate) parent: Option<RevisionId>,
    pub(crate) blobs: &'a BTreeMap<BlobId, u64>,
    pub(crate) resource_id: Uuid,
    pub(crate) namespace_id: Uuid,
}

pub(crate) async fn persist_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    revision: RevisionPersistence<'_>,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO revisions (revision_id,root_tree_id,author_principal_id,operation_id) VALUES ($1,$2,$3,$4)",
    )
    .bind(revision.revision_id.as_bytes().as_slice())
    .bind(revision.root_tree.as_bytes().as_slice())
    .bind(revision.author)
    .bind(revision.operation_id.as_uuid())
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    if let Some(parent) = revision.parent {
        sqlx::query(
            "INSERT INTO revision_parents (revision_id,parent_revision_id,ordinal) VALUES ($1,$2,0)",
        )
        .bind(revision.revision_id.as_bytes().as_slice())
        .bind(parent.as_bytes().as_slice())
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    }
    for blob in revision.blobs.keys() {
        sqlx::query("INSERT INTO revision_blob_reachability (revision_id,blob_id) VALUES ($1,$2)")
            .bind(revision.revision_id.as_bytes().as_slice())
            .bind(blob.as_bytes().as_slice())
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO resource_blob_reachability (resource_id,blob_id,reference_count) VALUES ($1,$2,1) \
             ON CONFLICT(resource_id,blob_id) DO UPDATE SET reference_count=resource_blob_reachability.reference_count+1",
        )
        .bind(revision.resource_id)
        .bind(blob.as_bytes().as_slice())
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO namespace_blob_reachability (namespace_id,blob_id,reference_count) VALUES ($1,$2,1) \
             ON CONFLICT(namespace_id,blob_id) DO UPDATE SET reference_count=namespace_blob_reachability.reference_count+1",
        )
        .bind(revision.namespace_id)
        .bind(blob.as_bytes().as_slice())
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query("SELECT denju_cancel_blob_gc($1)")
            .bind(blob.as_bytes().as_slice())
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
    }
    Ok(())
}

pub(crate) async fn persist_revision_snapshot(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    revision_id: RevisionId,
    manifest: &PublicSkillManifest,
    snapshot_key: &str,
    snapshot_sha: BlobId,
    snapshot_size: usize,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO resource_revision_snapshots \
         (resource_id,revision_id,manifest_json,snapshot_key,snapshot_sha256,snapshot_size) VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(resource_id)
    .bind(revision_id.as_bytes().as_slice())
    .bind(serde_json::to_value(manifest).map_err(internal_serialization_error)?)
    .bind(snapshot_key)
    .bind(snapshot_sha.as_bytes().as_slice())
    .bind(i64::try_from(snapshot_size).map_err(|_| {
        ApiError::new(ApiErrorCode::Internal, "snapshot size exceeds database range")
    })?)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

pub(crate) async fn record_lifecycle_operation<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    resource_id: Uuid,
    kind: &str,
    outcome: &T,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO skill_lifecycle_operations \
         (user_id,operation_id,request_hash,resource_id,operation_kind,outcome_json) VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(user_id)
    .bind(operation_id.as_uuid())
    .bind(request_hash.as_bytes().as_slice())
    .bind(resource_id)
    .bind(kind)
    .bind(serde_json::to_value(outcome).map_err(internal_serialization_error)?)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

pub(crate) fn generation_i64(value: u64) -> Result<i64, ApiError> {
    i64::try_from(value).map_err(|_| {
        ApiError::new(
            ApiErrorCode::InvalidRequest,
            "generation exceeds database range",
        )
    })
}

pub(crate) fn generation_u64(value: i64) -> Result<u64, ApiError> {
    u64::try_from(value)
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored generation is invalid"))
}

pub(crate) fn next_generation(value: i64) -> Result<i64, ApiError> {
    value
        .checked_add(1)
        .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "resource generation overflow"))
}

pub(crate) fn generation_conflict(current: i64) -> ApiError {
    ApiError::new(
        ApiErrorCode::GenerationConflict,
        format!("resource advanced to generation {current}"),
    )
}

pub(crate) fn internal_serialization_error(error: serde_json::Error) -> ApiError {
    ApiError::new(ApiErrorCode::Internal, error.to_string())
}
