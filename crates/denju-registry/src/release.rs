use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
    sync::atomic::Ordering,
    time::Duration,
};

use denju_core::{
    AuthorPrincipalId, OperationId, ResourceId, Revision, RevisionId, parse_skill_document,
    validate_skill_snapshot,
};
use denju_wire::{
    ApiError, ApiErrorCode, DirtyResource, PrivateRevisionResponse, PublicSkill,
    PublicSkillManifest, PublishSkillRequest, PublishSkillResponse, RequestHash,
    RestoreSkillRequest, SkillDeprecation, SkillRelease, SyncHint, SyncReconcileRequest,
    SyncReconcileResponse, publish_skill_request_hash, restore_skill_request_hash,
};
use serde_json::Value;
use sqlx::{FromRow, Row};
use uuid::Uuid;

use crate::{
    Registry, RegistryWake,
    access::{skill_access_for_user, user_can_read_revision},
    ingest::{decode_32, manifest_blobs},
    internal_api_error,
    release_validation::validate_release_metadata,
    revision_graph::{merge_base, revision_is_ancestor},
    team_access::{authorize_resource_publish, ensure_private_workspace_for_user},
    workspace_conflict::record_workspace_conflict,
};

#[derive(Debug, FromRow)]
struct ReleaseWorkspaceRow {
    owner_namespace_id: Uuid,
    owner: String,
    name: String,
    description: String,
    license: Option<String>,
    compatibility: Option<String>,
    visibility: String,
    generation: i64,
    workspace_generation: i64,
    latest_release_version: Option<i64>,
    revision_id: Vec<u8>,
    manifest_json: Value,
    snapshot_key: String,
    snapshot_sha256: Vec<u8>,
    snapshot_size: i64,
    deprecated: bool,
    replacement_id: Option<Uuid>,
    replacement_owner: Option<String>,
    replacement_name: Option<String>,
}

impl ReleaseWorkspaceRow {
    fn deprecation(&self) -> Option<SkillDeprecation> {
        self.deprecated.then(|| SkillDeprecation {
            replacement_resource_id: self.replacement_id.map(|id| id.to_string()),
            replacement_locator: self
                .replacement_owner
                .clone()
                .zip(self.replacement_name.clone())
                .map(|(owner, name)| format!("@{owner}/{name}")),
        })
    }
}

pub(crate) use crate::outbox::enqueue_resource_wake;
use crate::outbox::enqueue_skill_release_wake;

