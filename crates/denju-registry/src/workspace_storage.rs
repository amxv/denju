use std::collections::BTreeMap;

use denju_core::BlobId;
use denju_wire::{ApiError, ApiErrorCode, PublicSkillManifest};
use uuid::Uuid;

use crate::{ingest_storage::persist_revision, internal_api_error};

pub(crate) struct PrivateRevisionStorage<'a> {
    pub(crate) resource_id: Uuid,
    pub(crate) namespace_id: Uuid,
    pub(crate) author_principal_id: Uuid,
    pub(crate) operation_id: Uuid,
    pub(crate) revision_id: [u8; 32],
    pub(crate) parents: &'a [[u8; 32]],
    pub(crate) manifest: &'a PublicSkillManifest,
    pub(crate) root_tree_id: &'a [u8; 32],
    pub(crate) blobs: &'a BTreeMap<BlobId, u64>,
    pub(crate) snapshot_key: &'a str,
    pub(crate) snapshot_sha: &'a [u8; 32],
    pub(crate) snapshot_size: usize,
}

pub(crate) async fn persist_private_revision_storage(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    revision: PrivateRevisionStorage<'_>,
) -> Result<(), ApiError> {
    persist_revision(
        tx,
        &revision.revision_id,
        revision.root_tree_id,
        revision.author_principal_id,
        revision.operation_id,
    )
    .await?;
    for (ordinal, parent) in revision.parents.iter().enumerate() {
        sqlx::query(
            "INSERT INTO revision_parents (revision_id,parent_revision_id,ordinal) VALUES ($1,$2,$3) \
             ON CONFLICT DO NOTHING",
        )
        .bind(revision.revision_id.as_slice())
        .bind(parent.as_slice())
        .bind(i16::try_from(ordinal).map_err(|_| {
            ApiError::new(ApiErrorCode::Internal, "revision parent ordinal is invalid")
        })?)
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    }
    for blob in revision.blobs.keys() {
        sqlx::query(
            "INSERT INTO revision_blob_reachability (revision_id,blob_id) VALUES ($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(revision.revision_id.as_slice())
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
    }
    sqlx::query(
        "INSERT INTO resource_revision_snapshots \
         (resource_id,revision_id,manifest_json,snapshot_key,snapshot_sha256,snapshot_size) \
         VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING",
    )
    .bind(revision.resource_id)
    .bind(revision.revision_id.as_slice())
    .bind(
        serde_json::to_value(revision.manifest)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?,
    )
    .bind(revision.snapshot_key)
    .bind(revision.snapshot_sha.as_slice())
    .bind(i64::try_from(revision.snapshot_size).map_err(|_| {
        ApiError::new(
            ApiErrorCode::Internal,
            "snapshot size exceeds database range",
        )
    })?)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}
