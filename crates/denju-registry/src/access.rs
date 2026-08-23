use denju_wire::{ApiError, ApiErrorCode};
use uuid::Uuid;

use crate::internal_api_error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillAccess {
    pub(crate) user_id: Uuid,
    pub(crate) visibility: String,
    pub(crate) team_owned: bool,
    pub(crate) owner_access: bool,
    pub(crate) shared_access: bool,
    pub(crate) team_release_access: bool,
    pub(crate) workspace_access: bool,
}

impl SkillAccess {
    pub(crate) fn can_read_private(&self) -> bool {
        self.owner_access || (self.shared_access && !self.team_owned)
    }

    pub(crate) fn can_read_released(&self) -> bool {
        self.visibility == "public"
            || self.team_release_access
            || self.shared_access
            || self.owner_access
    }
}

pub(crate) async fn skill_access_for_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    namespace_id: Uuid,
    resource_id: Uuid,
) -> Result<SkillAccess, ApiError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, bool, bool, bool)>(
        "SELECT r.owner_namespace_id,r.visibility,n.kind, \
                EXISTS(SELECT 1 FROM private_skill_shares s WHERE s.resource_id=r.id AND s.recipient_user_id=$2), \
                EXISTS(SELECT 1 FROM team_memberships tm WHERE tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=$2), \
                EXISTS(SELECT 1 FROM skill_private_workspaces w WHERE w.resource_id=r.id AND w.workspace_user_id=$2) \
         FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
         WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL",
    )
    .bind(resource_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "skill not found"))?;
    Ok(SkillAccess {
        user_id,
        visibility: row.1,
        team_owned: row.2 == "team",
        owner_access: row.2 == "user" && row.0 == namespace_id,
        shared_access: row.3,
        team_release_access: row.2 == "team" && row.4,
        workspace_access: row.5,
    })
}

pub(crate) async fn user_can_read_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &SkillAccess,
    resource_id: Uuid,
    revision_id: &[u8; 32],
) -> Result<bool, ApiError> {
    if access.can_read_private() {
        return sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM resource_revision_snapshots WHERE resource_id=$1 AND revision_id=$2)",
        )
        .bind(resource_id)
        .bind(revision_id.as_slice())
        .fetch_one(&mut **tx)
        .await
        .map_err(internal_api_error);
    }
    if access.workspace_access {
        let workspace_revision = sqlx::query_scalar::<_, bool>(
            "WITH RECURSIVE ancestry(revision_id) AS ( \
               SELECT revision_id FROM skill_private_workspaces \
               WHERE resource_id=$1 AND workspace_user_id=$2 \
               UNION \
               SELECT rp.parent_revision_id FROM revision_parents rp JOIN ancestry a ON rp.revision_id=a.revision_id \
             ) SELECT EXISTS(SELECT 1 FROM ancestry WHERE revision_id=$3)",
        )
        .bind(resource_id)
        .bind(access.user_id)
        .bind(revision_id.as_slice())
        .fetch_one(&mut **tx)
        .await
        .map_err(internal_api_error)?;
        if workspace_revision {
            return Ok(true);
        }
    }
    if !access.can_read_released() {
        return Ok(false);
    }
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM skill_releases WHERE resource_id=$1 AND revision_id=$2)",
    )
    .bind(resource_id)
    .bind(revision_id.as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)
}

pub(crate) async fn user_can_fork_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    personal_namespace_id: Uuid,
    resource_id: Uuid,
    revision_id: &[u8; 32],
) -> Result<bool, ApiError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, bool, bool, bool, bool)>(
        "SELECT r.owner_namespace_id,n.kind,r.visibility, \
                EXISTS(SELECT 1 FROM private_skill_shares s WHERE s.resource_id=r.id AND s.recipient_user_id=$2), \
                EXISTS(SELECT 1 FROM resource_revision_snapshots rs WHERE rs.resource_id=r.id AND rs.revision_id=$3), \
                EXISTS(SELECT 1 FROM skill_releases sr WHERE sr.resource_id=r.id AND sr.revision_id=$3), \
                EXISTS(SELECT 1 FROM team_memberships tm WHERE tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=$2) \
         FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
         WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL",
    )
    .bind(resource_id)
    .bind(user_id)
    .bind(revision_id.as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let Some((owner_namespace_id, owner_kind, visibility, shared, exists, released, team_member)) =
        row
    else {
        return Ok(false);
    };
    if !exists {
        return Ok(false);
    }
    if owner_kind == "user" && (owner_namespace_id == personal_namespace_id || shared) {
        return Ok(true);
    }
    if visibility == "public" && released {
        return Ok(true);
    }
    if owner_kind != "team" {
        return Ok(false);
    }
    if released && (team_member || shared) {
        return Ok(true);
    }
    if !team_member {
        return Ok(false);
    }
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
    .bind(revision_id.as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)
}
