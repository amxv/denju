use std::collections::BTreeMap;

use denju_core::{
    BlobId, OwnedSkillEntry, SkillManifest, SkillManifestEntry, SkillManifestTree, TreeEntryKind,
};
use denju_wire::{ApiError, ApiErrorCode, RequestHash};
use uuid::Uuid;

use crate::{Registry, RegistryError, internal_api_error};

pub(crate) fn manifest_blobs(manifest: &SkillManifest) -> Result<BTreeMap<BlobId, u64>, ApiError> {
    let mut blobs = BTreeMap::new();
    for entry in manifest.entries() {
        if let SkillManifestEntry::File { blob, size, .. } = entry
            && let Some(existing) = blobs.insert(*blob, *size)
            && existing != *size
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "the same blob ID is declared with inconsistent sizes",
            ));
        }
    }
    Ok(blobs)
}

pub(crate) fn owned_entries_from_manifest(
    manifest: &SkillManifest,
    bytes: &BTreeMap<BlobId, Vec<u8>>,
) -> Result<Vec<OwnedSkillEntry>, ApiError> {
    manifest
        .entries()
        .iter()
        .map(|entry| match entry {
            SkillManifestEntry::File {
                path,
                blob,
                executable,
                ..
            } => Ok(OwnedSkillEntry::File {
                path: path.clone(),
                bytes: bytes.get(blob).cloned().ok_or_else(|| {
                    ApiError::new(ApiErrorCode::Internal, "verified blob disappeared")
                })?,
                executable: *executable,
            }),
            SkillManifestEntry::Directory { path } => {
                Ok(OwnedSkillEntry::Directory { path: path.clone() })
            }
            SkillManifestEntry::Symlink { path, target } => Ok(OwnedSkillEntry::Symlink {
                path: path.clone(),
                target: target.clone(),
            }),
        })
        .collect()
}

pub(crate) fn verify_blob(blob: BlobId, expected_size: u64, bytes: &[u8]) -> Result<(), ApiError> {
    if u64::try_from(bytes.len()).ok() != Some(expected_size) || BlobId::hash(bytes) != blob {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            format!("staged object {blob} failed size or SHA-256 verification"),
        ));
    }
    Ok(())
}

pub(crate) async fn enforce_namespace_quota(
    registry: &Registry,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    namespace_id: Uuid,
    blobs: &BTreeMap<BlobId, u64>,
) -> Result<(), ApiError> {
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(sum(cb.size_bytes),0)::bigint FROM namespace_blob_reachability nbr \
         JOIN canonical_blobs cb ON cb.blob_id=nbr.blob_id WHERE nbr.namespace_id=$1",
    )
    .bind(namespace_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let mut additional = 0_u64;
    for (blob, size) in blobs {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM namespace_blob_reachability WHERE namespace_id=$1 AND blob_id=$2)",
        )
        .bind(namespace_id)
        .bind(blob.as_bytes().as_slice())
        .fetch_one(&mut **tx)
        .await
        .map_err(internal_api_error)?;
        if !exists {
            additional = additional.checked_add(*size).ok_or_else(|| {
                ApiError::new(
                    ApiErrorCode::QuotaExceeded,
                    "namespace logical usage overflow",
                )
            })?;
        }
    }
    let current = u64::try_from(current)
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "namespace logical usage is invalid"))?;
    let projected = current.checked_add(additional).ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::QuotaExceeded,
            "namespace logical usage overflow",
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

pub(crate) async fn persist_canonical_blobs(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    blobs: &BTreeMap<BlobId, u64>,
) -> Result<(), ApiError> {
    for (blob, size) in blobs {
        let size = i64::try_from(*size).map_err(|_| {
            ApiError::new(ApiErrorCode::Internal, "blob size exceeds database range")
        })?;
        let key = canonical_blob_key(*blob);
        sqlx::query(
            "INSERT INTO canonical_blobs (blob_id,size_bytes,object_key) VALUES ($1,$2,$3) \
             ON CONFLICT(blob_id) DO NOTHING",
        )
        .bind(blob.as_bytes().as_slice())
        .bind(size)
        .bind(&key)
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
        let stored = sqlx::query_as::<_, (i64, String)>(
            "SELECT size_bytes,object_key FROM canonical_blobs WHERE blob_id=$1",
        )
        .bind(blob.as_bytes().as_slice())
        .fetch_one(&mut **tx)
        .await
        .map_err(internal_api_error)?;
        if stored != (size, key) {
            return Err(ApiError::new(
                ApiErrorCode::Internal,
                "canonical blob metadata conflicts with its content identity",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn persist_trees(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    trees: &[SkillManifestTree],
) -> Result<(), ApiError> {
    for tree in trees {
        sqlx::query("INSERT INTO merkle_trees (tree_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(tree.id().as_bytes().as_slice())
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
    }
    for tree in trees {
        for entry in tree.entries() {
            match entry.kind() {
                TreeEntryKind::File { blob, executable } => {
                    sqlx::query(
                        "INSERT INTO tree_entries (tree_id,name,kind,blob_id,executable) \
                         VALUES ($1,$2,'file',$3,$4) ON CONFLICT(tree_id,name) DO NOTHING",
                    )
                    .bind(tree.id().as_bytes().as_slice())
                    .bind(entry.name())
                    .bind(blob.as_bytes().as_slice())
                    .bind(*executable)
                    .execute(&mut **tx)
                    .await
                    .map_err(internal_api_error)?;
                }
                TreeEntryKind::Directory { tree: child } => {
                    sqlx::query(
                        "INSERT INTO tree_entries (tree_id,name,kind,child_tree_id) \
                         VALUES ($1,$2,'directory',$3) ON CONFLICT(tree_id,name) DO NOTHING",
                    )
                    .bind(tree.id().as_bytes().as_slice())
                    .bind(entry.name())
                    .bind(child.as_bytes().as_slice())
                    .execute(&mut **tx)
                    .await
                    .map_err(internal_api_error)?;
                }
                TreeEntryKind::Symlink { target } => {
                    sqlx::query(
                        "INSERT INTO tree_entries (tree_id,name,kind,symlink_target) \
                         VALUES ($1,$2,'symlink',$3) ON CONFLICT(tree_id,name) DO NOTHING",
                    )
                    .bind(tree.id().as_bytes().as_slice())
                    .bind(entry.name())
                    .bind(target)
                    .execute(&mut **tx)
                    .await
                    .map_err(internal_api_error)?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn canonical_blob_key(blob: BlobId) -> String {
    let id = blob.to_string();
    format!("blobs/sha256/{}/{id}", &id[..2])
}

pub(crate) fn ensure_request_hash(stored: &[u8], supplied: RequestHash) -> Result<(), ApiError> {
    if stored != supplied.as_bytes() {
        return Err(ApiError::new(
            ApiErrorCode::OperationConflict,
            "operation_id was already used with different request content",
        ));
    }
    Ok(())
}

pub(crate) fn decode_32(value: &[u8], field: &str) -> Result<[u8; 32], ApiError> {
    value.try_into().map_err(|_| {
        ApiError::new(
            ApiErrorCode::Internal,
            format!("{field} is not a 32-byte value"),
        )
    })
}

pub(crate) fn object_store_api_error(error: RegistryError) -> ApiError {
    ApiError::new(ApiErrorCode::Unavailable, error.to_string())
}
