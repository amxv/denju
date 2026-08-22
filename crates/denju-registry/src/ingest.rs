use std::{collections::BTreeMap, str::FromStr};

use denju_core::{
    AuthorPrincipalId, BlobId, OperationId, OwnedSkillEntry, ResourceId, Revision, RevisionId,
    SkillManifest, build_deterministic_skill_snapshot, parse_skill_document,
    validate_declared_skill_manifest, validate_skill_name,
};
use denju_wire::{
    ApiError, ApiErrorCode, ForkImportIntent, PrivateSkillImportCommitRequest,
    PrivateSkillImportPrepareResponse, PrivateSkillImportRequest, PrivateSkillImportResponse,
    PublicSkillManifest, RequestHash, SkillForkProvenance, StagedBlobUpload,
    private_skill_import_request_hash,
};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    Registry, access::user_can_fork_revision, internal_api_error,
    team_access::authorize_namespace_publish,
};

const GENERATION_ONE: u64 = 1;

#[derive(Debug)]
struct ValidatedImport {
    operation_id: OperationId,
    request_hash: RequestHash,
    manifest: SkillManifest,
    blobs: BTreeMap<BlobId, u64>,
    snapshot_sha256: [u8; 32],
    fork: Option<ValidatedFork>,
}

#[derive(Debug, Clone)]
struct ValidatedFork {
    upstream_resource_id: Uuid,
    upstream_revision_id: RevisionId,
    replace_subscription: bool,
    promotion_head_revision_id: Option<RevisionId>,
    historical_skill_name: Option<String>,
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
    revision_author_principal_id: Option<Uuid>,
    fork_upstream_resource_id: Option<Uuid>,
    fork_upstream_revision_id: Option<Vec<u8>>,
    fork_replace_subscription: bool,
    fork_promotion_head_revision_id: Option<Vec<u8>>,
    historical_skill_name: Option<String>,
    state: String,
    outcome_json: Option<Value>,
}

#[derive(Debug, FromRow)]
pub(crate) struct StagingRow {
    pub(crate) blob_id: Vec<u8>,
    pub(crate) size_bytes: i64,
    pub(crate) staging_key: String,
}

