use std::{collections::BTreeMap, str::FromStr};

use denju_core::{
    AuthorPrincipalId, BlobId, OperationId, ResourceId, ResourceLocator, Revision, RevisionId,
    build_deterministic_skill_snapshot, parse_skill_document, rewrite_skill_document_name,
    validate_declared_skill_manifest, validate_skill_name, validate_skill_snapshot,
};
use denju_wire::{
    ApiError, ApiErrorCode, DeleteSkillResponse, DeprecateSkillRequest, DeprecateSkillResponse,
    PublicSkillManifest, RenameSkillRequest, RenameSkillResponse, RequestHash,
    ResourceLifecycleRequest, UnpublishSkillResponse, delete_skill_request_hash,
    deprecate_skill_request_hash, rename_skill_request_hash, unpublish_skill_request_hash,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{FromRow, Row};
use uuid::Uuid;

use crate::{
    Registry,
    ingest::{
        decode_32, enforce_namespace_quota, manifest_blobs, persist_canonical_blobs, persist_trees,
    },
    internal_api_error,
    release::enqueue_resource_wake,
    rename_content::consume_prepared_rename_operation,
};

#[derive(Debug, FromRow)]
struct RenameSourceRow {
    owner_namespace_id: Uuid,
    owner: String,
    name: String,
    generation: i64,
    revision_id: Vec<u8>,
    manifest_json: Value,
    snapshot_key: String,
}

#[derive(Debug, FromRow)]
pub(crate) struct LockedResourceRow {
    pub(crate) owner_namespace_id: Uuid,
    pub(crate) owner: String,
    pub(crate) name: String,
    pub(crate) visibility: String,
    pub(crate) generation: i64,
    pub(crate) latest_release_version: Option<i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedSkillLocator {
    pub resource_id: Uuid,
    pub owner_namespace_id: Uuid,
    pub owner: String,
    pub name: String,
}

impl Registry {
    pub(crate) async fn resolve_active_skill_locator(
        &self,
        locator: &ResourceLocator,
    ) -> Result<ResolvedSkillLocator, ApiError> {
        let active = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
            "SELECT r.id,r.owner_namespace_id,n.slug,r.slug FROM resources r \
             JOIN namespaces n ON n.id=r.owner_namespace_id \
             WHERE n.slug=$1 AND r.kind='skill' AND r.slug=$2 AND r.deleted_at IS NULL",
        )
        .bind(locator.owner())
        .bind(locator.name())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?;
        if let Some((resource_id, owner_namespace_id, owner, name)) = active {
            return Ok(ResolvedSkillLocator {
                resource_id,
                owner_namespace_id,
                owner,
                name,
            });
        }
        sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
            "SELECT target.id,target.owner_namespace_id,target_owner.slug,target.slug \
             FROM resource_redirects rr JOIN namespaces old_owner ON old_owner.id=rr.namespace_id \
             JOIN resources target ON target.id=rr.target_resource_id AND target.deleted_at IS NULL \
             JOIN namespaces target_owner ON target_owner.id=target.owner_namespace_id \
             WHERE old_owner.slug=$1 AND rr.kind='skill' AND rr.old_slug=$2",
        )
        .bind(locator.owner())
        .bind(locator.name())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .map(|(resource_id, owner_namespace_id, owner, name)| ResolvedSkillLocator {
            resource_id,
            owner_namespace_id,
            owner,
            name,
        })
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "skill not found"))
    }

    pub async fn rename_skill(
        &self,
        bearer: &str,
        request: &RenameSkillRequest,
    ) -> Result<RenameSkillResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let resource_id = ResourceId::from_str(&request.resource_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        validate_skill_name(&request.new_name)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let request_hash = validate_lifecycle_hash(
            &request.request_hash,
            rename_skill_request_hash(
                &request.operation_id,
                &request.resource_id,
                request.expected_generation,
                &request.new_name,
                request.prepared_revision_operation_id.as_deref(),
            ),
        )?;
        if let Some(outcome) = self
            .replay_lifecycle_operation::<RenameSkillResponse>(
                authority.user_id,
                operation_id,
                request_hash,
                "rename",
                resource_id.as_uuid(),
            )
            .await?
        {
            return Ok(outcome);
        }

        let source = sqlx::query_as::<_, RenameSourceRow>(
            "SELECT r.owner_namespace_id,n.slug AS owner,r.slug AS name,r.generation, \
                    w.revision_id,w.manifest_json,w.snapshot_key \
             FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
             JOIN skill_private_workspaces w ON w.resource_id=r.id \
             WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL",
        )
        .bind(resource_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "owned skill not found"))?;
        if source.owner_namespace_id != authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "owned skill is unavailable",
            ));
        }
        let expected_generation = generation_i64(request.expected_generation)?;
        if source.generation != expected_generation {
            return Err(generation_conflict(source.generation));
        }
        if source.name == request.new_name {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "the skill already has that name",
            ));
        }

        let prepared_operation_id = request
            .prepared_revision_operation_id
            .as_deref()
            .map(|value| {
                OperationId::from_str(value)
                    .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
            })
            .transpose()?;
        let (mut entries, staging_keys) = if let Some(prepared_operation_id) = prepared_operation_id
        {
            let prepared = self
                .verified_prepared_rename_content(
                    &authority,
                    prepared_operation_id.as_uuid(),
                    resource_id.as_uuid(),
                    expected_generation,
                    &source.revision_id,
                    &source.name,
                )
                .await?;
            (prepared.entries, prepared.staging_keys)
        } else {
            let old_manifest_wire: PublicSkillManifest =
                serde_json::from_value(source.manifest_json)
                    .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
            let old_manifest = old_manifest_wire
                .to_core()
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error))?;
            let snapshot_bytes = self
                .objects
                .get(&source.snapshot_key)
                .await
                .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
            let entries = validate_skill_snapshot(&source.name, &old_manifest, &snapshot_bytes)
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
            (entries, Vec::new())
        };
        let skill_md = entries
            .iter_mut()
            .find_map(|entry| match entry {
                denju_core::OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                    Some(bytes)
                }
                _ => None,
            })
            .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "stored skill has no SKILL.md"))?;
        *skill_md = rewrite_skill_document_name(&source.name, skill_md, &request.new_name)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let new_snapshot = build_deterministic_skill_snapshot(&request.new_name, &entries)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let new_manifest = new_snapshot.manifest().clone();
        let new_manifest_wire = PublicSkillManifest::from_core(&new_manifest);
        let trees = validate_declared_skill_manifest(&new_manifest)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let blobs = manifest_blobs(&new_manifest)?;
        let document = entries
            .iter()
            .find_map(|entry| match entry {
                denju_core::OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                    Some(bytes.as_slice())
                }
                _ => None,
            })
            .ok_or_else(|| {
                ApiError::new(ApiErrorCode::Internal, "renamed skill has no SKILL.md")
            })?;
        let description = parse_skill_document(&request.new_name, document)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?
            .frontmatter()
            .description()
            .to_owned();
        let parent = RevisionId::from_bytes(decode_32(&source.revision_id, "stored revision ID")?);
        let author = AuthorPrincipalId::from_uuid(authority.author_principal_id)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let revision = Revision::new(new_manifest.root_tree(), vec![parent], author, operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let revision_id = revision.id();
        let snapshot_sha = BlobId::hash(new_snapshot.bytes());
        let snapshot_key = format!("snapshots/sha256/{snapshot_sha}.tar.zst");

        for entry in &entries {
            if let denju_core::OwnedSkillEntry::File { bytes, .. } = entry {
                let blob = BlobId::hash(bytes);
                self.objects
                    .put(&crate::ingest::canonical_blob_key(blob), bytes)
                    .await
                    .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
            }
        }
        self.objects
            .put(&snapshot_key, new_snapshot.bytes())
            .await
            .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let locked = lock_active_owned_skill(&mut tx, resource_id.as_uuid()).await?;
        ensure_owner(&locked, authority.namespace_id)?;
        if locked.generation != expected_generation || locked.name != source.name {
            return Err(generation_conflict(locked.generation));
        }
        let collision = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE owner_namespace_id=$1 AND kind='skill' \
             AND slug=$2 AND deleted_at IS NULL AND id<>$3)",
        )
        .bind(authority.namespace_id)
        .bind(&request.new_name)
        .bind(resource_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if collision {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!(
                    "@{}/{} already exists",
                    authority.namespace_slug, request.new_name
                ),
            ));
        }
        sqlx::query(
            "DELETE FROM resource_redirects WHERE namespace_id=$1 AND kind='skill' AND old_slug=$2",
        )
        .bind(authority.namespace_id)
        .bind(&request.new_name)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        enforce_namespace_quota(self, &mut tx, authority.namespace_id, &blobs).await?;
        persist_canonical_blobs(&mut tx, &blobs).await?;
        persist_trees(&mut tx, &trees).await?;
        persist_revision(
            &mut tx,
            RevisionPersistence {
                revision_id,
                root_tree: new_manifest.root_tree(),
                author: authority.author_principal_id,
                operation_id,
                parent: Some(parent),
                blobs: &blobs,
                resource_id: resource_id.as_uuid(),
                namespace_id: authority.namespace_id,
            },
        )
        .await?;
        persist_revision_snapshot(
            &mut tx,
            resource_id.as_uuid(),
            revision_id,
            &new_manifest_wire,
            &snapshot_key,
            snapshot_sha,
            new_snapshot.bytes().len(),
        )
        .await?;

        let next_generation = next_generation(locked.generation)?;
        let release_version = if locked.visibility == "public" {
            let latest = locked.latest_release_version.ok_or_else(|| {
                ApiError::new(
                    ApiErrorCode::Internal,
                    "public skill is missing its latest release",
                )
            })?;
            let version = latest
                .checked_add(1)
                .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "release version overflow"))?;
            sqlx::query(
                "INSERT INTO skill_releases \
                 (resource_id,version,revision_id,root_tree_id,manifest_json,snapshot_key,snapshot_sha256,snapshot_size,author_principal_id) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
            )
            .bind(resource_id.as_uuid())
            .bind(version)
            .bind(revision_id.as_bytes().as_slice())
            .bind(new_manifest.root_tree().as_bytes().as_slice())
            .bind(serde_json::to_value(&new_manifest_wire).map_err(internal_serialization_error)?)
            .bind(&snapshot_key)
            .bind(snapshot_sha.as_bytes().as_slice())
            .bind(i64::try_from(new_snapshot.bytes().len()).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "snapshot size exceeds database range")
            })?)
            .bind(authority.author_principal_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            Some(version)
        } else {
            None
        };
        let latest_release_version = release_version.or(locked.latest_release_version);
        sqlx::query(
            "UPDATE resources SET slug=$1,description=$2,generation=$3,latest_release_version=$4 WHERE id=$5",
        )
        .bind(&request.new_name)
        .bind(&description)
        .bind(next_generation)
        .bind(latest_release_version)
        .bind(resource_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "UPDATE skill_private_workspaces SET revision_id=$1,generation=$2,manifest_json=$3, \
             snapshot_key=$4,snapshot_sha256=$5,snapshot_size=$6,updated_at=now() WHERE resource_id=$7",
        )
        .bind(revision_id.as_bytes().as_slice())
        .bind(next_generation)
        .bind(serde_json::to_value(&new_manifest_wire).map_err(internal_serialization_error)?)
        .bind(&snapshot_key)
        .bind(snapshot_sha.as_bytes().as_slice())
        .bind(i64::try_from(new_snapshot.bytes().len()).map_err(|_| {
            ApiError::new(ApiErrorCode::Internal, "snapshot size exceeds database range")
        })?)
        .bind(resource_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO resource_redirects (namespace_id,kind,old_slug,target_resource_id) \
             VALUES ($1,'skill',$2,$3) ON CONFLICT(namespace_id,kind,old_slug) \
             DO UPDATE SET target_resource_id=excluded.target_resource_id,created_at=now()",
        )
        .bind(authority.namespace_id)
        .bind(&source.name)
        .bind(resource_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let generation = generation_u64(next_generation)?;
        let outcome = RenameSkillResponse {
            resource_id: resource_id.to_string(),
            old_locator: format!("@{}/{}", source.owner, source.name),
            locator: format!("@{}/{}", source.owner, request.new_name),
            generation,
            revision_id: revision_id.to_string(),
            release_version: release_version.map(generation_u64).transpose()?,
        };
        record_lifecycle_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            request_hash,
            resource_id.as_uuid(),
            "rename",
            &outcome,
        )
        .await?;
        if let Some(prepared_operation_id) = prepared_operation_id {
            consume_prepared_rename_operation(
                &mut tx,
                authority.user_id,
                prepared_operation_id.as_uuid(),
                resource_id.as_uuid(),
            )
            .await?;
        }
        enqueue_resource_wake(&mut tx, resource_id.as_uuid(), generation).await?;
        tx.commit().await.map_err(internal_api_error)?;
        for key in staging_keys {
            let _ = self.objects.delete(&key).await;
        }
        let _ = self.drain_outbox(64).await;
        Ok(outcome)
    }

    pub async fn unpublish_skill(
        &self,
        bearer: &str,
        request: &ResourceLifecycleRequest,
    ) -> Result<UnpublishSkillResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let (operation_id, resource_id, request_hash) =
            validate_resource_lifecycle_request(request, unpublish_skill_request_hash)?;
        if let Some(outcome) = self
            .replay_lifecycle_operation::<UnpublishSkillResponse>(
                authority.user_id,
                operation_id,
                request_hash,
                "unpublish",
                resource_id.as_uuid(),
            )
            .await?
        {
            return Ok(outcome);
        }
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let locked = lock_active_owned_skill(&mut tx, resource_id.as_uuid()).await?;
        ensure_owner(&locked, authority.namespace_id)?;
        ensure_generation(&locked, request.expected_generation)?;
        if locked.latest_release_version.is_none() {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "the skill has never been published",
            ));
        }
        if locked.visibility != "public" {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "the skill is already unpublished",
            ));
        }
        let next = next_generation(locked.generation)?;
        sqlx::query("UPDATE resources SET visibility='private',generation=$1 WHERE id=$2")
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
        let outcome = UnpublishSkillResponse {
            resource_id: resource_id.to_string(),
            locator: format!("@{}/{}", locked.owner, locked.name),
            generation,
            unpublished: true,
        };
        record_lifecycle_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            request_hash,
            resource_id.as_uuid(),
            "unpublish",
            &outcome,
        )
        .await?;
        enqueue_resource_wake(&mut tx, resource_id.as_uuid(), generation).await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.drain_outbox(64).await;
        Ok(outcome)
    }

    pub async fn delete_skill(
        &self,
        bearer: &str,
        request: &ResourceLifecycleRequest,
    ) -> Result<DeleteSkillResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let (operation_id, resource_id, request_hash) =
            validate_resource_lifecycle_request(request, delete_skill_request_hash)?;
        if let Some(outcome) = self
            .replay_lifecycle_operation::<DeleteSkillResponse>(
                authority.user_id,
                operation_id,
                request_hash,
                "delete",
                resource_id.as_uuid(),
            )
            .await?
        {
            return Ok(outcome);
        }
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let locked = lock_active_owned_skill(&mut tx, resource_id.as_uuid()).await?;
        ensure_owner(&locked, authority.namespace_id)?;
        ensure_generation(&locked, request.expected_generation)?;
        let next = next_generation(locked.generation)?;
        sqlx::query(
            "UPDATE resources SET visibility='private',generation=$1,deleted_at=now(), \
             deleted_owner_slug=$2,tombstone_release_version=latest_release_version WHERE id=$3",
        )
        .bind(next)
        .bind(&locked.owner)
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
        sqlx::query("DELETE FROM resource_redirects WHERE target_resource_id=$1")
            .bind(resource_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        let generation = generation_u64(next)?;
        let outcome = DeleteSkillResponse {
            resource_id: resource_id.to_string(),
            locator: format!("@{}/{}", locked.owner, locked.name),
            generation,
            deleted: true,
            final_release_version: locked
                .latest_release_version
                .map(generation_u64)
                .transpose()?,
        };
        record_lifecycle_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            request_hash,
            resource_id.as_uuid(),
            "delete",
            &outcome,
        )
        .await?;
        enqueue_resource_wake(&mut tx, resource_id.as_uuid(), generation).await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.drain_outbox(64).await;
        Ok(outcome)
    }

    pub async fn deprecate_skill(
        &self,
        bearer: &str,
        request: &DeprecateSkillRequest,
    ) -> Result<DeprecateSkillResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let resource_id = ResourceId::from_str(&request.resource_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let replacement_id = request
            .replacement_resource_id
            .as_deref()
            .map(ResourceId::from_str)
            .transpose()
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        if !request.deprecated && replacement_id.is_some() {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "an active skill cannot keep a deprecation replacement",
            ));
        }
        if replacement_id == Some(resource_id) {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "a skill cannot deprecate itself in favor of itself",
            ));
        }
        let request_hash = validate_lifecycle_hash(
            &request.request_hash,
            deprecate_skill_request_hash(
                &request.operation_id,
                &request.resource_id,
                request.expected_generation,
                request.deprecated,
                request.replacement_resource_id.as_deref(),
            ),
        )?;
        if let Some(outcome) = self
            .replay_lifecycle_operation::<DeprecateSkillResponse>(
                authority.user_id,
                operation_id,
                request_hash,
                "deprecate",
                resource_id.as_uuid(),
            )
            .await?
        {
            return Ok(outcome);
        }
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let locked = lock_active_owned_skill(&mut tx, resource_id.as_uuid()).await?;
        ensure_owner(&locked, authority.namespace_id)?;
        ensure_generation(&locked, request.expected_generation)?;
        if locked.visibility != "public" || locked.latest_release_version.is_none() {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "only a public skill can be deprecated",
            ));
        }
        let replacement = if let Some(replacement) = replacement_id {
            sqlx::query_as::<_, (String, String)>(
                "SELECT n.slug,r.slug FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
                 WHERE r.id=$1 AND r.kind='skill' AND r.visibility='public' AND r.deleted_at IS NULL",
            )
            .bind(replacement.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_api_error)?
            .map(|(owner, name)| format!("@{owner}/{name}"))
            .ok_or_else(|| {
                ApiError::new(ApiErrorCode::NotFound, "replacement public skill not found")
            })?
            .into()
        } else {
            None
        };
        let next = next_generation(locked.generation)?;
        sqlx::query(
            "UPDATE resources SET generation=$1,deprecated_at=CASE WHEN $2 THEN now() ELSE NULL END, \
             deprecation_replacement_resource_id=$3 WHERE id=$4",
        )
        .bind(next)
        .bind(request.deprecated)
        .bind(replacement_id.map(|id| id.as_uuid()))
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
        let outcome = DeprecateSkillResponse {
            resource_id: resource_id.to_string(),
            locator: format!("@{}/{}", locked.owner, locked.name),
            generation,
            deprecated: request.deprecated,
            replacement,
        };
        record_lifecycle_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            request_hash,
            resource_id.as_uuid(),
            "deprecate",
            &outcome,
        )
        .await?;
        enqueue_resource_wake(&mut tx, resource_id.as_uuid(), generation).await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.drain_outbox(64).await;
        Ok(outcome)
    }

    pub(crate) async fn tombstone_owned_resources_for_account_delete(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        namespace_id: Uuid,
        owner_slug: &str,
    ) -> Result<Vec<(Uuid, u64)>, ApiError> {
        let rows = sqlx::query_as::<_, (Uuid, i64)>(
            "SELECT id,generation FROM resources WHERE owner_namespace_id=$1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(namespace_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal_api_error)?;
        let mut wakes = Vec::with_capacity(rows.len());
        for (resource_id, generation) in rows {
            let next = next_generation(generation)?;
            sqlx::query(
                "UPDATE resources SET visibility='private',generation=$1,deleted_at=now(),deleted_owner_slug=$2, \
                 tombstone_release_version=latest_release_version WHERE id=$3",
            )
            .bind(next)
            .bind(owner_slug)
            .bind(resource_id)
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
            sqlx::query("UPDATE skill_private_workspaces SET generation=$1 WHERE resource_id=$2")
                .bind(next)
                .bind(resource_id)
                .execute(&mut **tx)
                .await
                .map_err(internal_api_error)?;
            sqlx::query("DELETE FROM resource_redirects WHERE target_resource_id=$1")
                .bind(resource_id)
                .execute(&mut **tx)
                .await
                .map_err(internal_api_error)?;
            let generation = generation_u64(next)?;
            enqueue_resource_wake(tx, resource_id, generation).await?;
            wakes.push((resource_id, generation));
        }
        Ok(wakes)
    }

    pub(crate) async fn replay_lifecycle_operation<T: DeserializeOwned>(
        &self,
        user_id: Uuid,
        operation_id: OperationId,
        request_hash: RequestHash,
        kind: &str,
        resource_id: Uuid,
    ) -> Result<Option<T>, ApiError> {
        let row = sqlx::query(
            "SELECT request_hash,resource_id,operation_kind,outcome_json FROM skill_lifecycle_operations \
             WHERE user_id=$1 AND operation_id=$2",
        )
        .bind(user_id)
        .bind(operation_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored_hash: Vec<u8> = row.get(0);
        let stored_resource: Uuid = row.get(1);
        let stored_kind: String = row.get(2);
        if stored_hash.as_slice() != request_hash.as_bytes()
            || stored_resource != resource_id
            || stored_kind != kind
        {
            return Err(ApiError::new(
                ApiErrorCode::OperationConflict,
                "operation_id was already used with different lifecycle content",
            ));
        }
        serde_json::from_value(row.get(3))
            .map(Some)
            .map_err(internal_serialization_error)
    }
}

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

fn validate_lifecycle_hash(
    supplied: &str,
    expected: Result<RequestHash, denju_wire::RequestHashError>,
) -> Result<RequestHash, ApiError> {
    let supplied = RequestHash::from_str(supplied)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
    let expected = expected
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
    if supplied == expected {
        Ok(supplied)
    } else {
        Err(ApiError::new(
            ApiErrorCode::InvalidRequestHash,
            "request_hash does not match the canonical lifecycle payload",
        ))
    }
}

struct RevisionPersistence<'a> {
    revision_id: RevisionId,
    root_tree: denju_core::TreeId,
    author: Uuid,
    operation_id: OperationId,
    parent: Option<RevisionId>,
    blobs: &'a BTreeMap<BlobId, u64>,
    resource_id: Uuid,
    namespace_id: Uuid,
}

async fn persist_revision(
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
        sqlx::query("DELETE FROM canonical_blob_gc WHERE blob_id=$1")
            .bind(blob.as_bytes().as_slice())
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
    }
    Ok(())
}

async fn persist_revision_snapshot(
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

fn generation_i64(value: u64) -> Result<i64, ApiError> {
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

fn generation_conflict(current: i64) -> ApiError {
    ApiError::new(
        ApiErrorCode::GenerationConflict,
        format!("resource advanced to generation {current}"),
    )
}

fn internal_serialization_error(error: serde_json::Error) -> ApiError {
    ApiError::new(ApiErrorCode::Internal, error.to_string())
}