impl Registry {
    /// Lazily starts the disposable cross-instance wake bridge. Correctness never depends on
    /// this task: EOF, process replacement, connection loss, or a missed notification is healed
    /// by the client's authoritative reconcile on reconnect. The direct URL is intentionally
    /// separate from pooled request SQL because PostgreSQL LISTEN requires a session connection.
    pub fn ensure_wake_listener(&self) {
        let Some(database_url) = self.database_listen_url.clone() else {
            return;
        };
        if self
            .wake_listener_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let wake_tx = self.wake_tx.clone();
        tokio::spawn(async move {
            loop {
                let mut listener = match sqlx::postgres::PgListener::connect(&database_url).await {
                    Ok(listener) => listener,
                    Err(_) => {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                };
                if listener.listen("denju_wake").await.is_err() {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                loop {
                    let notification = match listener.recv().await {
                        Ok(notification) => notification,
                        Err(_) => break,
                    };
                    let Ok(hint) = serde_json::from_str::<SyncHint>(notification.payload()) else {
                        let _ = wake_tx.send(RegistryWake::ResyncAll);
                        continue;
                    };
                    match hint {
                        SyncHint::Dirty { resources } => {
                            for resource in resources {
                                let Ok(resource_id) = Uuid::parse_str(&resource.resource_id) else {
                                    let _ = wake_tx.send(RegistryWake::ResyncAll);
                                    continue;
                                };
                                let _ = wake_tx.send(RegistryWake::Resource {
                                    resource_id,
                                    generation: resource.generation,
                                });
                            }
                        }
                        SyncHint::ResyncAll => {
                            let _ = wake_tx.send(RegistryWake::ResyncAll);
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    }

    pub async fn publish_skill(
        &self,
        bearer: &str,
        request: &PublishSkillRequest,
    ) -> Result<PublishSkillResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let resource_id = ResourceId::from_str(&request.resource_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let expected_hash = publish_skill_request_hash(
            &request.operation_id,
            &request.resource_id,
            request.expected_generation,
            request.public,
            request.message.as_deref(),
            &request.tags,
        )
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        if supplied_hash != expected_hash {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequestHash,
                "request_hash does not match the canonical publish payload",
            ));
        }
        validate_release_metadata(request.message.as_deref(), &request.tags)?;

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(row) = sqlx::query(
            "SELECT request_hash,resource_id,outcome_json FROM skill_release_operations \
             WHERE user_id=$1 AND operation_id=$2",
        )
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        {
            let stored_hash: Vec<u8> = row.get(0);
            let stored_resource: Uuid = row.get(1);
            if stored_hash.as_slice() != supplied_hash.as_bytes()
                || stored_resource != resource_id.as_uuid()
            {
                return Err(ApiError::new(
                    ApiErrorCode::OperationConflict,
                    "operation_id was already used with different publish content",
                ));
            }
            let outcome: PublishSkillResponse = serde_json::from_value(row.get(2))
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }

        crate::pack_drain::lock_skill_resource(&mut tx, resource_id.as_uuid()).await?;
        let resource_authority =
            authorize_resource_publish(&mut tx, &authority, resource_id.as_uuid()).await?;
        let current = sqlx::query_as::<_, ReleaseWorkspaceRow>(
            "SELECT r.owner_namespace_id,n.slug AS owner,r.slug AS name,w.description,w.license,w.compatibility,r.visibility,r.generation, \
                    w.generation AS workspace_generation, \
                    r.latest_release_version,w.revision_id,w.manifest_json,w.snapshot_key,w.snapshot_sha256,w.snapshot_size, \
                    r.deprecated_at IS NOT NULL AS deprecated,replacement.id AS replacement_id, \
                    replacement_owner.slug AS replacement_owner,replacement.slug AS replacement_name \
             FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
             JOIN skill_private_workspaces w ON w.resource_id=r.id AND w.workspace_user_id=$2 \
             LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
             LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
             WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL FOR UPDATE OF r,w",
        )
        .bind(resource_id.as_uuid())
        .bind(authority.user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "owned skill not found"))?;
        if current.owner_namespace_id != resource_authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "owned skill is unavailable",
            ));
        }
        let expected_generation = i64::try_from(request.expected_generation).map_err(|_| {
            ApiError::new(
                ApiErrorCode::InvalidRequest,
                "generation exceeds database range",
            )
        })?;
        if current.workspace_generation != expected_generation {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!(
                    "private workspace advanced to generation {}",
                    current.workspace_generation
                ),
            ));
        }
        let publish_public =
            !resource_authority.is_team || current.visibility == "public" || request.public;
        if let Some(latest) = current.latest_release_version {
            let latest_revision = sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT revision_id FROM skill_releases WHERE resource_id=$1 AND version=$2",
            )
            .bind(resource_id.as_uuid())
            .bind(latest)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if latest_revision == current.revision_id
                && (current.visibility == "public" || !publish_public)
            {
                return Err(ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "the private workspace has no unpublished changes",
                ));
            }
            if latest_revision == current.revision_id && publish_public {
                let next_generation = current.generation.checked_add(1).ok_or_else(|| {
                    ApiError::new(ApiErrorCode::Internal, "resource generation overflow")
                })?;
                sqlx::query("UPDATE resources SET visibility='public',generation=$1 WHERE id=$2")
                    .bind(next_generation)
                    .bind(resource_id.as_uuid())
                    .execute(&mut *tx)
                    .await
                    .map_err(internal_api_error)?;
                let release_row = sqlx::query(
                    "SELECT sr.message,COALESCE(array_agg(srt.tag ORDER BY srt.tag) FILTER (WHERE srt.tag IS NOT NULL),'{}'::text[]) \
                     FROM skill_releases sr LEFT JOIN skill_release_tags srt \
                     ON srt.resource_id=sr.resource_id AND srt.version=sr.version \
                     WHERE sr.resource_id=$1 AND sr.version=$2 GROUP BY sr.message",
                )
                .bind(resource_id.as_uuid())
                .bind(latest)
                .fetch_one(&mut *tx)
                .await
                .map_err(internal_api_error)?;
                let generation = u64::try_from(next_generation).map_err(|_| {
                    ApiError::new(ApiErrorCode::Internal, "resource generation is invalid")
                })?;
                let release_version = u64::try_from(latest).map_err(|_| {
                    ApiError::new(ApiErrorCode::Internal, "release version is invalid")
                })?;
                let revision = decode_32(&current.revision_id, "stored revision ID")?;
                let deprecation = current.deprecation();
                let outcome = PublishSkillResponse {
                    skill: PublicSkill {
                        resource_id: resource_id.to_string(),
                        locator: format!("@{}/{}", current.owner, current.name),
                        owner: current.owner,
                        name: current.name,
                        description: current.description,
                        generation,
                        version: Some(release_version),
                        live_private: false,
                        revision_id: hex::encode(revision),
                        deprecation,
                    },
                    release: SkillRelease {
                        version: release_version,
                        revision_id: hex::encode(revision),
                        message: release_row.get(0),
                        tags: release_row.get(1),
                    },
                };
                sqlx::query(
                    "INSERT INTO skill_release_operations (user_id,operation_id,request_hash,resource_id,outcome_json) \
                     VALUES ($1,$2,$3,$4,$5)",
                )
                .bind(authority.user_id)
                .bind(operation_id.as_uuid())
                .bind(supplied_hash.as_bytes().as_slice())
                .bind(resource_id.as_uuid())
                .bind(serde_json::to_value(&outcome).map_err(|error| {
                    ApiError::new(ApiErrorCode::Internal, error.to_string())
                })?)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
                enqueue_resource_wake(&mut tx, resource_id.as_uuid(), generation).await?;
                tx.commit().await.map_err(internal_api_error)?;
                let _ = self.drain_pack_release_events(16).await;
                let _ = self.drain_outbox(64).await;
                return Ok(outcome);
            }
            if resource_authority.is_team && latest_revision != current.revision_id {
                let current_revision =
                    decode_32(&current.revision_id, "stored workspace revision ID")?;
                let latest_revision = decode_32(&latest_revision, "stored release revision ID")?;
                if !revision_is_ancestor(&mut tx, &latest_revision, &current_revision).await? {
                    let active_conflict = sqlx::query_scalar::<_, bool>(
                        "SELECT EXISTS(SELECT 1 FROM skill_workspace_conflicts \
                         WHERE resource_id=$1 AND workspace_user_id=$2 AND resolved_at IS NULL)",
                    )
                    .bind(resource_id.as_uuid())
                    .bind(authority.user_id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(internal_api_error)?;
                    if !active_conflict {
                        let base = merge_base(&mut tx, &current_revision, &latest_revision).await?;
                        record_workspace_conflict(
                            &mut tx,
                            resource_id.as_uuid(),
                            authority.user_id,
                            &base,
                            &latest_revision,
                            &current_revision,
                            current.workspace_generation,
                        )
                        .await?;
                        tx.commit().await.map_err(internal_api_error)?;
                    }
                    return Err(ApiError::new(
                        ApiErrorCode::GenerationConflict,
                        "another maintainer published first; merge the newer team release into this private workspace and retry",
                    ));
                }
            }
        }
        let version = current
            .latest_release_version
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "release version overflow"))?;
        let next_generation = current
            .generation
            .checked_add(1)
            .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "resource generation overflow"))?;
        let revision = decode_32(&current.revision_id, "stored revision ID")?;
        sqlx::query(
            "INSERT INTO skill_releases \
             (resource_id,version,revision_id,root_tree_id,manifest_json,snapshot_key,snapshot_sha256,snapshot_size,author_principal_id,message) \
             SELECT $1,$2,$3,revision.root_tree_id,$4,$5,$6,$7,$8,$9 \
             FROM revisions revision WHERE revision.revision_id=$3",
        )
        .bind(resource_id.as_uuid())
        .bind(version)
        .bind(revision.as_slice())
        .bind(current.manifest_json.clone())
        .bind(&current.snapshot_key)
        .bind(&current.snapshot_sha256)
        .bind(current.snapshot_size)
        .bind(authority.author_principal_id)
        .bind(request.message.as_deref())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        for tag in &request.tags {
            sqlx::query(
                "INSERT INTO skill_release_tags (resource_id,version,tag) VALUES ($1,$2,$3)",
            )
            .bind(resource_id.as_uuid())
            .bind(version)
            .bind(tag)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        }
        sqlx::query(
            "UPDATE resources SET visibility=$1,latest_release_version=$2,generation=$3,description=$4,license=$5,compatibility=$6 WHERE id=$7",
        )
        .bind(if publish_public { "public" } else { "private" })
        .bind(version)
        .bind(next_generation)
        .bind(&current.description)
        .bind(&current.license)
        .bind(&current.compatibility)
        .bind(resource_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let generation = u64::try_from(next_generation)
            .map_err(|_| ApiError::new(ApiErrorCode::Internal, "resource generation is invalid"))?;
        let release_version = u64::try_from(version)
            .map_err(|_| ApiError::new(ApiErrorCode::Internal, "release version is invalid"))?;
        let deprecation = current.deprecation();
        let outcome = PublishSkillResponse {
            skill: PublicSkill {
                resource_id: resource_id.to_string(),
                locator: format!("@{}/{}", current.owner, current.name),
                owner: current.owner,
                name: current.name,
                description: current.description,
                generation,
                version: Some(release_version),
                live_private: false,
                revision_id: hex::encode(revision),
                deprecation,
            },
            release: SkillRelease {
                version: release_version,
                revision_id: hex::encode(revision),
                message: request.message.clone(),
                tags: request.tags.clone(),
            },
        };
        sqlx::query(
            "INSERT INTO skill_release_operations (user_id,operation_id,request_hash,resource_id,outcome_json) \
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .bind(supplied_hash.as_bytes().as_slice())
        .bind(resource_id.as_uuid())
        .bind(serde_json::to_value(&outcome).map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        enqueue_skill_release_wake(
            &mut tx,
            resource_id.as_uuid(),
            generation,
            release_version,
            &outcome.release.revision_id,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;

        // Common-case wake delivery is bounded and request-adjacent. The authoritative
        // outbox remains committed if this attempt is interrupted.
        let _ = self.drain_pack_release_events(16).await;
        let _ = self.drain_outbox(64).await;
        Ok(outcome)
    }

    pub async fn restore_skill(
        &self,
        bearer: &str,
        request: &RestoreSkillRequest,
    ) -> Result<PrivateRevisionResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let resource_id = ResourceId::from_str(&request.resource_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let target = RevisionId::from_str(&request.target_revision_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let expected_hash = restore_skill_request_hash(
            &request.operation_id,
            &request.resource_id,
            request.expected_generation,
            &request.target_revision_id,
        )
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        if supplied_hash != expected_hash {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequestHash,
                "request_hash does not match the canonical restore payload",
            ));
        }
        if let Some(row) = sqlx::query(
            "SELECT request_hash,resource_id,outcome_json FROM skill_restore_operations WHERE user_id=$1 AND operation_id=$2",
        )
        .bind(authority.user_id)
        .bind(operation_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        {
            let stored_hash: Vec<u8> = row.get(0);
            let stored_resource: Uuid = row.get(1);
            if stored_hash.as_slice() != supplied_hash.as_bytes() || stored_resource != resource_id.as_uuid() {
                return Err(ApiError::new(ApiErrorCode::OperationConflict, "operation_id was already used with different restore content"));
            }
            return serde_json::from_value(row.get(2))
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()));
        }

        let mut authority_tx = self.pool.begin().await.map_err(internal_api_error)?;
        let resource_authority =
            authorize_resource_publish(&mut authority_tx, &authority, resource_id.as_uuid())
                .await?;
        authority_tx.commit().await.map_err(internal_api_error)?;
        let access = skill_access_for_user(
            &self.pool,
            authority.user_id,
            authority.namespace_id,
            resource_id.as_uuid(),
        )
        .await?;
        if !user_can_read_revision(
            &self.pool,
            &access,
            resource_id.as_uuid(),
            target.as_bytes(),
        )
        .await?
        {
            return Err(ApiError::new(
                ApiErrorCode::NotFound,
                "restore revision not found",
            ));
        }

        let target_row = sqlx::query(
            "SELECT rrs.manifest_json,rrs.snapshot_key,rrs.snapshot_sha256,rrs.snapshot_size,r.slug,r.owner_namespace_id \
             FROM resource_revision_snapshots rrs JOIN resources r ON r.id=rrs.resource_id \
             WHERE rrs.resource_id=$1 AND rrs.revision_id=$2 AND r.kind='skill'",
        )
        .bind(resource_id.as_uuid())
        .bind(target.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "restore revision not found"))?;
        let owner_namespace: Uuid = target_row.get(5);
        if owner_namespace != resource_authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "owned skill is unavailable",
            ));
        }
        let manifest_wire: PublicSkillManifest = serde_json::from_value(target_row.get(0))
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let manifest = manifest_wire
            .to_core()
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error))?;
        let snapshot_key: String = target_row.get(1);
        let snapshot_sha: Vec<u8> = target_row.get(2);
        let snapshot_size: i64 = target_row.get(3);
        let slug: String = target_row.get(4);
        let snapshot_bytes = self
            .objects
            .get(&snapshot_key)
            .await
            .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
        let entries = validate_skill_snapshot(&slug, &manifest, &snapshot_bytes)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let skill_md = entries
            .iter()
            .find_map(|entry| match entry {
                denju_core::OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                    Some(bytes.as_slice())
                }
                _ => None,
            })
            .ok_or_else(|| {
                ApiError::new(
                    ApiErrorCode::Internal,
                    "stored revision is missing SKILL.md",
                )
            })?;
        let document = parse_skill_document(&slug, skill_md)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let description = document.frontmatter().description().to_owned();
        let license = document.frontmatter().license().map(str::to_owned);
        let compatibility = document.frontmatter().compatibility().map(str::to_owned);

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let resource_authority =
            authorize_resource_publish(&mut tx, &authority, resource_id.as_uuid()).await?;
        if resource_authority.is_team {
            let _ = ensure_private_workspace_for_user(
                &mut tx,
                resource_id.as_uuid(),
                authority.user_id,
            )
            .await?;
        }
        let current = sqlx::query_as::<_, (Uuid, i64, i64, Vec<u8>)>(
            "SELECT r.owner_namespace_id,r.generation,w.generation,w.revision_id FROM resources r \
             JOIN skill_private_workspaces w ON w.resource_id=r.id AND w.workspace_user_id=$2 \
             WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL FOR UPDATE OF r,w",
        )
        .bind(resource_id.as_uuid())
        .bind(authority.user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "owned skill not found"))?;
        if current.0 != resource_authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "owned skill is unavailable",
            ));
        }
        let expected_generation = i64::try_from(request.expected_generation).map_err(|_| {
            ApiError::new(
                ApiErrorCode::InvalidRequest,
                "generation exceeds database range",
            )
        })?;
        if current.2 != expected_generation {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("private workspace advanced to generation {}", current.2),
            ));
        }
        if current.3.as_slice() == target.as_bytes() {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "the requested revision is already the private workspace head",
            ));
        }
        let parent = RevisionId::from_bytes(decode_32(&current.3, "stored revision ID")?);
        let author = AuthorPrincipalId::from_uuid(authority.author_principal_id)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let revision = Revision::new(manifest.root_tree(), vec![parent], author, operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let new_revision = revision.id();
        let next_workspace_generation = current.2.checked_add(1).ok_or_else(|| {
            ApiError::new(ApiErrorCode::Internal, "workspace generation overflow")
        })?;
        let next_resource_generation = if resource_authority.is_team {
            current.1
        } else {
            current.1.checked_add(1).ok_or_else(|| {
                ApiError::new(ApiErrorCode::Internal, "resource generation overflow")
            })?
        };
        sqlx::query(
            "INSERT INTO revisions (revision_id,root_tree_id,author_principal_id,operation_id) VALUES ($1,$2,$3,$4)",
        )
        .bind(new_revision.as_bytes().as_slice())
        .bind(manifest.root_tree().as_bytes().as_slice())
        .bind(authority.author_principal_id)
        .bind(operation_id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query("INSERT INTO revision_parents (revision_id,parent_revision_id,ordinal) VALUES ($1,$2,0)")
            .bind(new_revision.as_bytes().as_slice())
            .bind(parent.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        let blobs = manifest_blobs(&manifest)?;
        for blob in blobs.keys() {
            sqlx::query(
                "INSERT INTO revision_blob_reachability (revision_id,blob_id) VALUES ($1,$2)",
            )
            .bind(new_revision.as_bytes().as_slice())
            .bind(blob.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            sqlx::query(
                "UPDATE resource_blob_reachability SET reference_count=reference_count+1 WHERE resource_id=$1 AND blob_id=$2",
            ).bind(resource_id.as_uuid()).bind(blob.as_bytes().as_slice())
                .execute(&mut *tx).await.map_err(internal_api_error)?;
            sqlx::query(
                "UPDATE namespace_blob_reachability SET reference_count=reference_count+1 WHERE namespace_id=$1 AND blob_id=$2",
            ).bind(resource_authority.namespace_id).bind(blob.as_bytes().as_slice())
                .execute(&mut *tx).await.map_err(internal_api_error)?;
        }
        sqlx::query(
            "INSERT INTO resource_revision_snapshots \
             (resource_id,revision_id,manifest_json,snapshot_key,snapshot_sha256,snapshot_size) VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(resource_id.as_uuid())
        .bind(new_revision.as_bytes().as_slice())
        .bind(serde_json::to_value(&manifest_wire).map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?)
        .bind(&snapshot_key)
        .bind(&snapshot_sha)
        .bind(snapshot_size)
        .execute(&mut *tx).await.map_err(internal_api_error)?;
        if !resource_authority.is_team {
            sqlx::query(
                "UPDATE resources SET generation=$1,description=$2,license=$3,compatibility=$4 WHERE id=$5",
            )
                .bind(next_resource_generation)
                .bind(&description)
                .bind(&license)
                .bind(&compatibility)
                .bind(resource_id.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
        }
        sqlx::query(
            "UPDATE skill_private_workspaces SET revision_id=$1,generation=$2,description=$3,license=$4,compatibility=$5,manifest_json=$6,snapshot_key=$7,snapshot_sha256=$8,snapshot_size=$9,updated_at=now() \
             WHERE resource_id=$10 AND workspace_user_id=$11",
        )
        .bind(new_revision.as_bytes().as_slice()).bind(next_workspace_generation).bind(&description).bind(&license).bind(&compatibility)
        .bind(serde_json::to_value(&manifest_wire).map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?)
        .bind(&snapshot_key).bind(&snapshot_sha).bind(snapshot_size).bind(resource_id.as_uuid()).bind(authority.user_id)
        .execute(&mut *tx).await.map_err(internal_api_error)?;
        let outcome = PrivateRevisionResponse {
            resource_id: resource_id.to_string(),
            generation: u64::try_from(next_workspace_generation).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "workspace generation is invalid")
            })?,
            revision_id: new_revision.to_string(),
            description,
            manifest: manifest_wire,
        };
        sqlx::query(
            "INSERT INTO skill_restore_operations (user_id,operation_id,request_hash,resource_id,outcome_json) VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(authority.user_id).bind(operation_id.as_uuid()).bind(supplied_hash.as_bytes().as_slice())
        .bind(resource_id.as_uuid())
        .bind(serde_json::to_value(&outcome).map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?)
        .execute(&mut *tx).await.map_err(internal_api_error)?;
        if !resource_authority.is_team {
            enqueue_resource_wake(
                &mut tx,
                resource_id.as_uuid(),
                u64::try_from(next_resource_generation).map_err(|_| {
                    ApiError::new(ApiErrorCode::Internal, "resource generation is invalid")
                })?,
            )
            .await?;
        }
        tx.commit().await.map_err(internal_api_error)?;
        if !resource_authority.is_team {
            let _ = self.drain_outbox(64).await;
        }
        Ok(outcome)
    }

    pub async fn reconcile_subscriptions(
        &self,
        bearer: &str,
        request: &SyncReconcileRequest,
    ) -> Result<SyncReconcileResponse, ApiError> {
        let catalog = self.subscription_catalog(bearer).await?;
        let known = request
            .known
            .iter()
            .map(|item| (item.resource_id.as_str(), item))
            .collect::<BTreeMap<_, _>>();
        let desired_ids = catalog
            .skills
            .iter()
            .map(|skill| skill.resource_id.as_str())
            .collect::<BTreeSet<_>>();
        let removed_resource_ids = request
            .known
            .iter()
            .filter(|item| !desired_ids.contains(item.resource_id.as_str()))
            .map(|item| item.resource_id.clone())
            .collect();
        let skills = catalog
            .skills
            .into_iter()
            .filter(|skill| {
                known.get(skill.resource_id.as_str()).is_none_or(|local| {
                    local.generation != skill.generation || local.revision_id != skill.revision_id
                })
            })
            .collect();
        Ok(SyncReconcileResponse {
            skills,
            removed_resource_ids,
        })
    }

    pub async fn watched_resource_ids(&self, bearer: &str) -> Result<BTreeSet<Uuid>, ApiError> {
        let subject = self.subscription_subject(bearer).await?;
        let rows = match subject {
            crate::identity_support::SubscriptionSubject::Installation(id) => sqlx::query_scalar::<
                _,
                Uuid,
            >(
                "SELECT s.resource_id FROM installation_subscriptions s WHERE s.installation_id=$1",
            )
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(internal_api_error)?,
            crate::identity_support::SubscriptionSubject::User(id) => {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT s.resource_id FROM account_subscriptions s WHERE s.user_id=$1",
                )
                .bind(id)
                .fetch_all(&self.pool)
                .await
                .map_err(internal_api_error)?
            }
        };
        Ok(rows.into_iter().collect())
    }

    pub fn hint_for_wake(wake: &RegistryWake, watched: &BTreeSet<Uuid>) -> Option<SyncHint> {
        match wake {
            RegistryWake::Resource {
                resource_id,
                generation,
            } if watched.contains(resource_id) => Some(SyncHint::Dirty {
                resources: vec![DirtyResource {
                    resource_id: resource_id.to_string(),
                    generation: *generation,
                }],
            }),
            RegistryWake::Resource { .. } => None,
            RegistryWake::ResyncAll => Some(SyncHint::ResyncAll),
        }
    }
}
