use std::collections::{BTreeMap, HashSet};

use denju_core::{
    AuthorPrincipalId, BlobId, DeterministicSkillSnapshot, OperationId, OwnedSkillEntry, Revision,
    RevisionId, build_deterministic_skill_snapshot, parse_skill_document,
    rewrite_skill_document_name, validate_declared_skill_manifest, validate_skill_snapshot,
};
use denju_wire::{
    ApiError, ApiErrorCode, PublicSkillManifest, RenameSkillRequest, RenameSkillResponse,
    RequestHash,
};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    Registry,
    identity_support::UserAuthority,
    ingest::{
        canonical_blob_key, decode_32, enforce_namespace_quota, manifest_blobs,
        persist_canonical_blobs, persist_trees,
    },
    internal_api_error,
    lifecycle::{
        RevisionPersistence, persist_revision, persist_revision_snapshot,
        record_lifecycle_operation,
    },
    release::enqueue_resource_wake,
    rename_content::{PreparedRenameExpectation, consume_prepared_rename_operation},
    team_access::authorize_resource_publish,
};

#[derive(Debug, Clone, FromRow)]
struct TeamResourceRow {
    owner_namespace_id: Uuid,
    owner: String,
    name: String,
    generation: i64,
    latest_release_version: Option<i64>,
}

#[derive(Debug, Clone, FromRow)]
struct WorkspaceRow {
    workspace_user_id: Uuid,
    generation: i64,
    revision_id: Vec<u8>,
    manifest_json: Value,
    snapshot_key: String,
}

#[derive(Debug, Clone, FromRow)]
struct ReleaseRow {
    version: i64,
    revision_id: Vec<u8>,
    manifest_json: Value,
    snapshot_key: String,
}

struct RenamedContent {
    source_revision_id: RevisionId,
    revision_id: RevisionId,
    manifest: PublicSkillManifest,
    snapshot: DeterministicSkillSnapshot,
    snapshot_key: String,
    snapshot_sha: BlobId,
    blobs: BTreeMap<BlobId, u64>,
    description: String,
}

pub(crate) async fn try_rename_team_skill(
    registry: &Registry,
    authority: &UserAuthority,
    operation_id: OperationId,
    resource_id: Uuid,
    request: &RenameSkillRequest,
    request_hash: RequestHash,
) -> Result<Option<RenameSkillResponse>, ApiError> {
    let mut tx = registry.begin_actor_tx(authority.user_id).await?;
    let owner_kind = sqlx::query_scalar::<_, String>(
        "SELECT n.kind FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
         WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL",
    )
    .bind(resource_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "owned skill not found"))?;
    tx.commit().await.map_err(internal_api_error)?;
    if owner_kind != "team" {
        return Ok(None);
    }
    rename_team_skill(
        registry,
        authority,
        operation_id,
        resource_id,
        request,
        request_hash,
    )
    .await
    .map(Some)
}

