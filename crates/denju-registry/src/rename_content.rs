use std::collections::BTreeMap;

use denju_core::{
    BlobId, OwnedSkillEntry, build_deterministic_skill_snapshot, validate_declared_skill_manifest,
};
use denju_wire::{ApiError, ApiErrorCode, PublicSkillManifest};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    Registry,
    identity_support::UserAuthority,
    ingest::{
        StagingRow, decode_32, manifest_blobs, object_store_api_error, owned_entries_from_manifest,
        verify_blob,
    },
    internal_api_error,
};

#[derive(Debug)]
pub(crate) struct PreparedRenameContent {
    pub(crate) entries: Vec<OwnedSkillEntry>,
    pub(crate) staging_keys: Vec<String>,
}

pub(crate) struct PreparedRenameExpectation<'a> {
    pub(crate) operation_id: Uuid,
    pub(crate) resource_id: Uuid,
    pub(crate) namespace_id: Uuid,
    pub(crate) generation: i64,
    pub(crate) parent_revision_id: &'a [u8],
    pub(crate) current_name: &'a str,
}

#[derive(Debug, FromRow)]
struct PreparedRevisionRow {
    namespace_id: Uuid,
    resource_id: Uuid,
    expected_generation: i64,
    expected_head_revision_id: Vec<u8>,
    manifest_json: Value,
    state: String,
}

impl Registry {
    pub(crate) async fn verified_prepared_rename_content(
        &self,
        authority: &UserAuthority,
        expectation: PreparedRenameExpectation<'_>,
    ) -> Result<PreparedRenameContent, ApiError> {
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        let operation = sqlx::query_as::<_, PreparedRevisionRow>(
            "SELECT namespace_id,resource_id,expected_generation,expected_head_revision_id,manifest_json,state \
             FROM private_revision_operations WHERE user_id=$1 AND operation_id=$2",
        )
        .bind(authority.user_id)
        .bind(expectation.operation_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| {
            ApiError::new(
                ApiErrorCode::InvalidRequest,
                "prepared rename content was not found",
            )
        })?;
        if operation.state != "prepared"
            || operation.namespace_id != expectation.namespace_id
            || operation.resource_id != expectation.resource_id
            || operation.expected_generation != expectation.generation
            || operation.expected_head_revision_id.as_slice() != expectation.parent_revision_id
        {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                "prepared rename content no longer matches the current workspace",
            ));
        }

        let manifest_wire: PublicSkillManifest = serde_json::from_value(operation.manifest_json)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let manifest = manifest_wire
            .to_core()
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error))?;
        validate_declared_skill_manifest(&manifest)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let expected_blobs = manifest_blobs(&manifest)?;
        let staging_rows = sqlx::query_as::<_, StagingRow>(
            "SELECT blob_id,size_bytes,staging_key FROM private_revision_staging \
             WHERE user_id=$1 AND operation_id=$2 ORDER BY blob_id",
        )
        .bind(authority.user_id)
        .bind(expectation.operation_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let mut staging = BTreeMap::new();
        for row in staging_rows {
            let blob = BlobId::from_bytes(decode_32(&row.blob_id, "staging blob ID")?);
            let size = u64::try_from(row.size_bytes).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "stored staging size is invalid")
            })?;
            if !expected_blobs.contains_key(&blob) {
                return Err(ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "prepared rename contains an unexpected staged object",
                ));
            }
            staging.insert(blob, (size, row.staging_key));
        }

        let mut canonical = BTreeMap::<BlobId, (i64, String)>::new();
        for (blob, expected_size) in &expected_blobs {
            if staging.contains_key(blob) {
                continue;
            }
            let row = sqlx::query_as::<_, (i64, String)>(
                "SELECT cb.size_bytes,cb.object_key FROM namespace_blob_reachability nbr \
                 JOIN canonical_blobs cb ON cb.blob_id=nbr.blob_id \
                 WHERE nbr.namespace_id=$1 AND nbr.blob_id=$2",
            )
            .bind(expectation.namespace_id)
            .bind(blob.as_bytes().as_slice())
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_api_error)?
            .ok_or_else(|| {
                ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "prepared rename object proof is missing; retry the rename",
                )
            })?;
            if u64::try_from(row.0).ok() != Some(*expected_size) {
                return Err(ApiError::new(
                    ApiErrorCode::Internal,
                    "canonical object size is invalid",
                ));
            }
            canonical.insert(*blob, row);
        }
        tx.commit().await.map_err(internal_api_error)?;

        let mut bytes_by_blob = BTreeMap::<BlobId, Vec<u8>>::new();
        for (blob, expected_size) in &expected_blobs {
            let bytes = if let Some((staged_size, key)) = staging.get(blob) {
                if staged_size != expected_size {
                    return Err(ApiError::new(
                        ApiErrorCode::InvalidRequest,
                        "prepared rename object size intent changed",
                    ));
                }
                self.objects
                    .get(key)
                    .await
                    .map_err(object_store_api_error)?
            } else {
                let row = canonical.get(blob).ok_or_else(|| {
                    ApiError::new(ApiErrorCode::Internal, "canonical object proof disappeared")
                })?;
                self.objects
                    .get(&row.1)
                    .await
                    .map_err(object_store_api_error)?
            };
            verify_blob(*blob, *expected_size, &bytes)?;
            bytes_by_blob.insert(*blob, bytes);
        }
        let entries = owned_entries_from_manifest(&manifest, &bytes_by_blob)?;
        let snapshot = build_deterministic_skill_snapshot(expectation.current_name, &entries)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        if snapshot.manifest() != &manifest {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "prepared rename bytes do not match the declared manifest",
            ));
        }
        Ok(PreparedRenameContent {
            entries,
            staging_keys: staging.into_values().map(|(_, key)| key).collect(),
        })
    }
}

pub(crate) async fn consume_prepared_rename_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: Uuid,
    resource_id: Uuid,
) -> Result<(), ApiError> {
    let deleted = sqlx::query(
        "DELETE FROM private_revision_operations \
         WHERE user_id=$1 AND operation_id=$2 AND resource_id=$3 AND state='prepared'",
    )
    .bind(user_id)
    .bind(operation_id)
    .bind(resource_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .rows_affected();
    if deleted == 1 {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::GenerationConflict,
            "prepared rename content was consumed concurrently",
        ))
    }
}
