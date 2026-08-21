use std::{collections::BTreeMap, str::FromStr};

use denju_core::{
    AuthorPrincipalId, BlobId, OperationId, ResourceId, Revision, RevisionId,
    build_deterministic_skill_snapshot, parse_skill_document, validate_declared_skill_manifest,
};
use denju_wire::{
    ApiError, ApiErrorCode, PrivateRevisionCommitRequest, PrivateRevisionCommitResponse,
    PrivateRevisionOperationState, PrivateRevisionPrepareResponse, PrivateRevisionRequest,
    PrivateRevisionResponse, PublicSkillManifest, RequestHash, StagedBlobUpload,
    private_revision_request_hash,
};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    Registry,
    fork_sync::{
        ValidatedForkSync, parse_fork_sync_intent, require_pending_fork_promotion,
        validate_fork_sync,
    },
    ingest::{
        StagingRow, canonical_blob_key, decode_32, enforce_namespace_quota, ensure_request_hash,
        manifest_blobs, object_store_api_error, owned_entries_from_manifest,
        persist_canonical_blobs, persist_trees, verify_blob,
    },
    internal_api_error,
    workspace_conflict::{
        record_workspace_conflict, resolve_workspace_conflict, validate_merge_conflict,
    },
    workspace_storage::{PrivateRevisionStorage, persist_private_revision_storage},
};

