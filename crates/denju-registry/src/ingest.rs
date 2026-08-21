use std::{collections::BTreeMap, str::FromStr};

use denju_core::{
    AuthorPrincipalId, BlobId, OperationId, OwnedSkillEntry, Revision, SkillManifest,
    SkillManifestEntry, SkillManifestTree, TreeEntryKind, build_deterministic_skill_snapshot,
    parse_skill_document, validate_declared_skill_manifest, validate_skill_name,
};
use denju_wire::{
    ApiError, ApiErrorCode, PrivateSkill, PrivateSkillCatalog, PrivateSkillImportCommitRequest,
    PrivateSkillImportPrepareResponse, PrivateSkillImportRequest, PrivateSkillImportResponse,
    PublicSkillManifest, RequestHash, SnapshotDownload, StagedBlobUpload,
    private_skill_import_request_hash,
};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{Registry, RegistryError, internal_api_error};

const GENERATION_ONE: u64 = 1;

#[derive(Debug)]
struct ValidatedImport {
    operation_id: OperationId,
    request_hash: RequestHash,
    manifest: SkillManifest,
    blobs: BTreeMap<BlobId, u64>,
    snapshot_sha256: [u8; 32],
}

#[derive(Debug, FromRow)]
struct ImportOperationRow {
    request_hash: Vec<u8>,
    namespace_id: Uuid,
    resource_id: Uuid,
    slug: String,
    expected_generation: i64,
    revision_id: Vec<u8>,
    manifest_json: Value,
    snapshot_sha256: Vec<u8>,
    snapshot_size: i64,
    state: String,
    outcome_json: Option<Value>,
}

#[derive(Debug, FromRow)]
pub(crate) struct StagingRow {
    pub(crate) blob_id: Vec<u8>,
    pub(crate) size_bytes: i64,
    pub(crate) staging_key: String,
}

#[derive(Debug, FromRow)]
struct PrivateSkillRow {
    resource_id: Uuid,
    owner: String,
    name: String,
    description: String,
    generation: i64,
    revision_id: Vec<u8>,
    manifest_json: Value,
    snapshot_key: String,
    snapshot_sha256: Vec<u8>,
    snapshot_size: i64,
}

