use std::{collections::BTreeMap, str::FromStr};

use denju_core::{
    AuthorPrincipalId, BlobId, OperationId, ResourceId, Revision, RevisionId,
    build_deterministic_skill_snapshot, parse_skill_document, validate_declared_skill_manifest,
};
use denju_wire::{
    ApiError, ApiErrorCode, PrivateRevisionCommitRequest, PrivateRevisionPrepareResponse,
    PrivateRevisionRequest, PrivateRevisionResponse, PublicSkillManifest, RequestHash,
    StagedBlobUpload, private_revision_request_hash,
};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    Registry,
    ingest::{
        StagingRow, canonical_blob_key, decode_32, enforce_namespace_quota, ensure_request_hash,
        manifest_blobs, object_store_api_error, owned_entries_from_manifest,
        persist_canonical_blobs, persist_trees, verify_blob,
    },
    internal_api_error,
};

#[derive(Debug, FromRow)]
struct RevisionOperationRow {
    request_hash: Vec<u8>,
    namespace_id: Uuid,
    resource_id: Uuid,
    expected_generation: i64,
    parent_revision_id: Vec<u8>,
    revision_id: Vec<u8>,
    manifest_json: Value,
    state: String,
    outcome_json: Option<Value>,
}

impl Registry {
    pub async fn prepare_private_revision(
        &self,
        bearer: &str,
        request: &PrivateRevisionRequest,
    ) -> Result<PrivateRevisionPrepareResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let resource_id = ResourceId::from_str(&request.resource_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let parent = RevisionId::from_str(&request.expected_parent_revision_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let expected_hash = private_revision_request_hash(
            &request.operation_id,
            &request.resource_id,
            request.expected_generation,
            &request.expected_parent_revision_id,
            &request.manifest,
        )
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        if supplied_hash != expected_hash {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequestHash,
                "request_hash does not match the canonical private revision payload",
            ));
        }
        let manifest = request
            .manifest
            .to_core()
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error))?;
        validate_declared_skill_manifest(&manifest)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let blobs = manifest_blobs(&manifest)?;
        for size in blobs.values() {
            if *size > self.limits.max_object_bytes {
                return Err(ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "skill contains an object above the registry object-size limit",
                ));
            }
        }
        let author = AuthorPrincipalId::from_uuid(authority.author_principal_id)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let revision = Revision::new(manifest.root_tree(), vec![parent], author, operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let revision_id = revision.id();

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(existing) =
            fetch_revision_operation(&mut tx, authority.user_id, operation_id.as_uuid()).await?
        {
            ensure_request_hash(&existing.request_hash, supplied_hash)?;
            let uploads = if existing.state == "committed" {
                Vec::new()
            } else {
                fetch_revision_staging(&mut tx, authority.user_id, operation_id.as_uuid()).await?
            };
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(PrivateRevisionPrepareResponse {
                resource_id: existing.resource_id.to_string(),
                revision_id: hex::encode(decode_32(&existing.revision_id, "stored revision ID")?),
                generation: u64::try_from(existing.expected_generation)
                    .ok()
                    .and_then(|generation| generation.checked_add(1))
                    .ok_or_else(|| {
                        ApiError::new(ApiErrorCode::Internal, "stored generation is invalid")
                    })?,
                committed: existing.state == "committed",
                uploads: self.presign_revision_staging(uploads).await?,
            });
        }

        let current = sqlx::query_as::<_, (Uuid, i64, Vec<u8>)>(
            "SELECT r.owner_namespace_id,r.generation,w.revision_id \
             FROM resources r JOIN skill_private_workspaces w ON w.resource_id=r.id \
             WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL FOR UPDATE",
        )
        .bind(resource_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "private skill not found"))?;
        if current.0 != authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "private skill is unavailable",
            ));
        }
        let expected_generation = i64::try_from(request.expected_generation).map_err(|_| {
            ApiError::new(
                ApiErrorCode::InvalidRequest,
                "generation exceeds database range",
            )
        })?;
        if current.1 != expected_generation || current.2.as_slice() != parent.as_bytes() {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("private workspace advanced to generation {}", current.1),
            ));
        }

        sqlx::query(
            "INSERT INTO private_revision_operations \
             (user_id,operation_id,request_hash,namespace_id,resource_id,expected_generation,parent_revision_id,revision_id,root_tree_id,manifest_json,state) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'prepared')",
        )
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .bind(supplied_hash.as_bytes().as_slice())
        .bind(authority.namespace_id)
        .bind(resource_id.as_uuid())
        .bind(expected_generation)
        .bind(parent.as_bytes().as_slice())
        .bind(revision_id.as_bytes().as_slice())
        .bind(manifest.root_tree().as_bytes().as_slice())
        .bind(serde_json::to_value(&request.manifest).map_err(|error| {
            ApiError::new(ApiErrorCode::InvalidRequest, error.to_string())
        })?)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;

        for (blob, size) in &blobs {
            let proven = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM namespace_blob_reachability WHERE namespace_id=$1 AND blob_id=$2)",
            )
            .bind(authority.namespace_id)
            .bind(blob.as_bytes().as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if proven {
                continue;
            }
            sqlx::query(
                "INSERT INTO private_revision_staging \
                 (user_id,operation_id,blob_id,size_bytes,staging_key) VALUES ($1,$2,$3,$4,$5)",
            )
            .bind(authority.user_id)
            .bind(operation_id.as_uuid())
            .bind(blob.as_bytes().as_slice())
            .bind(i64::try_from(*size).map_err(|_| {
                ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "object exceeds database range",
                )
            })?)
            .bind(format!(
                "staging/{}/{}/{}",
                operation_id,
                Uuid::now_v7(),
                blob
            ))
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        }
        let staging =
            fetch_revision_staging(&mut tx, authority.user_id, operation_id.as_uuid()).await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(PrivateRevisionPrepareResponse {
            resource_id: resource_id.to_string(),
            revision_id: revision_id.to_string(),
            generation: request.expected_generation.checked_add(1).ok_or_else(|| {
                ApiError::new(ApiErrorCode::InvalidRequest, "generation overflow")
            })?,
            committed: false,
            uploads: self.presign_revision_staging(staging).await?,
        })
    }

    pub async fn commit_private_revision(
        &self,
        bearer: &str,
        request: &PrivateRevisionCommitRequest,
    ) -> Result<PrivateRevisionResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let operation =
            fetch_revision_operation_pool(&self.pool, authority.user_id, operation_id.as_uuid())
                .await?
                .ok_or_else(|| {
                    ApiError::new(ApiErrorCode::NotFound, "private revision not found")
                })?;
        ensure_request_hash(&operation.request_hash, supplied_hash)?;
        if operation.state == "committed" {
            return decode_revision_outcome(operation.outcome_json);
        }
        if operation.namespace_id != authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "private revision namespace is unavailable",
            ));
        }
        let manifest_wire: PublicSkillManifest =
            serde_json::from_value(operation.manifest_json.clone())
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let manifest = manifest_wire
            .to_core()
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error))?;
        let trees = validate_declared_skill_manifest(&manifest)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let expected_blobs = manifest_blobs(&manifest)?;
        let staging_rows = sqlx::query_as::<_, StagingRow>(
            "SELECT blob_id,size_bytes,staging_key FROM private_revision_staging \
             WHERE user_id=$1 AND operation_id=$2 ORDER BY blob_id",
        )
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(internal_api_error)?;
        let mut staging = BTreeMap::new();
        for row in staging_rows {
            let blob = BlobId::from_bytes(decode_32(&row.blob_id, "staging blob ID")?);
            let size = u64::try_from(row.size_bytes).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "stored staging size is invalid")
            })?;
            staging.insert(blob, (size, row.staging_key));
        }

        let mut bytes_by_blob = BTreeMap::<BlobId, Vec<u8>>::new();
        for (blob, expected_size) in &expected_blobs {
            let bytes = if let Some((staged_size, key)) = staging.get(blob) {
                if staged_size != expected_size {
                    return Err(ApiError::new(
                        ApiErrorCode::InvalidRequest,
                        "staged object size intent changed",
                    ));
                }
                self.objects
                    .get(key)
                    .await
                    .map_err(object_store_api_error)?
            } else {
                let row = sqlx::query_as::<_, (i64, String)>(
                    "SELECT cb.size_bytes,cb.object_key FROM namespace_blob_reachability nbr \
                     JOIN canonical_blobs cb ON cb.blob_id=nbr.blob_id \
                     WHERE nbr.namespace_id=$1 AND nbr.blob_id=$2",
                )
                .bind(authority.namespace_id)
                .bind(blob.as_bytes().as_slice())
                .fetch_optional(&self.pool)
                .await
                .map_err(internal_api_error)?
                .ok_or_else(|| {
                    ApiError::new(
                        ApiErrorCode::InvalidRequest,
                        "required object proof is missing; rerun revision prepare",
                    )
                })?;
                if u64::try_from(row.0).ok() != Some(*expected_size) {
                    return Err(ApiError::new(
                        ApiErrorCode::Internal,
                        "canonical object size is invalid",
                    ));
                }
                self.objects
                    .get(&row.1)
                    .await
                    .map_err(object_store_api_error)?
            };
            verify_blob(*blob, *expected_size, &bytes)?;
            bytes_by_blob.insert(*blob, bytes);
        }

        let resource = sqlx::query_as::<_, (String, Uuid)>(
            "SELECT slug,owner_namespace_id FROM resources WHERE id=$1 AND kind='skill' AND deleted_at IS NULL",
        )
        .bind(operation.resource_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "private skill not found"))?;
        if resource.1 != authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "private skill is unavailable",
            ));
        }
        let owned_entries = owned_entries_from_manifest(&manifest, &bytes_by_blob)?;
        let snapshot = build_deterministic_skill_snapshot(&resource.0, &owned_entries)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        if snapshot.manifest() != &manifest {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "verified object bytes do not match the declared manifest",
            ));
        }
        let skill_md = owned_entries
            .iter()
            .find_map(|entry| match entry {
                denju_core::OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                    Some(bytes.as_slice())
                }
                _ => None,
            })
            .ok_or_else(|| {
                ApiError::new(ApiErrorCode::InvalidRequest, "skill is missing SKILL.md")
            })?;
        let document = parse_skill_document(&resource.0, skill_md)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let description = document.frontmatter().description().to_owned();

        for blob in staging.keys() {
            self.objects
                .put(
                    &canonical_blob_key(*blob),
                    bytes_by_blob.get(blob).ok_or_else(|| {
                        ApiError::new(ApiErrorCode::Internal, "verified staged blob disappeared")
                    })?,
                )
                .await
                .map_err(object_store_api_error)?;
        }
        let snapshot_sha = BlobId::hash(snapshot.bytes());
        let snapshot_key = format!("snapshots/sha256/{snapshot_sha}.tar.zst");
        self.objects
            .put(&snapshot_key, snapshot.bytes())
            .await
            .map_err(object_store_api_error)?;

        let revision_id = decode_32(&operation.revision_id, "stored revision ID")?;
        let parent = decode_32(&operation.parent_revision_id, "stored parent revision ID")?;
        let next_generation = operation
            .expected_generation
            .checked_add(1)
            .ok_or_else(|| {
                ApiError::new(ApiErrorCode::Internal, "workspace generation overflow")
            })?;
        let outcome = PrivateRevisionResponse {
            resource_id: operation.resource_id.to_string(),
            generation: u64::try_from(next_generation).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "workspace generation is invalid")
            })?,
            revision_id: hex::encode(revision_id),
            description: description.clone(),
            manifest: manifest_wire.clone(),
        };

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let current = sqlx::query_as::<_, (Uuid, i64, Vec<u8>)>(
            "SELECT r.owner_namespace_id,r.generation,w.revision_id \
             FROM resources r JOIN skill_private_workspaces w ON w.resource_id=r.id \
             WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL FOR UPDATE",
        )
        .bind(operation.resource_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "private skill not found"))?;
        if current.0 != authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "private skill is unavailable",
            ));
        }
        if current.1 != operation.expected_generation || current.2.as_slice() != parent {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("private workspace advanced to generation {}", current.1),
            ));
        }
        let locked = fetch_revision_operation(&mut tx, authority.user_id, operation_id.as_uuid())
            .await?
            .ok_or_else(|| {
                ApiError::new(ApiErrorCode::Internal, "revision operation disappeared")
            })?;
        ensure_request_hash(&locked.request_hash, supplied_hash)?;
        if locked.state == "committed" {
            return decode_revision_outcome(locked.outcome_json);
        }

        enforce_namespace_quota(self, &mut tx, authority.namespace_id, &expected_blobs).await?;
        persist_canonical_blobs(&mut tx, &expected_blobs).await?;
        persist_trees(&mut tx, &trees).await?;
        sqlx::query(
            "INSERT INTO revisions (revision_id,root_tree_id,author_principal_id,operation_id) \
             VALUES ($1,$2,$3,$4) ON CONFLICT(revision_id) DO NOTHING",
        )
        .bind(revision_id.as_slice())
        .bind(manifest.root_tree().as_bytes().as_slice())
        .bind(authority.author_principal_id)
        .bind(operation_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO revision_parents (revision_id,parent_revision_id,ordinal) VALUES ($1,$2,0) \
             ON CONFLICT DO NOTHING",
        )
        .bind(revision_id.as_slice())
        .bind(parent.as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        for blob in expected_blobs.keys() {
            sqlx::query(
                "INSERT INTO revision_blob_reachability (revision_id,blob_id) VALUES ($1,$2) ON CONFLICT DO NOTHING",
            )
            .bind(revision_id.as_slice())
            .bind(blob.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            sqlx::query(
                "INSERT INTO resource_blob_reachability (resource_id,blob_id,reference_count) VALUES ($1,$2,1) \
                 ON CONFLICT(resource_id,blob_id) DO UPDATE SET reference_count=resource_blob_reachability.reference_count+1",
            )
            .bind(operation.resource_id)
            .bind(blob.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            sqlx::query(
                "INSERT INTO namespace_blob_reachability (namespace_id,blob_id,reference_count) VALUES ($1,$2,1) \
                 ON CONFLICT(namespace_id,blob_id) DO UPDATE SET reference_count=namespace_blob_reachability.reference_count+1",
            )
            .bind(authority.namespace_id)
            .bind(blob.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        }
        sqlx::query("UPDATE resources SET generation=$1,description=$2 WHERE id=$3")
            .bind(next_generation)
            .bind(&description)
            .bind(operation.resource_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query(
            "UPDATE skill_private_workspaces SET revision_id=$1,generation=$2,manifest_json=$3,snapshot_key=$4,snapshot_sha256=$5,snapshot_size=$6,updated_at=now() \
             WHERE resource_id=$7",
        )
        .bind(revision_id.as_slice())
        .bind(next_generation)
        .bind(serde_json::to_value(&manifest_wire).map_err(|error| {
            ApiError::new(ApiErrorCode::Internal, error.to_string())
        })?)
        .bind(&snapshot_key)
        .bind(snapshot_sha.as_bytes().as_slice())
        .bind(i64::try_from(snapshot.bytes().len()).map_err(|_| {
            ApiError::new(ApiErrorCode::Internal, "snapshot size exceeds database range")
        })?)
        .bind(operation.resource_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO resource_revision_snapshots \
             (resource_id,revision_id,manifest_json,snapshot_key,snapshot_sha256,snapshot_size) \
             VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING",
        )
        .bind(operation.resource_id)
        .bind(revision_id.as_slice())
        .bind(
            serde_json::to_value(&manifest_wire)
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?,
        )
        .bind(&snapshot_key)
        .bind(snapshot_sha.as_bytes().as_slice())
        .bind(i64::try_from(snapshot.bytes().len()).map_err(|_| {
            ApiError::new(
                ApiErrorCode::Internal,
                "snapshot size exceeds database range",
            )
        })?)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "UPDATE private_revision_operations SET state='committed',outcome_json=$1,updated_at=now() \
             WHERE user_id=$2 AND operation_id=$3 AND state='prepared'",
        )
        .bind(serde_json::to_value(&outcome).map_err(|error| {
            ApiError::new(ApiErrorCode::Internal, error.to_string())
        })?)
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        tx.commit().await.map_err(internal_api_error)?;

        for (_, key) in staging.values() {
            let _ = self.objects.delete(key).await;
        }
        Ok(outcome)
    }

    async fn presign_revision_staging(
        &self,
        rows: Vec<StagingRow>,
    ) -> Result<Vec<StagedBlobUpload>, ApiError> {
        let mut uploads = Vec::with_capacity(rows.len());
        for row in rows {
            let blob = decode_32(&row.blob_id, "staging blob ID")?;
            let size = u64::try_from(row.size_bytes).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "stored staging size is invalid")
            })?;
            uploads.push(StagedBlobUpload {
                blob_id: hex::encode(blob),
                size_bytes: size,
                url: self
                    .objects
                    .presign_put(&row.staging_key, size)
                    .await
                    .map_err(object_store_api_error)?,
            });
        }
        Ok(uploads)
    }
}