#[derive(Debug, FromRow)]
struct RevisionOperationRow {
    request_hash: Vec<u8>,
    namespace_id: Uuid,
    resource_id: Uuid,
    expected_generation: i64,
    expected_head_revision_id: Vec<u8>,
    revision_id: Vec<u8>,
    manifest_json: Value,
    revision_author_principal_id: Option<Uuid>,
    fork_sync_base_revision_id: Option<Vec<u8>>,
    fork_sync_upstream_revision_id: Option<Vec<u8>>,
    historical_skill_name: Option<String>,
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
        let expected_head = RevisionId::from_str(&request.expected_head_revision_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let parents = parse_revision_parents(&request.parent_revision_ids)?;
        let fork_sync = request
            .fork_sync
            .as_ref()
            .map(parse_fork_sync_intent)
            .transpose()?;
        if let Some(name) = request.historical_skill_name.as_deref() {
            denju_core::validate_skill_name(name)
                .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        }
        if !parents.contains(&expected_head) {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "expected_head_revision_id must be one of parent_revision_ids",
            ));
        }
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let expected_hash =
            private_revision_request_hash(&denju_wire::PrivateRevisionRequestHashInput {
                operation_id: &request.operation_id,
                resource_id: &request.resource_id,
                expected_generation: request.expected_generation,
                expected_head_revision_id: &request.expected_head_revision_id,
                parent_revision_ids: &request.parent_revision_ids,
                manifest: &request.manifest,
                revision_author_principal_id: request.revision_author_principal_id.as_deref(),
                fork_sync: request.fork_sync.as_ref(),
                historical_skill_name: request.historical_skill_name.as_deref(),
            })
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
        let revision_author = self
            .revision_author_for_user(&authority, request.revision_author_principal_id.as_deref())
            .await?;
        let author = AuthorPrincipalId::from_uuid(revision_author)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let revision =
            Revision::new(manifest.root_tree(), parents.clone(), author, operation_id)
                .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let revision_id = revision.id();

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(existing) =
            fetch_revision_operation(&mut tx, authority.user_id, operation_id.as_uuid()).await?
        {
            ensure_request_hash(&existing.request_hash, supplied_hash)?;
            let state = revision_operation_state(&existing.state)?;
            let uploads = if state == PrivateRevisionOperationState::Prepared {
                fetch_revision_staging(&mut tx, authority.user_id, operation_id.as_uuid()).await?
            } else {
                Vec::new()
            };
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(PrivateRevisionPrepareResponse {
                resource_id: existing.resource_id.to_string(),
                revision_id: hex::encode(decode_32(&existing.revision_id, "stored revision ID")?),
                expected_generation: u64::try_from(existing.expected_generation).map_err(|_| {
                    ApiError::new(ApiErrorCode::Internal, "stored generation is invalid")
                })?,
                state,
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
        if request.historical_skill_name.is_some() {
            require_pending_fork_promotion(&mut tx, resource_id.as_uuid()).await?;
        }
        let expected_generation = i64::try_from(request.expected_generation).map_err(|_| {
            ApiError::new(
                ApiErrorCode::InvalidRequest,
                "generation exceeds database range",
            )
        })?;
        let current_head_matches = current.2.as_slice() == expected_head.as_bytes();
        let active_conflict = sqlx::query_scalar::<_, Uuid>(
            "SELECT conflict_id FROM skill_workspace_conflicts \
             WHERE resource_id=$1 AND resolved_at IS NULL ORDER BY created_at,conflict_id LIMIT 1",
        )
        .bind(resource_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if let Some(conflict_id) = active_conflict {
            if fork_sync.is_some() {
                return Err(ApiError::new(
                    ApiErrorCode::GenerationConflict,
                    "resolve the active private workspace conflict before syncing this fork",
                ));
            }
            if parents.len() != 2 || operation_id.as_uuid() != conflict_id {
                return Err(ApiError::new(
                    ApiErrorCode::GenerationConflict,
                    "private workspace has an unresolved content conflict; reconcile it before saving another revision",
                ));
            }
            if !current_head_matches || current.1 != expected_generation {
                return Err(ApiError::new(
                    ApiErrorCode::GenerationConflict,
                    "workspace conflict target advanced; reconcile before retrying the merge",
                ));
            }
            validate_merge_conflict(&mut tx, conflict_id, resource_id.as_uuid(), &parents).await?;
        } else if current_head_matches {
            if current.1 != expected_generation {
                return Err(ApiError::new(
                    ApiErrorCode::GenerationConflict,
                    format!("private workspace advanced to generation {}", current.1),
                ));
            }
            if let Some(fork_sync) = fork_sync {
                if parents.len() != 2 || !parents.contains(&fork_sync.upstream_revision) {
                    return Err(ApiError::new(
                        ApiErrorCode::InvalidRequest,
                        "fork sync revision must have the current fork head and selected upstream revision as its two parents",
                    ));
                }
                validate_fork_sync(
                    &mut tx,
                    authority.user_id,
                    authority.namespace_id,
                    resource_id.as_uuid(),
                    fork_sync,
                )
                .await?;
            } else if parents.len() == 2 {
                return Err(ApiError::new(
                    ApiErrorCode::GenerationConflict,
                    "merge revision has no matching active workspace conflict",
                ));
            }
        } else {
            if fork_sync.is_some() {
                return Err(ApiError::new(
                    ApiErrorCode::GenerationConflict,
                    "fork head advanced while syncing upstream; fetch the current fork and retry",
                ));
            }
            if parents.len() != 1 {
                return Err(ApiError::new(
                    ApiErrorCode::GenerationConflict,
                    "merge target advanced again; reconcile the new head before retrying",
                ));
            }
            let expected_head_exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM resource_revision_snapshots \
                 WHERE resource_id=$1 AND revision_id=$2)",
            )
            .bind(resource_id.as_uuid())
            .bind(expected_head.as_bytes().as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if !expected_head_exists {
                return Err(ApiError::new(
                    ApiErrorCode::GenerationConflict,
                    "expected private workspace head is no longer valid history for this resource",
                ));
            }
        }

        sqlx::query(
            "INSERT INTO private_revision_operations \
             (user_id,operation_id,request_hash,namespace_id,resource_id,expected_generation,expected_head_revision_id,revision_id,root_tree_id,manifest_json, \
              revision_author_principal_id,fork_sync_base_revision_id,fork_sync_upstream_revision_id,historical_skill_name,state) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,'prepared')",
        )
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .bind(supplied_hash.as_bytes().as_slice())
        .bind(authority.namespace_id)
        .bind(resource_id.as_uuid())
        .bind(expected_generation)
        .bind(expected_head.as_bytes().as_slice())
        .bind(revision_id.as_bytes().as_slice())
        .bind(manifest.root_tree().as_bytes().as_slice())
        .bind(serde_json::to_value(&request.manifest).map_err(|error| {
            ApiError::new(ApiErrorCode::InvalidRequest, error.to_string())
        })?)
        .bind(revision_author)
        .bind(fork_sync.map(|sync| sync.expected_base.as_bytes().as_slice().to_vec()))
        .bind(fork_sync.map(|sync| sync.upstream_revision.as_bytes().as_slice().to_vec()))
        .bind(request.historical_skill_name.as_deref())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;

        for (ordinal, parent) in parents.iter().enumerate() {
            sqlx::query(
                "INSERT INTO private_revision_operation_parents \
                 (user_id,operation_id,ordinal,parent_revision_id) VALUES ($1,$2,$3,$4)",
            )
            .bind(authority.user_id)
            .bind(operation_id.as_uuid())
            .bind(i16::try_from(ordinal).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "revision parent ordinal is invalid")
            })?)
            .bind(parent.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        }

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
            expected_generation: request.expected_generation,
            state: PrivateRevisionOperationState::Prepared,
            uploads: self.presign_revision_staging(staging).await?,
        })
    }

    pub async fn commit_private_revision(
        &self,
        bearer: &str,
        request: &PrivateRevisionCommitRequest,
    ) -> Result<PrivateRevisionCommitResponse, ApiError> {
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
        if revision_operation_state(&operation.state)? != PrivateRevisionOperationState::Prepared {
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
        let validation_name = operation
            .historical_skill_name
            .as_deref()
            .unwrap_or(&resource.0);
        let snapshot = build_deterministic_skill_snapshot(validation_name, &owned_entries)
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
        let document = parse_skill_document(validation_name, skill_md)
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
        let expected_head = decode_32(
            &operation.expected_head_revision_id,
            "stored expected workspace head",
        )?;
        let parents = fetch_revision_operation_parents_pool(
            &self.pool,
            authority.user_id,
            operation_id.as_uuid(),
        )
        .await?;

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
        let locked = fetch_revision_operation(&mut tx, authority.user_id, operation_id.as_uuid())
            .await?
            .ok_or_else(|| {
                ApiError::new(ApiErrorCode::Internal, "revision operation disappeared")
            })?;
        ensure_request_hash(&locked.request_hash, supplied_hash)?;
        if revision_operation_state(&locked.state)? != PrivateRevisionOperationState::Prepared {
            return decode_revision_outcome(locked.outcome_json);
        }
        if locked.historical_skill_name.is_some() {
            require_pending_fork_promotion(&mut tx, operation.resource_id).await?;
        }
        let revision_author = locked
            .revision_author_principal_id
            .unwrap_or(authority.author_principal_id);
        let fork_sync = revision_operation_fork_sync(&locked)?;
        let current_head = decode_32(&current.2, "stored private workspace head")?;
        let head_matches = current_head == expected_head;
        let active_conflict = sqlx::query_scalar::<_, Uuid>(
            "SELECT conflict_id FROM skill_workspace_conflicts \
             WHERE resource_id=$1 AND resolved_at IS NULL ORDER BY created_at,conflict_id LIMIT 1",
        )
        .bind(operation.resource_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if let Some(conflict_id) = active_conflict
            && (fork_sync.is_some() || parents.len() != 2 || operation_id.as_uuid() != conflict_id)
        {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                "private workspace has an unresolved content conflict; reconcile it before saving another revision",
            ));
        }
        if head_matches && current.1 != operation.expected_generation {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("private workspace advanced to generation {}", current.1),
            ));
        }
        if !head_matches && fork_sync.is_some() {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                "fork head advanced while syncing upstream; fetch the current fork and retry",
            ));
        }
        if !head_matches && parents.len() != 1 {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                "merge target advanced again; reconcile the new head before retrying",
            ));
        }
        if head_matches && parents.len() == 2 {
            let parent_ids = parents
                .iter()
                .copied()
                .map(RevisionId::from_bytes)
                .collect::<Vec<_>>();
            if let Some(fork_sync) = fork_sync {
                if !parent_ids.contains(&fork_sync.upstream_revision) {
                    return Err(ApiError::new(
                        ApiErrorCode::InvalidRequest,
                        "stored fork sync parents do not match the selected upstream revision",
                    ));
                }
                validate_fork_sync(
                    &mut tx,
                    authority.user_id,
                    authority.namespace_id,
                    operation.resource_id,
                    fork_sync,
                )
                .await?;
            } else {
                validate_merge_conflict(
                    &mut tx,
                    operation_id.as_uuid(),
                    operation.resource_id,
                    &parent_ids,
                )
                .await?;
            }
        }

        enforce_namespace_quota(self, &mut tx, authority.namespace_id, &expected_blobs).await?;
        persist_canonical_blobs(&mut tx, &expected_blobs).await?;
        persist_trees(&mut tx, &trees).await?;
        persist_private_revision_storage(
            &mut tx,
            PrivateRevisionStorage {
                resource_id: operation.resource_id,
                namespace_id: authority.namespace_id,
                author_principal_id: revision_author,
                operation_id: operation_id.as_uuid(),
                revision_id,
                parents: &parents,
                manifest: &manifest_wire,
                root_tree_id: manifest.root_tree().as_bytes(),
                blobs: &expected_blobs,
                snapshot_key: &snapshot_key,
                snapshot_sha: snapshot_sha.as_bytes(),
                snapshot_size: snapshot.bytes().len(),
            },
        )
        .await?;

        let (state, outcome, wake_generation) = if head_matches {
            let next_generation = current.1.checked_add(1).ok_or_else(|| {
                ApiError::new(ApiErrorCode::Internal, "workspace generation overflow")
            })?;
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
            if parents.len() == 2 && fork_sync.is_none() {
                resolve_workspace_conflict(
                    &mut tx,
                    operation_id.as_uuid(),
                    operation.resource_id,
                    &revision_id,
                )
                .await?;
            }
            if let Some(fork_sync) = fork_sync {
                sqlx::query(
                    "UPDATE skill_forks SET sync_base_revision_id=$1 WHERE resource_id=$2 AND sync_base_revision_id=$3",
                )
                .bind(fork_sync.upstream_revision.as_bytes().as_slice())
                .bind(operation.resource_id)
                .bind(fork_sync.expected_base.as_bytes().as_slice())
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            sqlx::query(
                "UPDATE skill_forks SET promotion_pending=FALSE \
                 WHERE resource_id=$1 AND promotion_pending=TRUE AND promotion_head_revision_id=$2",
            )
            .bind(operation.resource_id)
            .bind(revision_id.as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            let generation = u64::try_from(next_generation).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "workspace generation is invalid")
            })?;
            (
                "advanced",
                PrivateRevisionCommitResponse::Advanced {
                    revision: PrivateRevisionResponse {
                        resource_id: operation.resource_id.to_string(),
                        generation,
                        revision_id: hex::encode(revision_id),
                        description: description.clone(),
                        manifest: manifest_wire.clone(),
                    },
                },
                generation,
            )
        } else {
            let conflict = record_workspace_conflict(
                &mut tx,
                operation.resource_id,
                &expected_head,
                &revision_id,
                &current_head,
                current.1,
            )
            .await?;
            let generation = u64::try_from(current.1).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "workspace generation is invalid")
            })?;
            (
                "diverged",
                PrivateRevisionCommitResponse::Diverged {
                    resource_id: operation.resource_id.to_string(),
                    revision_id: hex::encode(revision_id),
                    conflict,
                },
                generation,
            )
        };
        sqlx::query(
            "UPDATE private_revision_operations SET state=$1,outcome_json=$2,updated_at=now() \
             WHERE user_id=$3 AND operation_id=$4 AND state='prepared'",
        )
        .bind(state)
        .bind(
            serde_json::to_value(&outcome)
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?,
        )
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        crate::release::enqueue_resource_wake(&mut tx, operation.resource_id, wake_generation)
            .await?;
        tx.commit().await.map_err(internal_api_error)?;

        for (_, key) in staging.values() {
            let _ = self.objects.delete(key).await;
        }
        let _ = self.drain_outbox(64).await;
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