impl Registry {
    pub async fn prepare_private_skill_import(
        &self,
        bearer: &str,
        request: &PrivateSkillImportRequest,
    ) -> Result<PrivateSkillImportPrepareResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let validated = self.validate_private_import_request(request)?;
        let author = AuthorPrincipalId::from_uuid(authority.author_principal_id)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let revision = Revision::new(
            validated.manifest.root_tree(),
            Vec::new(),
            author,
            validated.operation_id,
        )
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let revision_id = revision.id();

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM namespaces WHERE id=$1 FOR UPDATE")
            .bind(authority.namespace_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;

        if let Some(existing) =
            fetch_import_operation(&mut tx, authority.user_id, validated.operation_id.as_uuid())
                .await?
        {
            ensure_request_hash(&existing.request_hash, validated.request_hash)?;
            let revision = decode_32(&existing.revision_id, "stored revision ID")?;
            let uploads = if existing.state == "committed" {
                Vec::new()
            } else {
                fetch_staging_rows(&mut tx, authority.user_id, validated.operation_id.as_uuid())
                    .await?
            };
            tx.commit().await.map_err(internal_api_error)?;
            let uploads = self.presign_staging_rows(uploads).await?;
            return Ok(PrivateSkillImportPrepareResponse {
                resource_id: existing.resource_id.to_string(),
                locator: format!("@{}/{}", authority.namespace_slug, existing.slug),
                revision_id: hex::encode(revision),
                generation: GENERATION_ONE,
                committed: existing.state == "committed",
                uploads,
            });
        }

        if request.expected_generation != 0 {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                "new skill import requires expected_generation=0",
            ));
        }
        if sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM resources WHERE owner_namespace_id=$1 AND kind='skill' AND slug=$2",
        )
        .bind(authority.namespace_id)
        .bind(&request.name)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?
            != 0
        {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("@{}/{} already exists", authority.namespace_slug, request.name),
            ));
        }
        if sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM private_import_operations \
             WHERE namespace_id=$1 AND slug=$2 AND state='prepared'",
        )
        .bind(authority.namespace_id)
        .bind(&request.name)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?
            != 0
        {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                format!(
                    "another import is already preparing @{}/{}",
                    authority.namespace_slug, request.name
                ),
            ));
        }

        let resource_id = Uuid::now_v7();
        let manifest_json = serde_json::to_value(&request.manifest)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let snapshot_size = i64::try_from(request.snapshot_size_bytes)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "snapshot is too large"))?;
        sqlx::query(
            "INSERT INTO private_import_operations \
             (user_id,operation_id,request_hash,namespace_id,resource_id,slug,expected_generation,revision_id,root_tree_id,manifest_json,snapshot_sha256,snapshot_size,state) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'prepared')",
        )
        .bind(authority.user_id)
        .bind(validated.operation_id.as_uuid())
        .bind(validated.request_hash.as_bytes().as_slice())
        .bind(authority.namespace_id)
        .bind(resource_id)
        .bind(&request.name)
        .bind(i64::try_from(request.expected_generation).unwrap_or(i64::MAX))
        .bind(revision_id.as_bytes().as_slice())
        .bind(validated.manifest.root_tree().as_bytes().as_slice())
        .bind(manifest_json)
        .bind(validated.snapshot_sha256.as_slice())
        .bind(snapshot_size)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;

        for (blob, size) in &validated.blobs {
            let already_proven = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM namespace_blob_reachability WHERE namespace_id=$1 AND blob_id=$2)",
            )
            .bind(authority.namespace_id)
            .bind(blob.as_bytes().as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if already_proven {
                continue;
            }
            let staging_key = format!(
                "staging/{}/{}/{}",
                validated.operation_id,
                Uuid::now_v7(),
                blob
            );
            sqlx::query(
                "INSERT INTO private_import_staging \
                 (user_id,operation_id,blob_id,size_bytes,staging_key) VALUES ($1,$2,$3,$4,$5)",
            )
            .bind(authority.user_id)
            .bind(validated.operation_id.as_uuid())
            .bind(blob.as_bytes().as_slice())
            .bind(i64::try_from(*size).map_err(|_| {
                ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "object size exceeds database range",
                )
            })?)
            .bind(staging_key)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        }
        let staging =
            fetch_staging_rows(&mut tx, authority.user_id, validated.operation_id.as_uuid())
                .await?;
        tx.commit().await.map_err(internal_api_error)?;
        let uploads = self.presign_staging_rows(staging).await?;
        Ok(PrivateSkillImportPrepareResponse {
            resource_id: resource_id.to_string(),
            locator: format!("@{}/{}", authority.namespace_slug, request.name),
            revision_id: revision_id.to_string(),
            generation: GENERATION_ONE,
            committed: false,
            uploads,
        })
    }

    pub async fn commit_private_skill_import(
        &self,
        bearer: &str,
        request: &PrivateSkillImportCommitRequest,
    ) -> Result<PrivateSkillImportResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let operation =
            fetch_import_operation_pool(&self.pool, authority.user_id, operation_id.as_uuid())
                .await?
                .ok_or_else(|| {
                    ApiError::new(ApiErrorCode::NotFound, "private import operation not found")
                })?;
        ensure_request_hash(&operation.request_hash, supplied_hash)?;
        if operation.state == "committed" {
            return decode_import_outcome(operation.outcome_json);
        }
        if operation.namespace_id != authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "import namespace is unavailable",
            ));
        }
        if operation.expected_generation != 0 {
            return Err(ApiError::new(
                ApiErrorCode::Internal,
                "stored import expected generation is invalid",
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
            "SELECT blob_id,size_bytes,staging_key FROM private_import_staging \
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
                        "required object proof is missing; rerun import prepare",
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

        let owned_entries = owned_entries_from_manifest(&manifest, &bytes_by_blob)?;
        let snapshot = build_deterministic_skill_snapshot(&operation.slug, &owned_entries)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        if snapshot.manifest() != &manifest {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "verified object bytes do not match the declared manifest",
            ));
        }
        let expected_snapshot_sha =
            decode_32(&operation.snapshot_sha256, "stored snapshot SHA-256")?;
        if BlobId::hash(snapshot.bytes()).as_bytes() != &expected_snapshot_sha
            || u64::try_from(snapshot.bytes().len()).ok()
                != u64::try_from(operation.snapshot_size).ok()
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "verified objects do not reproduce the declared deterministic snapshot",
            ));
        }
        let skill_md = owned_entries
            .iter()
            .find_map(|entry| match entry {
                OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                    Some(bytes.as_slice())
                }
                _ => None,
            })
            .ok_or_else(|| {
                ApiError::new(ApiErrorCode::InvalidRequest, "skill is missing SKILL.md")
            })?;
        let document = parse_skill_document(&operation.slug, skill_md)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let description = document.frontmatter().description().to_owned();

        for blob in staging.keys() {
            let bytes = bytes_by_blob.get(blob).ok_or_else(|| {
                ApiError::new(
                    ApiErrorCode::Internal,
                    "staging object is not referenced by manifest",
                )
            })?;
            self.objects
                .put(&canonical_blob_key(*blob), bytes)
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
        let outcome = PrivateSkillImportResponse {
            resource_id: operation.resource_id.to_string(),
            locator: format!("@{}/{}", authority.namespace_slug, operation.slug),
            owner: authority.namespace_slug.clone(),
            name: operation.slug.clone(),
            description: description.clone(),
            generation: GENERATION_ONE,
            revision_id: hex::encode(revision_id),
            manifest: manifest_wire.clone(),
        };

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM namespaces WHERE id=$1 FOR UPDATE")
            .bind(authority.namespace_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        let locked = fetch_import_operation(&mut tx, authority.user_id, operation_id.as_uuid())
            .await?
            .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "import operation disappeared"))?;
        ensure_request_hash(&locked.request_hash, supplied_hash)?;
        if locked.state == "committed" {
            return decode_import_outcome(locked.outcome_json);
        }
        if sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM resources WHERE owner_namespace_id=$1 AND kind='skill' AND slug=$2",
        )
        .bind(authority.namespace_id)
        .bind(&operation.slug)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?
            != 0
        {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("{} changed while import was in progress", outcome.locator),
            ));
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
        for blob in expected_blobs.keys() {
            sqlx::query(
                "INSERT INTO revision_blob_reachability (revision_id,blob_id) VALUES ($1,$2) ON CONFLICT DO NOTHING",
            )
            .bind(revision_id.as_slice())
            .bind(blob.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        }
        sqlx::query(
            "INSERT INTO resources \
             (id,owner_namespace_id,slug,kind,visibility,description,generation,latest_release_version) \
             VALUES ($1,$2,$3,'skill','private',$4,1,NULL)",
        )
        .bind(operation.resource_id)
        .bind(authority.namespace_id)
        .bind(&operation.slug)
        .bind(&description)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO skill_private_workspaces \
             (resource_id,revision_id,generation,manifest_json,snapshot_key,snapshot_sha256,snapshot_size) \
             VALUES ($1,$2,1,$3,$4,$5,$6)",
        )
        .bind(operation.resource_id)
        .bind(revision_id.as_slice())
        .bind(serde_json::to_value(&manifest_wire).map_err(|error| {
            ApiError::new(ApiErrorCode::Internal, error.to_string())
        })?)
        .bind(&snapshot_key)
        .bind(snapshot_sha.as_bytes().as_slice())
        .bind(i64::try_from(snapshot.bytes().len()).map_err(|_| {
            ApiError::new(ApiErrorCode::Internal, "snapshot size exceeds database range")
        })?)
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
        for blob in expected_blobs.keys() {
            sqlx::query(
                "INSERT INTO resource_blob_reachability (resource_id,blob_id,reference_count) \
                 VALUES ($1,$2,1)",
            )
            .bind(operation.resource_id)
            .bind(blob.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            sqlx::query(
                "INSERT INTO namespace_blob_reachability (namespace_id,blob_id,reference_count) \
                 VALUES ($1,$2,1) \
                 ON CONFLICT(namespace_id,blob_id) DO UPDATE SET reference_count=namespace_blob_reachability.reference_count+1",
            )
            .bind(authority.namespace_id)
            .bind(blob.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        }
        let outcome_json = serde_json::to_value(&outcome)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        sqlx::query(
            "UPDATE private_import_operations SET state='committed',outcome_json=$1,updated_at=now() \
             WHERE user_id=$2 AND operation_id=$3 AND state='prepared'",
        )
        .bind(outcome_json)
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

    pub async fn private_skill_catalog(
        &self,
        bearer: &str,
    ) -> Result<PrivateSkillCatalog, ApiError> {
        let authority = self.user_authority(bearer, "skills:read").await?;
        let rows = sqlx::query_as::<_, PrivateSkillRow>(
            "SELECT r.id AS resource_id,n.slug AS owner,r.slug AS name,r.description,r.generation, \
                    w.revision_id,w.manifest_json,w.snapshot_key,w.snapshot_sha256,w.snapshot_size \
             FROM resources r \
             JOIN namespaces n ON n.id=r.owner_namespace_id \
             JOIN skill_private_workspaces w ON w.resource_id=r.id \
             WHERE r.owner_namespace_id=$1 AND r.kind='skill' \
             ORDER BY r.slug,r.id",
        )
        .bind(authority.namespace_id)
        .fetch_all(&self.pool)
        .await
        .map_err(internal_api_error)?;
        let mut skills = Vec::with_capacity(rows.len());
        for row in rows {
            let manifest = serde_json::from_value(row.manifest_json)
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
            let revision_id = decode_32(&row.revision_id, "stored revision ID")?;
            let snapshot_sha = decode_32(&row.snapshot_sha256, "stored snapshot SHA-256")?;
            let generation = u64::try_from(row.generation).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "stored generation is invalid")
            })?;
            let snapshot_size = u64::try_from(row.snapshot_size).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "stored snapshot size is invalid")
            })?;
            let url = self
                .objects
                .presign_get(&row.snapshot_key)
                .await
                .map_err(object_store_api_error)?;
            skills.push(PrivateSkill {
                resource_id: row.resource_id.to_string(),
                locator: format!("@{}/{}", row.owner, row.name),
                owner: row.owner,
                name: row.name,
                description: row.description,
                generation,
                revision_id: hex::encode(revision_id),
                manifest,
                snapshot: SnapshotDownload {
                    sha256: hex::encode(snapshot_sha),
                    size_bytes: snapshot_size,
                    url,
                },
            });
        }
        Ok(PrivateSkillCatalog { skills })
    }

    fn validate_private_import_request(
        &self,
        request: &PrivateSkillImportRequest,
    ) -> Result<ValidatedImport, ApiError> {
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        validate_skill_name(&request.name)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let expected_hash = private_skill_import_request_hash(
            &request.operation_id,
            request.expected_generation,
            &request.name,
            &request.manifest,
            &request.snapshot_sha256,
            request.snapshot_size_bytes,
        )
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        if supplied_hash != expected_hash {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequestHash,
                "request_hash does not match the canonical import payload",
            ));
        }
        if request.snapshot_size_bytes > self.limits.max_release_bytes {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "deterministic snapshot exceeds registry release-size limit",
            ));
        }
        if request.snapshot_size_bytes > self.limits.max_transfer_bytes {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "deterministic snapshot exceeds registry transfer limit",
            ));
        }
        let snapshot_sha256 = crate::decode_hash(&request.snapshot_sha256, "snapshot_sha256")?;
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
        Ok(ValidatedImport {
            operation_id,
            request_hash: supplied_hash,
            manifest,
            blobs,
            snapshot_sha256,
        })
    }

    async fn presign_staging_rows(
        &self,
        rows: Vec<StagingRow>,
    ) -> Result<Vec<StagedBlobUpload>, ApiError> {
        let mut uploads = Vec::with_capacity(rows.len());
        for row in rows {
            let blob = decode_32(&row.blob_id, "staging blob ID")?;
            let size = u64::try_from(row.size_bytes).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "stored staging size is invalid")
            })?;
            let url = self
                .objects
                .presign_put(&row.staging_key, size)
                .await
                .map_err(object_store_api_error)?;
            uploads.push(StagedBlobUpload {
                blob_id: hex::encode(blob),
                size_bytes: size,
                url,
            });
        }
        Ok(uploads)
    }
}

