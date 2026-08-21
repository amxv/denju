use denju_wire::{ApiError, ApiErrorCode};
use uuid::Uuid;

use crate::internal_api_error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillAccess {
    pub(crate) visibility: String,
    pub(crate) owner_access: bool,
    pub(crate) shared_access: bool,
}

impl SkillAccess {
    pub(crate) fn can_read_private(&self) -> bool {
        self.owner_access || self.shared_access
    }
}

pub(crate) async fn skill_access_for_user(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    namespace_id: Uuid,
    resource_id: Uuid,
) -> Result<SkillAccess, ApiError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, String, bool)>(
        "SELECT r.owner_namespace_id,n.slug,r.slug,r.visibility, \
                EXISTS(SELECT 1 FROM private_skill_shares s WHERE s.resource_id=r.id AND s.recipient_user_id=$2) \
         FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
         WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL",
    )
    .bind(resource_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "skill not found"))?;
    Ok(SkillAccess {
        visibility: row.3,
        owner_access: row.0 == namespace_id,
        shared_access: row.4,
    })
}

pub(crate) async fn user_can_read_revision(
    pool: &sqlx::PgPool,
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
        .fetch_one(pool)
        .await
        .map_err(internal_api_error);
    }
    if access.visibility != "public" {
        return Ok(false);
    }
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM skill_releases WHERE resource_id=$1 AND revision_id=$2)",
    )
    .bind(resource_id)
    .bind(revision_id.as_slice())
    .fetch_one(pool)
    .await
    .map_err(internal_api_error)
}