async fn rename_team_skill(
    registry: &Registry,
    authority: &UserAuthority,
    operation_id: OperationId,
    resource_id: Uuid,
    request: &RenameSkillRequest,
    request_hash: RequestHash,
) -> Result<RenameSkillResponse, ApiError> {
    let mut authority_tx = registry.begin_actor_tx(authority.user_id).await?;
    let resource_authority =
        authorize_resource_publish(&mut authority_tx, authority, resource_id).await?;
    if !resource_authority.is_team {
        return Err(ApiError::new(
            ApiErrorCode::Internal,
            "team rename was selected for a non-team skill",
        ));
    }
    let source = load_team_resource(&mut authority_tx, resource_id).await?;
    if source.owner_namespace_id != resource_authority.namespace_id {
        return Err(ApiError::new(
            ApiErrorCode::Unauthorized,
            "team skill is unavailable",
        ));
    }
    ensure_resource_generation(source.generation, request.expected_generation)?;
    if source.name == request.new_name {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "the skill already has that name",
        ));
    }

    authority_tx.commit().await.map_err(internal_api_error)?;
    // Team rename is the one lifecycle operation that must mechanically rewrite every
    // maintainer's private workspace while still never exposing one maintainer's draft through
    // the ordinary app/RLS read surface. Re-authorize inside the isolated worker transaction,
    // then use that trusted service boundary only for the cross-workspace read/write itself.
    let mut worker_read_tx = registry.begin_worker_tx().await?;
    let worker_authority =
        authorize_resource_publish(&mut worker_read_tx, authority, resource_id).await?;
    if !worker_authority.is_team || worker_authority.namespace_id != source.owner_namespace_id {
        return Err(ApiError::new(
            ApiErrorCode::Unauthorized,
            "team skill is unavailable",
        ));
    }
    let workspaces = load_workspaces(&mut worker_read_tx, resource_id).await?;
    worker_read_tx.commit().await.map_err(internal_api_error)?;
    let actor_index = workspaces
        .iter()
        .position(|workspace| workspace.workspace_user_id == authority.user_id)
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "team workspace not found"))?;
    let prepared_operation_id = request
        .prepared_revision_operation_id
        .as_deref()
        .map(|value| {
            value
                .parse::<OperationId>()
                .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
        })
        .transpose()?;
    let prepared = if let Some(prepared_operation_id) = prepared_operation_id {
        let actor = &workspaces[actor_index];
        Some(
            registry
                .verified_prepared_rename_content(
                    authority,
                    PreparedRenameExpectation {
                        operation_id: prepared_operation_id.as_uuid(),
                        resource_id,
                        namespace_id: source.owner_namespace_id,
                        generation: actor.generation,
                        parent_revision_id: &actor.revision_id,
                        current_name: &source.name,
                    },
                )
                .await?,
        )
    } else {
        None
    };

    let author = AuthorPrincipalId::from_uuid(authority.author_principal_id)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    let mut renamed_workspaces = Vec::with_capacity(workspaces.len());
    for (index, workspace) in workspaces.iter().enumerate() {
        let entries = if index == actor_index {
            if let Some(prepared) = prepared.as_ref() {
                prepared.entries.clone()
            } else {
                load_stored_entries(
                    registry,
                    &source.name,
                    &workspace.manifest_json,
                    &workspace.snapshot_key,
                )
                .await?
            }
        } else {
            load_stored_entries(
                registry,
                &source.name,
                &workspace.manifest_json,
                &workspace.snapshot_key,
            )
            .await?
        };
        renamed_workspaces.push(
            build_renamed_content(
                registry,
                &source.name,
                &request.new_name,
                entries,
                decode_revision(&workspace.revision_id)?,
                author,
                operation_id,
            )
            .await?,
        );
    }

    let release = match source.latest_release_version {
        Some(version) => Some(load_release(registry, resource_id, version).await?),
        None => None,
    };
    let renamed_release = if let Some(release) = release.as_ref() {
        let release_revision = decode_revision(&release.revision_id)?;
        let entries = load_stored_entries(
            registry,
            &source.name,
            &release.manifest_json,
            &release.snapshot_key,
        )
        .await?;
        Some(
            build_renamed_content(
                registry,
                &source.name,
                &request.new_name,
                entries,
                release_revision,
                author,
                operation_id,
            )
            .await?,
        )
    } else {
        None
    };

    let mut tx = registry.begin_worker_tx().await?;
    let current_authority = authorize_resource_publish(&mut tx, authority, resource_id).await?;
    if !current_authority.is_team || current_authority.namespace_id != source.owner_namespace_id {
        return Err(ApiError::new(
            ApiErrorCode::Unauthorized,
            "team skill is unavailable",
        ));
    }
    let locked = sqlx::query_as::<_, TeamResourceRow>(
        "SELECT r.owner_namespace_id,n.slug AS owner,r.slug AS name,r.generation,r.latest_release_version \
         FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id AND n.kind='team' \
         WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL FOR UPDATE OF r",
    )
    .bind(resource_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "team skill not found"))?;
    if locked.owner_namespace_id != source.owner_namespace_id
        || locked.name != source.name
        || locked.generation != source.generation
        || locked.latest_release_version != source.latest_release_version
    {
        return Err(resource_generation_conflict(locked.generation));
    }
    let unresolved_conflicts = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM skill_workspace_conflicts \
         WHERE resource_id=$1 AND resolved_at IS NULL)",
    )
    .bind(resource_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(internal_api_error)?;
    if unresolved_conflicts {
        return Err(ApiError::new(
            ApiErrorCode::GenerationConflict,
            "team skill has an unresolved maintainer workspace conflict; resolve it before renaming",
        ));
    }
    let current_workspaces = sqlx::query_as::<_, WorkspaceRow>(
        "SELECT workspace_user_id,generation,revision_id,manifest_json,snapshot_key \
         FROM skill_private_workspaces WHERE resource_id=$1 \
         ORDER BY workspace_user_id FOR UPDATE",
    )
    .bind(resource_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(internal_api_error)?;
    ensure_workspaces_unchanged(&workspaces, &current_workspaces)?;

    let collision = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM resources WHERE owner_namespace_id=$1 AND kind='skill' \
         AND slug=$2 AND deleted_at IS NULL AND id<>$3)",
    )
    .bind(source.owner_namespace_id)
    .bind(&request.new_name)
    .bind(resource_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(internal_api_error)?;
    if collision {
        return Err(ApiError::new(
            ApiErrorCode::GenerationConflict,
            format!("@{}/{} already exists", source.owner, request.new_name),
        ));
    }
    sqlx::query(
        "DELETE FROM resource_redirects WHERE namespace_id=$1 AND kind='skill' AND old_slug=$2",
    )
    .bind(source.owner_namespace_id)
    .bind(&request.new_name)
    .execute(&mut *tx)
    .await
    .map_err(internal_api_error)?;

    let mut persisted = HashSet::<[u8; 32]>::new();
    for content in renamed_workspaces.iter().chain(renamed_release.iter()) {
        if !persisted.insert(*content.revision_id.as_bytes()) {
            continue;
        }
        persist_content(
            registry,
            &mut tx,
            source.owner_namespace_id,
            resource_id,
            authority.author_principal_id,
            operation_id,
            content,
        )
        .await?;
    }

    for (workspace, content) in workspaces.iter().zip(&renamed_workspaces) {
        let next_workspace_generation = workspace.generation.checked_add(1).ok_or_else(|| {
            ApiError::new(ApiErrorCode::Internal, "workspace generation overflow")
        })?;
        let updated = sqlx::query(
            "UPDATE skill_private_workspaces SET description=$1,revision_id=$2,generation=$3,manifest_json=$4, \
             snapshot_key=$5,snapshot_sha256=$6,snapshot_size=$7,updated_at=now() \
             WHERE resource_id=$8 AND workspace_user_id=$9 AND generation=$10 AND revision_id=$11",
        )
        .bind(&content.description)
        .bind(content.revision_id.as_bytes().as_slice())
        .bind(next_workspace_generation)
        .bind(serde_json::to_value(&content.manifest).map_err(serialization_api_error)?)
        .bind(&content.snapshot_key)
        .bind(content.snapshot_sha.as_bytes().as_slice())
        .bind(snapshot_size_i64(&content.snapshot)?)
        .bind(resource_id)
        .bind(workspace.workspace_user_id)
        .bind(workspace.generation)
        .bind(workspace.revision_id.as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .rows_affected();
        if updated != 1 {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                "a team workspace advanced while the rename was being prepared",
            ));
        }
    }

    let release_version = if let Some(release) = release.as_ref() {
        let version = release
            .version
            .checked_add(1)
            .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "release version overflow"))?;
        let content = renamed_release
            .as_ref()
            .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "renamed release disappeared"))?;
        sqlx::query(
            "INSERT INTO skill_releases \
             (resource_id,version,revision_id,root_tree_id,manifest_json,snapshot_key,snapshot_sha256,snapshot_size,author_principal_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(resource_id)
        .bind(version)
        .bind(content.revision_id.as_bytes().as_slice())
        .bind(content.snapshot.manifest().root_tree().as_bytes().as_slice())
        .bind(serde_json::to_value(&content.manifest).map_err(serialization_api_error)?)
        .bind(&content.snapshot_key)
        .bind(content.snapshot_sha.as_bytes().as_slice())
        .bind(snapshot_size_i64(&content.snapshot)?)
        .bind(authority.author_principal_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        Some(version)
    } else {
        None
    };

    let next_resource_generation = source
        .generation
        .checked_add(1)
        .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "resource generation overflow"))?;
    sqlx::query("UPDATE resources SET slug=$1,generation=$2,latest_release_version=$3 WHERE id=$4")
        .bind(&request.new_name)
        .bind(next_resource_generation)
        .bind(release_version.or(source.latest_release_version))
        .bind(resource_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
    sqlx::query(
        "INSERT INTO resource_redirects (namespace_id,kind,old_slug,target_resource_id) \
         VALUES ($1,'skill',$2,$3) ON CONFLICT(namespace_id,kind,old_slug) \
         DO UPDATE SET target_resource_id=excluded.target_resource_id,created_at=now()",
    )
    .bind(source.owner_namespace_id)
    .bind(&source.name)
    .bind(resource_id)
    .execute(&mut *tx)
    .await
    .map_err(internal_api_error)?;

    let actor_revision = renamed_workspaces[actor_index].revision_id;
    let outcome = RenameSkillResponse {
        resource_id: resource_id.to_string(),
        old_locator: format!("@{}/{}", source.owner, source.name),
        locator: format!("@{}/{}", source.owner, request.new_name),
        generation: generation_u64(next_resource_generation)?,
        revision_id: actor_revision.to_string(),
        release_version: release_version.map(generation_u64).transpose()?,
    };
    record_lifecycle_operation(
        &mut tx,
        authority.user_id,
        operation_id,
        request_hash,
        resource_id,
        "rename",
        &outcome,
    )
    .await?;
    if let Some(prepared_operation_id) = prepared_operation_id {
        consume_prepared_rename_operation(
            &mut tx,
            authority.user_id,
            prepared_operation_id.as_uuid(),
            resource_id,
        )
        .await?;
    }
    enqueue_resource_wake(
        &mut tx,
        resource_id,
        generation_u64(next_resource_generation)?,
    )
    .await?;
    tx.commit().await.map_err(internal_api_error)?;
    if let Some(prepared) = prepared {
        for key in prepared.staging_keys {
            let _ = registry.objects.delete(&key).await;
        }
    }
    let _ = registry.drain_outbox(64).await;
    Ok(outcome)
}