async fn fetch_revision_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: Uuid,
) -> Result<Option<RevisionOperationRow>, ApiError> {
    sqlx::query_as::<_, RevisionOperationRow>(
        "SELECT request_hash,namespace_id,resource_id,expected_generation,parent_revision_id,revision_id,manifest_json,state,outcome_json \
         FROM private_revision_operations WHERE user_id=$1 AND operation_id=$2",
    )
    .bind(user_id)
    .bind(operation_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)
}

async fn fetch_revision_operation_pool(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    operation_id: Uuid,
) -> Result<Option<RevisionOperationRow>, ApiError> {
    sqlx::query_as::<_, RevisionOperationRow>(
        "SELECT request_hash,namespace_id,resource_id,expected_generation,parent_revision_id,revision_id,manifest_json,state,outcome_json \
         FROM private_revision_operations WHERE user_id=$1 AND operation_id=$2",
    )
    .bind(user_id)
    .bind(operation_id)
    .fetch_optional(pool)
    .await
    .map_err(internal_api_error)
}

async fn fetch_revision_staging(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: Uuid,
) -> Result<Vec<StagingRow>, ApiError> {
    sqlx::query_as::<_, StagingRow>(
        "SELECT blob_id,size_bytes,staging_key FROM private_revision_staging \
         WHERE user_id=$1 AND operation_id=$2 ORDER BY blob_id",
    )
    .bind(user_id)
    .bind(operation_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal_api_error)
}

fn decode_revision_outcome(value: Option<Value>) -> Result<PrivateRevisionResponse, ApiError> {
    serde_json::from_value(value.ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::Internal,
            "committed private revision has no stored outcome",
        )
    })?)
    .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}