impl Registry {
    pub async fn prepare_private_skill_import(
        &self,
        bearer: &str,
        request: &PrivateSkillImportRequest,
    ) -> Result<PrivateSkillImportPrepareResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let validated = self.validate_private_import_request(request)?;
        let revision_author = self
            .revision_author_for_user(&authority, request.revision_author_principal_id.as_deref())
            .await?;
        let author = AuthorPrincipalId::from_uuid(revision_author)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let parents = validated
            .fork
            .iter()
            .map(|fork| fork.upstream_revision_id)
            .collect();
        let revision = Revision::new(
            validated.manifest.root_tree(),
            parents,
            author,
            validated.operation_id,
        )
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let revision_id = revision.id();

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let target = authorize_namespace_publish(&mut tx, &authority, &request.owner).await?;
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM namespaces WHERE id=$1 FOR UPDATE")
            .bind(target.namespace_id)
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
                locator: format!("@{}/{}", target.namespace_slug, existing.slug),
                revision_id: hex::encode(revision),
                generation: GENERATION_ONE,
                committed: existing.state == "committed",
                uploads,
            });
        }

        if let Some(fork) = validated.fork.as_ref() {
            if target.is_team {
                return Err(ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "fork resources are personal; transfer an existing personal resource into a team instead",
                ));
            }
            validate_fork_source(
                &mut tx,
                authority.user_id,
                authority.namespace_id,
                fork.clone(),
            )
            .await?;
            if fork.replace_subscription {
                let subscribed = sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM account_subscriptions WHERE user_id=$1 AND resource_id=$2)",
                )
                .bind(authority.user_id)
                .bind(fork.upstream_resource_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(internal_api_error)?;
                if !subscribed {
                    return Err(ApiError::new(
                        ApiErrorCode::GenerationConflict,
                        "automatic fork source is no longer an active account subscription",
                    ));
                }
            }
        }

        if request.expected_generation != 0 {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                "new skill import requires expected_generation=0",
            ));
        }
        if sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM resources WHERE owner_namespace_id=$1 AND kind='skill' AND slug=$2 AND deleted_at IS NULL",
        )
        .bind(target.namespace_id)
        .bind(&request.name)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?
            != 0
        {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("@{}/{} already exists", target.namespace_slug, request.name),
            ));
        }
        if sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM private_import_operations \
             WHERE namespace_id=$1 AND slug=$2 AND state='prepared'",
        )
        .bind(target.namespace_id)
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
                    target.namespace_slug, request.name
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
             (user_id,operation_id,request_hash,namespace_id,resource_id,slug,expected_generation,revision_id,root_tree_id,manifest_json,snapshot_sha256,snapshot_size, \
              revision_author_principal_id,fork_upstream_resource_id,fork_upstream_revision_id,fork_replace_subscription,fork_promotion_head_revision_id,historical_skill_name,state) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,'prepared')",
        )
        .bind(authority.user_id)
        .bind(validated.operation_id.as_uuid())
        .bind(validated.request_hash.as_bytes().as_slice())
        .bind(target.namespace_id)
        .bind(resource_id)
        .bind(&request.name)
        .bind(i64::try_from(request.expected_generation).unwrap_or(i64::MAX))
        .bind(revision_id.as_bytes().as_slice())
        .bind(validated.manifest.root_tree().as_bytes().as_slice())
        .bind(manifest_json)
        .bind(validated.snapshot_sha256.as_slice())
        .bind(snapshot_size)
        .bind(revision_author)
        .bind(validated.fork.as_ref().map(|fork| fork.upstream_resource_id))
        .bind(
            validated
                .fork
                .as_ref()
                .map(|fork| fork.upstream_revision_id.as_bytes().as_slice().to_vec()),
        )
        .bind(validated.fork.as_ref().is_some_and(|fork| fork.replace_subscription))
        .bind(
            validated
                .fork
                .as_ref()
                .and_then(|fork| fork.promotion_head_revision_id)
                .map(|revision| revision.as_bytes().as_slice().to_vec()),
        )
        .bind(
            validated
                .fork
                .as_ref()
                .and_then(|fork| fork.historical_skill_name.as_deref()),
        )
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;

        for (blob, size) in &validated.blobs {
            let already_proven = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM namespace_blob_reachability WHERE namespace_id=$1 AND blob_id=$2)",
            )
            .bind(target.namespace_id)
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
            locator: format!("@{}/{}", target.namespace_slug, request.name),
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
        let target_slug =
            sqlx::query_scalar::<_, String>("SELECT slug FROM namespaces WHERE id=$1")
                .bind(operation.namespace_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(internal_api_error)?
                .ok_or_else(|| {
                    ApiError::new(ApiErrorCode::NotFound, "import namespace is unavailable")
                })?;
        let mut authority_tx = self.pool.begin().await.map_err(internal_api_error)?;
        let target =
            authorize_namespace_publish(&mut authority_tx, &authority, &target_slug).await?;
        if target.namespace_id != operation.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "import namespace is unavailable",
            ));
        }
        authority_tx.commit().await.map_err(internal_api_error)?;
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
                .bind(operation.namespace_id)
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
        let validation_name = operation
            .historical_skill_name
            .as_deref()
            .unwrap_or(&operation.slug);
        let snapshot = build_deterministic_skill_snapshot(validation_name, &owned_entries)
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
        let document = parse_skill_document(validation_name, skill_md)
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

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let target = authorize_namespace_publish(&mut tx, &authority, &target_slug).await?;
        if target.namespace_id != operation.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "import namespace is unavailable",
            ));
        }
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM namespaces WHERE id=$1 FOR UPDATE")
            .bind(operation.namespace_id)
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
        let revision_author = locked
            .revision_author_principal_id
            .unwrap_or(authority.author_principal_id);
        let fork = import_operation_fork(&locked)?;
        let fork_access = if let Some(fork) = fork.as_ref() {
            Some(
                validate_fork_source(
                    &mut tx,
                    authority.user_id,
                    authority.namespace_id,
                    fork.clone(),
                )
                .await?,
            )
        } else {
            None
        };
        let outcome_fork = fork
            .as_ref()
            .zip(fork_access.as_ref())
            .map(|(fork, access)| SkillForkProvenance {
                upstream_resource_id: fork.upstream_resource_id.to_string(),
                upstream_locator: format!("@{}/{}", access.0, access.1),
                created_from_revision_id: fork.upstream_revision_id.to_string(),
                sync_base_revision_id: fork.upstream_revision_id.to_string(),
            });
        let outcome = PrivateSkillImportResponse {
            resource_id: operation.resource_id.to_string(),
            locator: format!("@{}/{}", target.namespace_slug, operation.slug),
            owner: target.namespace_slug.clone(),
            name: operation.slug.clone(),
            description: description.clone(),
            generation: GENERATION_ONE,
            revision_id: hex::encode(revision_id),
            manifest: manifest_wire.clone(),
            fork: outcome_fork.clone(),
        };
        if sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM resources WHERE owner_namespace_id=$1 AND kind='skill' AND slug=$2 AND deleted_at IS NULL",
        )
        .bind(operation.namespace_id)
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

        enforce_namespace_quota(self, &mut tx, operation.namespace_id, &expected_blobs).await?;
        sqlx::query(
            "DELETE FROM resource_redirects WHERE namespace_id=$1 AND kind='skill' AND old_slug=$2",
        )
        .bind(operation.namespace_id)
        .bind(&operation.slug)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        persist_canonical_blobs(&mut tx, &expected_blobs).await?;
        persist_trees(&mut tx, &trees).await?;
        sqlx::query(
            "INSERT INTO revisions (revision_id,root_tree_id,author_principal_id,operation_id) \
             VALUES ($1,$2,$3,$4) ON CONFLICT(revision_id) DO NOTHING",
        )
        .bind(revision_id.as_slice())
        .bind(manifest.root_tree().as_bytes().as_slice())
        .bind(revision_author)
        .bind(operation_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if let Some(fork) = fork.as_ref() {
            sqlx::query(
                "INSERT INTO revision_parents (revision_id,parent_revision_id,ordinal) VALUES ($1,$2,0) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(revision_id.as_slice())
            .bind(fork.upstream_revision_id.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        }
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
            "DELETE FROM resource_redirects WHERE namespace_id=$1 AND kind='skill' AND old_slug=$2",
        )
        .bind(operation.namespace_id)
        .bind(&operation.slug)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO resources \
             (id,owner_namespace_id,slug,kind,visibility,description,generation,latest_release_version) \
             VALUES ($1,$2,$3,'skill','private',$4,1,NULL)",
        )
        .bind(operation.resource_id)
        .bind(operation.namespace_id)
        .bind(&operation.slug)
        .bind(&description)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if let Some(fork) = fork.as_ref() {
            let promotion_head = fork.promotion_head_revision_id;
            let promotion_pending =
                promotion_head.is_some_and(|head| head.as_bytes() != &revision_id);
            sqlx::query(
                "INSERT INTO skill_forks \
                 (resource_id,upstream_resource_id,created_from_revision_id,sync_base_revision_id,promotion_head_revision_id,promotion_pending) \
                 VALUES ($1,$2,$3,$3,$4,$5)",
            )
            .bind(operation.resource_id)
            .bind(fork.upstream_resource_id)
            .bind(fork.upstream_revision_id.as_bytes().as_slice())
            .bind(promotion_head.map(|head| head.as_bytes().as_slice().to_vec()))
            .bind(promotion_pending)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if fork.replace_subscription {
                sqlx::query(
                    "DELETE FROM account_subscriptions WHERE user_id=$1 AND resource_id=$2",
                )
                .bind(authority.user_id)
                .bind(fork.upstream_resource_id)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
        }
        sqlx::query(
            "INSERT INTO skill_private_workspaces \
             (resource_id,workspace_user_id,description,revision_id,generation,manifest_json,snapshot_key,snapshot_sha256,snapshot_size) \
             VALUES ($1,$2,$3,$4,1,$5,$6,$7,$8)",
        )
        .bind(operation.resource_id)
        .bind(authority.user_id)
        .bind(&description)
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
            .bind(operation.namespace_id)
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
        let expected_hash =
            private_skill_import_request_hash(&denju_wire::PrivateSkillImportRequestHashInput {
                operation_id: &request.operation_id,
                expected_generation: request.expected_generation,
                owner: &request.owner,
                name: &request.name,
                manifest: &request.manifest,
                snapshot_sha256: &request.snapshot_sha256,
                snapshot_size_bytes: request.snapshot_size_bytes,
                revision_author_principal_id: request.revision_author_principal_id.as_deref(),
                fork: request.fork.as_ref(),
            })
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
        let fork = request.fork.as_ref().map(parse_fork_intent).transpose()?;
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
            fork,
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
                snapshot_sha256,snapshot_size,revision_author_principal_id,fork_upstream_resource_id,fork_upstream_revision_id,fork_replace_subscription, \
                fork_promotion_head_revision_id,historical_skill_name,state,outcome_json \
         FROM private_import_operations WHERE user_id=$1 AND operation_id=$2",
    )
    .bind(user_id)
    .bind(operation_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)
}