async fn load_team_resource(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
) -> Result<TeamResourceRow, ApiError> {
    sqlx::query_as::<_, TeamResourceRow>(
        "SELECT r.owner_namespace_id,n.slug AS owner,r.slug AS name,r.generation,r.latest_release_version \
         FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id AND n.kind='team' \
         WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL",
    )
    .bind(resource_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "team skill not found"))
}

async fn load_workspaces(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
) -> Result<Vec<WorkspaceRow>, ApiError> {
    sqlx::query_as::<_, WorkspaceRow>(
        "SELECT workspace_user_id,generation,revision_id,manifest_json,snapshot_key \
         FROM skill_private_workspaces WHERE resource_id=$1 ORDER BY workspace_user_id",
    )
    .bind(resource_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal_api_error)
}

async fn load_release(
    registry: &Registry,
    resource_id: Uuid,
    version: i64,
) -> Result<ReleaseRow, ApiError> {
    sqlx::query_as::<_, ReleaseRow>(
        "SELECT version,revision_id,manifest_json,snapshot_key FROM skill_releases \
         WHERE resource_id=$1 AND version=$2",
    )
    .bind(resource_id)
    .bind(version)
    .fetch_optional(&registry.pool)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "latest team release is missing"))
}

async fn load_stored_entries(
    registry: &Registry,
    current_name: &str,
    manifest_json: &Value,
    snapshot_key: &str,
) -> Result<Vec<OwnedSkillEntry>, ApiError> {
    let wire: PublicSkillManifest = serde_json::from_value(manifest_json.clone())
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    let manifest = wire
        .to_core()
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error))?;
    let bytes = registry
        .objects
        .get(snapshot_key)
        .await
        .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
    validate_skill_snapshot(current_name, &manifest, &bytes)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}