fn revision_operation_fork_sync(
    operation: &RevisionOperationRow,
) -> Result<Option<ValidatedForkSync>, ApiError> {
    match (
        operation.fork_sync_base_revision_id.as_deref(),
        operation.fork_sync_upstream_revision_id.as_deref(),
    ) {
        (None, None) => Ok(None),
        (Some(base), Some(upstream)) => Ok(Some(ValidatedForkSync {
            expected_base: RevisionId::from_bytes(decode_32(
                base,
                "stored fork sync base revision ID",
            )?),
            upstream_revision: RevisionId::from_bytes(decode_32(
                upstream,
                "stored fork sync upstream revision ID",
            )?),
        })),
        _ => Err(ApiError::new(
            ApiErrorCode::Internal,
            "stored fork sync intent is invalid",
        )),
    }
}

async fn fetch_revision_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: Uuid,
) -> Result<Option<RevisionOperationRow>, ApiError> {
    sqlx::query_as::<_, RevisionOperationRow>(
        "SELECT request_hash,namespace_id,resource_id,expected_generation,expected_head_revision_id,revision_id,manifest_json, \
                revision_author_principal_id,fork_sync_base_revision_id,fork_sync_upstream_revision_id,historical_skill_name,state,outcome_json \
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
        "SELECT request_hash,namespace_id,resource_id,expected_generation,expected_head_revision_id,revision_id,manifest_json, \
                revision_author_principal_id,fork_sync_base_revision_id,fork_sync_upstream_revision_id,historical_skill_name,state,outcome_json \
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

async fn fetch_revision_operation_parents_pool(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    operation_id: Uuid,
) -> Result<Vec<[u8; 32]>, ApiError> {
    let rows = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT parent_revision_id FROM private_revision_operation_parents \
         WHERE user_id=$1 AND operation_id=$2 ORDER BY ordinal",
    )
    .bind(user_id)
    .bind(operation_id)
    .fetch_all(pool)
    .await
    .map_err(internal_api_error)?;
    rows.into_iter()
        .map(|row| decode_32(&row, "stored revision parent"))
        .collect()
}

fn parse_revision_parents(values: &[String]) -> Result<Vec<RevisionId>, ApiError> {
    if !(1..=2).contains(&values.len()) {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "private revisions require one or two parent_revision_ids",
        ));
    }
    let parents = values
        .iter()
        .map(|value| {
            RevisionId::from_str(value)
                .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut sorted = parents.clone();
    sorted.sort();
    if sorted != parents || sorted.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "parent_revision_ids must be unique and sorted canonically",
        ));
    }
    Ok(parents)
}

fn revision_operation_state(value: &str) -> Result<PrivateRevisionOperationState, ApiError> {
    match value {
        "prepared" => Ok(PrivateRevisionOperationState::Prepared),
        "advanced" => Ok(PrivateRevisionOperationState::Advanced),
        "diverged" => Ok(PrivateRevisionOperationState::Diverged),
        _ => Err(ApiError::new(
            ApiErrorCode::Internal,
            "stored private revision operation state is invalid",
        )),
    }
}

fn decode_revision_outcome(
    value: Option<Value>,
) -> Result<PrivateRevisionCommitResponse, ApiError> {
    serde_json::from_value(value.ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::Internal,
            "completed private revision has no stored outcome",
        )
    })?)
    .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}
