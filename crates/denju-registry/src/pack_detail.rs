use denju_wire::{
    ApiError, ApiErrorCode, PackMember, PackUnavailableReason, PublicSkillManifest,
    SnapshotDownload, SubscribedSkill, SubscriptionContent,
};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    Registry,
    admin::effective_quarantine_tx,
    ingest::decode_32,
    internal_api_error,
    lifecycle::generation_u64,
    pack_storage::{PackRow, ResolvedPackMember},
};

impl Registry {
    pub(crate) async fn pack_member_detail(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        pack: &PackRow,
        actor: Option<(Uuid, Uuid)>,
        member: ResolvedPackMember,
    ) -> Result<PackMember, ApiError> {
        let row = sqlx::query(
            "SELECT r.owner_namespace_id,n.slug,r.slug,r.description,r.generation,r.visibility,r.deleted_at IS NOT NULL, \
             EXISTS(SELECT 1 FROM private_skill_shares s WHERE s.resource_id=r.id AND s.recipient_user_id=$2),n.kind, \
             EXISTS(SELECT 1 FROM team_memberships tm WHERE tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=$2), \
             EXISTS(SELECT 1 FROM skill_releases sr WHERE sr.resource_id=r.id AND sr.revision_id=$3) \
             FROM resources r LEFT JOIN namespaces n ON n.id=r.owner_namespace_id WHERE r.id=$1 AND r.kind='skill'",
        )
        .bind(member.skill_resource_id)
        .bind(actor.map(|value| value.0))
        .bind(member.resolved_revision_id.as_slice())
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "pack member resource is missing"))?;
        let owner_namespace: Option<Uuid> = row.get(0);
        let owner: Option<String> = row.get(1);
        let name: String = row.get(2);
        let description: String = row.get(3);
        let generation: i64 = row.get(4);
        let visibility: String = row.get(5);
        let deleted: bool = row.get(6);
        let shared: bool = row.get(7);
        let owner_kind: Option<String> = row.get(8);
        let team_member: bool = row.get(9);
        let released: bool = row.get(10);
        let locator = format!(
            "@{}/{}",
            owner.clone().unwrap_or_else(|| "deleted".to_owned()),
            name
        );
        let private_actor_readable = if let Some((user_id, namespace_id)) = actor {
            if owner_namespace == Some(namespace_id) || shared {
                true
            } else if owner_kind.as_deref() == Some("team") && team_member {
                released
                    || revision_in_user_workspace(
                        tx,
                        member.skill_resource_id,
                        user_id,
                        &member.resolved_revision_id,
                    )
                    .await?
            } else {
                false
            }
        } else {
            false
        };
        let readable = if deleted {
            false
        } else if pack.visibility == "public" {
            visibility == "public"
        } else if visibility == "public" {
            true
        } else {
            private_actor_readable
        };
        let quarantine = effective_quarantine_tx(
            tx,
            member.skill_resource_id,
            member.resolved_release_version,
        )
        .await?;
        let unavailable_reason = if quarantine.is_some() {
            Some(PackUnavailableReason::Quarantined)
        } else if deleted {
            Some(PackUnavailableReason::Deleted)
        } else if pack.visibility == "public" && visibility != "public" {
            Some(PackUnavailableReason::Unpublished)
        } else if !readable {
            Some(PackUnavailableReason::AccessRevoked)
        } else {
            None
        };
        let revision_id = hex::encode(decode_32(&member.resolved_revision_id, "pack revision ID")?);
        let desired = if unavailable_reason.is_none() {
            let snapshot = sqlx::query_as::<_, (serde_json::Value, String, Vec<u8>, i64)>(
                "SELECT manifest_json,snapshot_key,snapshot_sha256,snapshot_size FROM resource_revision_snapshots \
                 WHERE resource_id=$1 AND revision_id=$2",
            )
            .bind(member.skill_resource_id)
            .bind(member.resolved_revision_id.as_slice())
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal_api_error)?;
            let snapshot = snapshot.ok_or_else(|| {
                ApiError::new(
                    ApiErrorCode::Internal,
                    "pack member revision lost its immutable snapshot",
                )
            })?;
            let manifest: PublicSkillManifest = serde_json::from_value(snapshot.0)
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
            let sha = decode_32(&snapshot.2, "pack member snapshot SHA-256")?;
            let size_bytes = u64::try_from(snapshot.3).map_err(|_| {
                ApiError::new(
                    ApiErrorCode::Internal,
                    "pack member snapshot size is invalid",
                )
            })?;
            let url = self
                .objects
                .presign_get(&snapshot.1)
                .await
                .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
            Some(SubscribedSkill {
                resource_id: member.skill_resource_id.to_string(),
                locator: locator.clone(),
                owner: owner.unwrap_or_else(|| "deleted".to_owned()),
                name,
                description,
                generation: generation_u64(generation)?,
                revision_id: revision_id.clone(),
                deprecation: None,
                content: match member.resolved_release_version {
                    Some(version) => SubscriptionContent::Release {
                        version: generation_u64(version)?,
                        following_latest: member.pinned_release_version.is_none(),
                    },
                    None => SubscriptionContent::PrivateWorkspace,
                },
                manifest,
                snapshot: SnapshotDownload {
                    sha256: hex::encode(sha),
                    size_bytes,
                    url,
                },
                retain_on_delete: false,
                retained_after_delete: false,
            })
        } else {
            None
        };
        Ok(PackMember {
            resource_id: member.skill_resource_id.to_string(),
            locator,
            pinned_release_version: optional_version(member.pinned_release_version)?,
            resolved_release_version: optional_version(member.resolved_release_version)?,
            revision_id,
            unavailable_reason,
            desired,
        })
    }
}

async fn revision_in_user_workspace(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    user_id: Uuid,
    revision_id: &[u8],
) -> Result<bool, ApiError> {
    sqlx::query_scalar::<_, bool>(
        "WITH RECURSIVE ancestry(revision_id) AS ( \
           SELECT revision_id FROM skill_private_workspaces \
           WHERE resource_id=$1 AND workspace_user_id=$2 \
           UNION \
           SELECT rp.parent_revision_id FROM revision_parents rp JOIN ancestry a ON rp.revision_id=a.revision_id \
         ) SELECT EXISTS(SELECT 1 FROM ancestry WHERE revision_id=$3)",
    )
    .bind(resource_id)
    .bind(user_id)
    .bind(revision_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)
}

fn optional_version(value: Option<i64>) -> Result<Option<u64>, ApiError> {
    value.map(generation_u64).transpose()
}