async fn build_renamed_content(
    registry: &Registry,
    current_name: &str,
    new_name: &str,
    mut entries: Vec<OwnedSkillEntry>,
    parent: RevisionId,
    author: AuthorPrincipalId,
    operation_id: OperationId,
) -> Result<RenamedContent, ApiError> {
    let skill_md = entries
        .iter_mut()
        .find_map(|entry| match entry {
            OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => Some(bytes),
            _ => None,
        })
        .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "stored skill has no SKILL.md"))?;
    *skill_md = rewrite_skill_document_name(current_name, skill_md, new_name)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    let description = parse_skill_document(new_name, skill_md)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?
        .frontmatter()
        .description()
        .to_owned();
    let snapshot = build_deterministic_skill_snapshot(new_name, &entries)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    let revision = Revision::new(
        snapshot.manifest().root_tree(),
        vec![parent],
        author,
        operation_id,
    )
    .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    let revision_id = revision.id();
    let snapshot_sha = BlobId::hash(snapshot.bytes());
    let snapshot_key = format!("snapshots/sha256/{snapshot_sha}.tar.zst");
    for entry in &entries {
        if let OwnedSkillEntry::File { bytes, .. } = entry {
            registry
                .objects
                .put(&canonical_blob_key(BlobId::hash(bytes)), bytes)
                .await
                .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
        }
    }
    registry
        .objects
        .put(&snapshot_key, snapshot.bytes())
        .await
        .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
    let blobs = manifest_blobs(snapshot.manifest())?;
    let manifest = PublicSkillManifest::from_core(snapshot.manifest());
    Ok(RenamedContent {
        source_revision_id: parent,
        revision_id,
        manifest,
        snapshot,
        snapshot_key,
        snapshot_sha,
        blobs,
        description,
    })
}

