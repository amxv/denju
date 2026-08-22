use denju_core::ResourceLocator;
use denju_wire::{ApiError, ApiErrorCode, TeamRole};
use sqlx::Row;
use uuid::Uuid;

use crate::{identity_support::UserAuthority, internal_api_error};

#[derive(Debug, Clone)]
pub(crate) struct NamespacePublishAuthority {
    pub(crate) namespace_id: Uuid,
    pub(crate) namespace_slug: String,
    pub(crate) is_team: bool,
}

pub(crate) fn role_can_publish(role: TeamRole, members_can_publish: bool) -> bool {
    matches!(role, TeamRole::Owner | TeamRole::Maintainer)
        || (role == TeamRole::Member && members_can_publish)
}

pub(crate) async fn authorize_namespace_publish(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    authority: &UserAuthority,
    owner: &str,
) -> Result<NamespacePublishAuthority, ApiError> {
    let slug = parse_namespace(owner)?;
    let row = sqlx::query(
        "SELECT n.id,n.kind,tm.role,t.members_can_publish \
         FROM namespaces n \
         LEFT JOIN teams t ON t.namespace_id=n.id \
         LEFT JOIN team_memberships tm ON tm.team_namespace_id=n.id AND tm.user_id=$2 \
         WHERE n.slug=$1 FOR SHARE OF n",
    )
    .bind(&slug)
    .bind(authority.user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "namespace not found"))?;
    let namespace_id: Uuid = row.get(0);
    let kind: String = row.get(1);
    if kind == "user" {
        if namespace_id != authority.namespace_id {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "resource namespace is unavailable",
            ));
        }
        return Ok(NamespacePublishAuthority {
            namespace_id,
            namespace_slug: slug,
            is_team: false,
        });
    }
    if kind != "team" {
        return Err(ApiError::new(
            ApiErrorCode::Internal,
            "stored namespace kind is invalid",
        ));
    }
    let role = row
        .get::<Option<String>, _>(2)
        .as_deref()
        .map(parse_role)
        .transpose()?
        .ok_or_else(|| ApiError::new(ApiErrorCode::Unauthorized, "team is unavailable"))?;
    let members_can_publish = row
        .get::<Option<bool>, _>(3)
        .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "team settings are unavailable"))?;
    if !role_can_publish(role, members_can_publish) {
        return Err(ApiError::new(
            ApiErrorCode::Unauthorized,
            "team role does not allow publishing",
        ));
    }
    Ok(NamespacePublishAuthority {
        namespace_id,
        namespace_slug: slug,
        is_team: true,
    })
}

pub(crate) async fn authorize_resource_publish(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    authority: &UserAuthority,
    resource_id: Uuid,
) -> Result<NamespacePublishAuthority, ApiError> {
    let owner = sqlx::query_scalar::<_, String>(
        "SELECT n.slug FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
         WHERE r.id=$1 AND r.deleted_at IS NULL",
    )
    .bind(resource_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "resource not found"))?;
    authorize_namespace_publish(tx, authority, &owner).await
}

pub(crate) async fn user_is_team_member(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    namespace_id: Uuid,
) -> Result<bool, ApiError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM team_memberships WHERE team_namespace_id=$1 AND user_id=$2)",
    )
    .bind(namespace_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(internal_api_error)
}

pub(crate) async fn ensure_private_workspace_for_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    user_id: Uuid,
) -> Result<bool, ApiError> {
    let inserted = sqlx::query(
        "INSERT INTO skill_private_workspaces \
         (resource_id,workspace_user_id,description,revision_id,generation,manifest_json,snapshot_key,snapshot_sha256,snapshot_size) \
         SELECT r.id,$2,r.description,sr.revision_id,r.generation,rrs.manifest_json,rrs.snapshot_key,rrs.snapshot_sha256,rrs.snapshot_size \
         FROM resources r \
         JOIN skill_releases sr ON sr.resource_id=r.id AND sr.version=r.latest_release_version \
         JOIN resource_revision_snapshots rrs ON rrs.resource_id=r.id AND rrs.revision_id=sr.revision_id \
         WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL \
         ON CONFLICT(resource_id,workspace_user_id) DO NOTHING",
    )
    .bind(resource_id)
    .bind(user_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .rows_affected();
    Ok(inserted == 1)
}

fn parse_namespace(value: &str) -> Result<String, ApiError> {
    let slug = value.strip_prefix('@').unwrap_or(value);
    if slug.contains('/') || slug.is_empty() {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "team namespace must be a single @name",
        ));
    }
    let locator = format!("@{slug}/validation")
        .parse::<ResourceLocator>()
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
    Ok(locator.owner().to_owned())
}

fn parse_role(value: &str) -> Result<TeamRole, ApiError> {
    match value {
        "owner" => Ok(TeamRole::Owner),
        "maintainer" => Ok(TeamRole::Maintainer),
        "member" => Ok(TeamRole::Member),
        _ => Err(ApiError::new(
            ApiErrorCode::Internal,
            "stored team role is invalid",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_authority_matches_team_role_policy() {
        assert!(role_can_publish(TeamRole::Owner, false));
        assert!(role_can_publish(TeamRole::Owner, true));
        assert!(role_can_publish(TeamRole::Maintainer, false));
        assert!(role_can_publish(TeamRole::Maintainer, true));
        assert!(!role_can_publish(TeamRole::Member, false));
        assert!(role_can_publish(TeamRole::Member, true));
    }
}