fn parse_fork_intent(intent: &ForkImportIntent) -> Result<ValidatedFork, ApiError> {
    let upstream_resource_id = ResourceId::from_str(&intent.upstream_resource_id)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    let upstream_revision_id = RevisionId::from_str(&intent.upstream_revision_id)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    let promotion_head_revision_id = intent
        .promotion_head_revision_id
        .as_deref()
        .map(RevisionId::from_str)
        .transpose()
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    if let Some(name) = intent.historical_skill_name.as_deref() {
        validate_skill_name(name)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        if promotion_head_revision_id.is_none() {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "historical fork content requires an explicit promotion head",
            ));
        }
    }
    Ok(ValidatedFork {
        upstream_resource_id: upstream_resource_id.as_uuid(),
        upstream_revision_id,
        replace_subscription: intent.replace_subscription,
        promotion_head_revision_id,
        historical_skill_name: intent.historical_skill_name.clone(),
    })
}

fn import_operation_fork(row: &ImportOperationRow) -> Result<Option<ValidatedFork>, ApiError> {
    match (
        row.fork_upstream_resource_id,
        row.fork_upstream_revision_id.as_deref(),
    ) {
        (None, None)
            if !row.fork_replace_subscription
                && row.fork_promotion_head_revision_id.is_none()
                && row.historical_skill_name.is_none() =>
        {
            Ok(None)
        }
        (Some(resource_id), Some(revision)) => Ok(Some(ValidatedFork {
            upstream_resource_id: resource_id,
            upstream_revision_id: RevisionId::from_bytes(decode_32(
                revision,
                "stored fork upstream revision ID",
            )?),
            replace_subscription: row.fork_replace_subscription,
            promotion_head_revision_id: row
                .fork_promotion_head_revision_id
                .as_deref()
                .map(|revision| {
                    decode_32(revision, "stored fork promotion head revision ID")
                        .map(RevisionId::from_bytes)
                })
                .transpose()?,
            historical_skill_name: row.historical_skill_name.clone(),
        })),
        _ => Err(ApiError::new(
            ApiErrorCode::Internal,
            "stored fork import intent is invalid",
        )),
    }
}

