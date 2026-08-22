use std::collections::BTreeMap;

use denju_wire::{
    ApiError, ApiErrorCode, PrivateSkill, PrivateSkillCatalog, SkillForkProvenance,
    SnapshotDownload,
};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    Registry,
    ingest::{decode_32, object_store_api_error},
    internal_api_error,
    workspace_conflict::unresolved_workspace_conflicts_for_resources,
};

#[derive(Debug, FromRow)]
struct PrivateSkillRow {
    resource_id: Uuid,
    owner: String,
    name: String,
    description: String,
    resource_generation: i64,
    workspace_generation: i64,
    revision_id: Vec<u8>,
    manifest_json: Value,
    snapshot_key: String,
    snapshot_sha256: Vec<u8>,
    snapshot_size: i64,
    upstream_resource_id: Option<Uuid>,
    upstream_owner: Option<String>,
    upstream_name: Option<String>,
    created_from_revision_id: Option<Vec<u8>>,
    sync_base_revision_id: Option<Vec<u8>>,
}

impl Registry {
    pub async fn private_skill_catalog(
        &self,
        bearer: &str,
    ) -> Result<PrivateSkillCatalog, ApiError> {
        let authority = self.user_authority(bearer, "skills:read").await?;
        // Team workspaces are private maintainer refs, not shared drafts. Provision a missing
        // ref only from the team's last immutable release; unpublished work from another
        // maintainer is never a seed. Ordinary members receive none unless the team-wide
        // members-can-publish policy explicitly grants them maintainer-equivalent write access.
        sqlx::query(
            "INSERT INTO skill_private_workspaces \
             (resource_id,workspace_user_id,description,revision_id,generation,manifest_json,snapshot_key,snapshot_sha256,snapshot_size) \
             SELECT r.id,$1,r.description,sr.revision_id,r.generation,rrs.manifest_json,rrs.snapshot_key,rrs.snapshot_sha256,rrs.snapshot_size \
             FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id AND n.kind='team' \
             JOIN teams t ON t.namespace_id=r.owner_namespace_id \
             JOIN team_memberships tm ON tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=$1 \
             JOIN skill_releases sr ON sr.resource_id=r.id AND sr.version=r.latest_release_version \
             JOIN resource_revision_snapshots rrs ON rrs.resource_id=r.id AND rrs.revision_id=sr.revision_id \
             WHERE r.kind='skill' AND r.deleted_at IS NULL AND \
               (tm.role IN ('owner','maintainer') OR (tm.role='member' AND t.members_can_publish)) \
             ON CONFLICT(resource_id,workspace_user_id) DO NOTHING",
        )
        .bind(authority.user_id)
        .execute(&self.pool)
        .await
        .map_err(internal_api_error)?;
        let rows = sqlx::query_as::<_, PrivateSkillRow>(
            "SELECT r.id AS resource_id,n.slug AS owner,r.slug AS name,w.description, \
                    r.generation AS resource_generation,w.generation AS workspace_generation, \
                    w.revision_id,w.manifest_json,w.snapshot_key,w.snapshot_sha256,w.snapshot_size, \
                    f.upstream_resource_id,COALESCE(upstream_owner.slug,upstream.deleted_owner_slug) AS upstream_owner,upstream.slug AS upstream_name, \
                    f.created_from_revision_id,f.sync_base_revision_id \
             FROM resources r \
             JOIN namespaces n ON n.id=r.owner_namespace_id \
             JOIN skill_private_workspaces w ON w.resource_id=r.id AND w.workspace_user_id=$1 \
             LEFT JOIN skill_forks f ON f.resource_id=r.id \
             LEFT JOIN resources upstream ON upstream.id=f.upstream_resource_id \
             LEFT JOIN namespaces upstream_owner ON upstream_owner.id=upstream.owner_namespace_id \
             WHERE r.kind='skill' AND r.deleted_at IS NULL \
               AND COALESCE(f.promotion_pending,FALSE)=FALSE \
             ORDER BY r.slug,r.id",
        )
        .bind(authority.user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(internal_api_error)?;
        let generations = rows
            .iter()
            .map(|row| {
                let generation = u64::try_from(row.workspace_generation).map_err(|_| {
                    ApiError::new(ApiErrorCode::Internal, "stored generation is invalid")
                })?;
                Ok((row.resource_id, generation))
            })
            .collect::<Result<BTreeMap<_, _>, ApiError>>()?;
        let mut conflicts = unresolved_workspace_conflicts_for_resources(
            &self.pool,
            authority.user_id,
            &generations,
        )
        .await?;

        let mut skills = Vec::with_capacity(rows.len());
        for row in rows {
            let fork = fork_provenance(&row)?;
            let manifest = serde_json::from_value(row.manifest_json)
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
            let revision_id = decode_32(&row.revision_id, "stored revision ID")?;
            let snapshot_sha = decode_32(&row.snapshot_sha256, "stored snapshot SHA-256")?;
            let workspace_generation = generations[&row.resource_id];
            let generation = u64::try_from(row.resource_generation).map_err(|_| {
                ApiError::new(
                    ApiErrorCode::Internal,
                    "stored resource generation is invalid",
                )
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
                workspace_generation,
                revision_id: hex::encode(revision_id),
                manifest,
                snapshot: SnapshotDownload {
                    sha256: hex::encode(snapshot_sha),
                    size_bytes: snapshot_size,
                    url,
                },
                conflicts: conflicts.remove(&row.resource_id).unwrap_or_default(),
                fork,
            });
        }
        Ok(PrivateSkillCatalog { skills })
    }
}

fn fork_provenance(row: &PrivateSkillRow) -> Result<Option<SkillForkProvenance>, ApiError> {
    let Some(resource_id) = row.upstream_resource_id else {
        if row.upstream_owner.is_some()
            || row.upstream_name.is_some()
            || row.created_from_revision_id.is_some()
            || row.sync_base_revision_id.is_some()
        {
            return Err(ApiError::new(
                ApiErrorCode::Internal,
                "stored fork provenance is incomplete",
            ));
        }
        return Ok(None);
    };
    let owner = row.upstream_owner.as_deref().ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::Internal,
            "stored fork upstream owner is missing",
        )
    })?;
    let name = row.upstream_name.as_deref().ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::Internal,
            "stored fork upstream name is missing",
        )
    })?;
    let created = decode_32(
        row.created_from_revision_id.as_deref().ok_or_else(|| {
            ApiError::new(
                ApiErrorCode::Internal,
                "stored fork creation revision is missing",
            )
        })?,
        "stored fork creation revision",
    )?;
    let sync_base = decode_32(
        row.sync_base_revision_id.as_deref().ok_or_else(|| {
            ApiError::new(ApiErrorCode::Internal, "stored fork sync base is missing")
        })?,
        "stored fork sync base",
    )?;
    Ok(Some(SkillForkProvenance {
        upstream_resource_id: resource_id.to_string(),
        upstream_locator: format!("@{owner}/{name}"),
        created_from_revision_id: hex::encode(created),
        sync_base_revision_id: hex::encode(sync_base),
    }))
}