async fn persist_content(
    registry: &Registry,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    namespace_id: Uuid,
    resource_id: Uuid,
    author_principal_id: Uuid,
    operation_id: OperationId,
    content: &RenamedContent,
) -> Result<(), ApiError> {
    let trees = validate_declared_skill_manifest(content.snapshot.manifest())
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    enforce_namespace_quota(registry, tx, namespace_id, &content.blobs).await?;
    persist_canonical_blobs(tx, &content.blobs).await?;
    persist_trees(tx, &trees).await?;
    persist_revision(
        tx,
        RevisionPersistence {
            revision_id: content.revision_id,
            root_tree: content.snapshot.manifest().root_tree(),
            author: author_principal_id,
            operation_id,
            parent: Some(content.source_revision_id),
            blobs: &content.blobs,
            resource_id,
            namespace_id,
        },
    )
    .await?;
    persist_revision_snapshot(
        tx,
        resource_id,
        content.revision_id,
        &content.manifest,
        &content.snapshot_key,
        content.snapshot_sha,
        content.snapshot.bytes().len(),
    )
    .await
}

fn ensure_workspaces_unchanged(
    before: &[WorkspaceRow],
    after: &[WorkspaceRow],
) -> Result<(), ApiError> {
    let unchanged = before.len() == after.len()
        && before.iter().zip(after).all(|(before, after)| {
            before.workspace_user_id == after.workspace_user_id
                && before.generation == after.generation
                && before.revision_id == after.revision_id
        });
    if unchanged {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::GenerationConflict,
            "a team workspace advanced while the rename was being prepared",
        ))
    }
}

fn decode_revision(bytes: &[u8]) -> Result<RevisionId, ApiError> {
    Ok(RevisionId::from_bytes(decode_32(
        bytes,
        "stored revision ID",
    )?))
}

fn ensure_resource_generation(current: i64, expected: u64) -> Result<(), ApiError> {
    let expected = i64::try_from(expected).map_err(|_| {
        ApiError::new(
            ApiErrorCode::InvalidRequest,
            "generation exceeds database range",
        )
    })?;
    if current == expected {
        Ok(())
    } else {
        Err(resource_generation_conflict(current))
    }
}

fn resource_generation_conflict(current: i64) -> ApiError {
    ApiError::new(
        ApiErrorCode::GenerationConflict,
        format!("resource advanced to generation {current}"),
    )
}

fn generation_u64(value: i64) -> Result<u64, ApiError> {
    u64::try_from(value)
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored generation is invalid"))
}

fn snapshot_size_i64(snapshot: &DeterministicSkillSnapshot) -> Result<i64, ApiError> {
    i64::try_from(snapshot.bytes().len()).map_err(|_| {
        ApiError::new(
            ApiErrorCode::Internal,
            "snapshot size exceeds database range",
        )
    })
}

fn serialization_api_error(error: serde_json::Error) -> ApiError {
    ApiError::new(ApiErrorCode::Internal, error.to_string())
}