async fn fetch_import_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: Uuid,
) -> Result<Option<ImportOperationRow>, ApiError> {
    sqlx::query_as::<_, ImportOperationRow>(
        "SELECT request_hash,namespace_id,resource_id,slug,expected_generation,revision_id,manifest_json, \
                snapshot_sha256,snapshot_size,state,outcome_json \
         FROM private_import_operations WHERE user_id=$1 AND operation_id=$2",
    )
    .bind(user_id)
    .bind(operation_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)
}

async fn fetch_import_operation_pool(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    operation_id: Uuid,
) -> Result<Option<ImportOperationRow>, ApiError> {
    sqlx::query_as::<_, ImportOperationRow>(
        "SELECT request_hash,namespace_id,resource_id,slug,expected_generation,revision_id,manifest_json, \
                snapshot_sha256,snapshot_size,state,outcome_json \
         FROM private_import_operations WHERE user_id=$1 AND operation_id=$2",
    )
    .bind(user_id)
    .bind(operation_id)
    .fetch_optional(pool)
    .await
    .map_err(internal_api_error)
}

async fn fetch_staging_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: Uuid,
) -> Result<Vec<StagingRow>, ApiError> {
    sqlx::query_as::<_, StagingRow>(
        "SELECT blob_id,size_bytes,staging_key FROM private_import_staging \
         WHERE user_id=$1 AND operation_id=$2 ORDER BY blob_id",
    )
    .bind(user_id)
    .bind(operation_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal_api_error)
}

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

fn decode_import_outcome(value: Option<Value>) -> Result<PrivateSkillImportResponse, ApiError> {
    serde_json::from_value(value.ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::Internal,
            "committed import has no stored outcome",
        )
    })?)
    .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
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