async fn validate_fork_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    namespace_id: Uuid,
    fork: ValidatedFork,
) -> Result<(String, String), ApiError> {
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT n.slug,r.slug FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
         WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL FOR SHARE OF r",
    )
    .bind(fork.upstream_resource_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "fork upstream is unavailable"))?;
    if !user_can_fork_revision(
        tx,
        user_id,
        namespace_id,
        fork.upstream_resource_id,
        fork.upstream_revision_id.as_bytes(),
    )
    .await?
    {
        return Err(ApiError::new(
            ApiErrorCode::NotFound,
            "fork upstream revision is unavailable",
        ));
    }
    Ok(row)
}

async fn fetch_import_operation_pool(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    operation_id: Uuid,
) -> Result<Option<ImportOperationRow>, ApiError> {
    sqlx::query_as::<_, ImportOperationRow>(
        "SELECT request_hash,namespace_id,resource_id,slug,expected_generation,revision_id,manifest_json, \
                snapshot_sha256,snapshot_size,revision_author_principal_id,fork_upstream_resource_id,fork_upstream_revision_id,fork_replace_subscription, \
                fork_promotion_head_revision_id,historical_skill_name,state,outcome_json \
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

pub(crate) use crate::ingest_storage::{
    canonical_blob_key, decode_32, enforce_namespace_quota, ensure_request_hash, manifest_blobs,
    object_store_api_error, owned_entries_from_manifest, persist_canonical_blobs, persist_trees,
    verify_blob,
};

fn decode_import_outcome(value: Option<Value>) -> Result<PrivateSkillImportResponse, ApiError> {
    serde_json::from_value(value.ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::Internal,
            "committed import has no stored outcome",
        )
    })?)
    .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}
