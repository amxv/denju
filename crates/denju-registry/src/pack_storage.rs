use std::collections::BTreeMap;

use denju_core::ResourceLocator;
use denju_wire::{ApiError, ApiErrorCode, PackSummary};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    admin::effective_quarantine_tx,
    internal_api_error,
    lifecycle::{generation_u64, next_generation},
};

#[derive(Debug, Clone)]
pub(crate) struct PackRow {
    pub(crate) id: Uuid,
    pub(crate) owner_namespace_id: Uuid,
    pub(crate) owner: String,
    pub(crate) name: String,
    pub(crate) generation: i64,
    pub(crate) visibility: String,
    pub(crate) current_version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedPackMember {
    pub(crate) skill_resource_id: Uuid,
    pub(crate) pinned_release_version: Option<i64>,
    pub(crate) resolved_release_version: Option<i64>,
    pub(crate) resolved_revision_id: Vec<u8>,
}

pub(crate) async fn load_owned_pack_for_update(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    namespace_id: Uuid,
) -> Result<PackRow, ApiError> {
    let row = sqlx::query(
        "SELECT r.id,r.owner_namespace_id,n.slug,r.slug,r.generation,r.visibility,ps.current_version \
         FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id JOIN pack_state ps ON ps.resource_id=r.id \
         WHERE r.id=$1 AND r.kind='pack' AND r.deleted_at IS NULL FOR UPDATE OF r,ps",
    )
    .bind(resource_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "pack not found"))?;
    let owner_namespace_id: Uuid = row.get(1);
    if owner_namespace_id != namespace_id {
        return Err(ApiError::new(ApiErrorCode::NotFound, "pack not found"));
    }
    Ok(PackRow {
        id: row.get(0),
        owner_namespace_id,
        owner: row.get(2),
        name: row.get(3),
        generation: row.get(4),
        visibility: row.get(5),
        current_version: row.get(6),
    })
}

pub(crate) async fn load_pack_by_locator(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    locator: &ResourceLocator,
) -> Result<PackRow, ApiError> {
    let active = sqlx::query(
        "SELECT r.id,r.owner_namespace_id,n.slug,r.slug,r.generation,r.visibility,ps.current_version \
         FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id JOIN pack_state ps ON ps.resource_id=r.id \
         WHERE n.slug=$1 AND r.kind='pack' AND r.slug=$2 AND r.deleted_at IS NULL",
    )
    .bind(locator.owner())
    .bind(locator.name())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let row = match active {
        Some(row) => row,
        None => sqlx::query(
            "SELECT r.id,r.owner_namespace_id,n.slug,r.slug,r.generation,r.visibility,ps.current_version \
             FROM resource_redirects redirect JOIN namespaces old_ns ON old_ns.id=redirect.namespace_id \
             JOIN resources r ON r.id=redirect.target_resource_id JOIN namespaces n ON n.id=r.owner_namespace_id \
             JOIN pack_state ps ON ps.resource_id=r.id WHERE old_ns.slug=$1 AND redirect.kind='pack' AND redirect.old_slug=$2 \
             AND r.kind='pack' AND r.deleted_at IS NULL",
        )
        .bind(locator.owner())
        .bind(locator.name())
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "pack not found"))?,
    };
    Ok(PackRow {
        id: row.get(0),
        owner_namespace_id: row.get(1),
        owner: row.get(2),
        name: row.get(3),
        generation: row.get(4),
        visibility: row.get(5),
        current_version: row.get(6),
    })
}

pub(crate) async fn insert_pack_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pack: &PackRow,
    members: &[ResolvedPackMember],
    source_release_event_id: Option<i64>,
) -> Result<PackRow, ApiError> {
    let version = next_generation(pack.current_version)?;
    let generation = next_generation(pack.generation)?;
    sqlx::query(
        "INSERT INTO pack_revisions (pack_resource_id,version,source_release_event_id) VALUES ($1,$2,$3)",
    )
    .bind(pack.id)
    .bind(version)
    .bind(source_release_event_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    for (ordinal, member) in members.iter().enumerate() {
        sqlx::query(
            "INSERT INTO pack_revision_members \
             (pack_resource_id,pack_version,ordinal,skill_resource_id,pinned_release_version,resolved_release_version,resolved_revision_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(pack.id)
        .bind(version)
        .bind(i32::try_from(ordinal).map_err(|_| ApiError::new(ApiErrorCode::Internal, "pack has too many members"))?)
        .bind(member.skill_resource_id)
        .bind(member.pinned_release_version)
        .bind(member.resolved_release_version)
        .bind(member.resolved_revision_id.as_slice())
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    }
    sqlx::query("UPDATE pack_state SET current_version=$1,updated_at=now() WHERE resource_id=$2")
        .bind(version)
        .bind(pack.id)
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    sqlx::query(
        "UPDATE resources SET generation=$1,latest_release_version=CASE WHEN visibility='public' THEN $2 ELSE latest_release_version END WHERE id=$3",
    )
    .bind(generation)
    .bind(version)
    .bind(pack.id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(PackRow {
        generation,
        current_version: version,
        ..pack.clone()
    })
}

pub(crate) async fn order_resolved_members(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    members: &mut [ResolvedPackMember],
) -> Result<(), ApiError> {
    if members.len() < 2 {
        return Ok(());
    }
    let ids = members
        .iter()
        .map(|member| member.skill_resource_id)
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT r.id,COALESCE(n.slug,r.deleted_owner_slug,''),r.slug FROM resources r \
         LEFT JOIN namespaces n ON n.id=r.owner_namespace_id WHERE r.id=ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let keys = rows
        .into_iter()
        .map(|(id, owner, name)| (id, (owner, name)))
        .collect::<BTreeMap<_, _>>();
    if keys.len() != members.len() {
        return Err(ApiError::new(
            ApiErrorCode::Internal,
            "pack membership references a missing skill resource",
        ));
    }
    members.sort_by(|left, right| {
        let left_key = keys
            .get(&left.skill_resource_id)
            .expect("validated pack member ordering key");
        let right_key = keys
            .get(&right.skill_resource_id)
            .expect("validated pack member ordering key");
        left_key
            .cmp(right_key)
            .then_with(|| left.skill_resource_id.cmp(&right.skill_resource_id))
    });
    Ok(())
}

pub(crate) async fn resolve_all_members(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    namespace_id: Uuid,
    public_audience: bool,
    team_audience: bool,
    pack_id: Uuid,
) -> Result<Vec<ResolvedPackMember>, ApiError> {
    let rows = sqlx::query_as::<_, (Uuid, Option<i64>)>(
        "SELECT pm.skill_resource_id,pm.pinned_release_version FROM pack_members pm \
         JOIN resources r ON r.id=pm.skill_resource_id LEFT JOIN namespaces n ON n.id=r.owner_namespace_id \
         WHERE pm.pack_resource_id=$1 ORDER BY COALESCE(n.slug,r.deleted_owner_slug),r.slug,r.id",
    )
    .bind(pack_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let mut resolved = Vec::with_capacity(rows.len());
    for (skill_id, pinned) in rows {
        resolved.push(
            resolve_member(
                tx,
                user_id,
                namespace_id,
                public_audience,
                team_audience,
                skill_id,
                u64_version(pinned)?,
            )
            .await?,
        );
    }
    Ok(resolved)
}

pub(crate) async fn resolve_member(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    namespace_id: Uuid,
    public_audience: bool,
    team_audience: bool,
    skill_id: Uuid,
    pinned_release_version: Option<u64>,
) -> Result<ResolvedPackMember, ApiError> {
    let row = sqlx::query(
        "SELECT r.owner_namespace_id,r.visibility,r.deleted_at IS NOT NULL,r.latest_release_version, \
         EXISTS(SELECT 1 FROM private_skill_shares s WHERE s.resource_id=r.id AND s.recipient_user_id=$2), \
         n.kind, \
         EXISTS(SELECT 1 FROM team_memberships tm WHERE tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=$2), \
         EXISTS(SELECT 1 FROM skill_private_workspaces w WHERE w.resource_id=r.id AND w.workspace_user_id=$2) \
         FROM resources r LEFT JOIN namespaces n ON n.id=r.owner_namespace_id \
         WHERE r.id=$1 AND r.kind='skill'",
    )
    .bind(skill_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "pack member skill not found"))?;
    let owner_namespace: Option<Uuid> = row.get(0);
    let visibility: String = row.get(1);
    let deleted: bool = row.get(2);
    let latest_release: Option<i64> = row.get(3);
    let shared: bool = row.get(4);
    let owner_kind: Option<String> = row.get(5);
    let team_member: bool = row.get(6);
    let own_workspace: bool = row.get(7);
    if deleted || (public_audience && visibility != "public") {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "pack member is not readable by the pack's full audience",
        ));
    }
    let same_namespace = owner_namespace == Some(namespace_id);
    let readable = if public_audience {
        visibility == "public"
    } else if team_audience {
        visibility == "public" || (owner_kind.as_deref() == Some("team") && same_namespace)
    } else {
        visibility == "public"
            || (owner_kind.as_deref() == Some("user") && (same_namespace || shared))
            || (owner_kind.as_deref() == Some("team")
                && ((latest_release.is_some() && (team_member || shared))
                    || (team_member && own_workspace)))
    };
    if !readable {
        return Err(ApiError::new(
            ApiErrorCode::NotFound,
            "pack member skill is not readable",
        ));
    }
    if let Some(version) = pinned_release_version {
        let version = i64::try_from(version).map_err(|_| {
            ApiError::new(ApiErrorCode::InvalidRequest, "release version is too large")
        })?;
        if effective_quarantine_tx(tx, skill_id, Some(version))
            .await?
            .is_some()
        {
            return Err(ApiError::new(
                ApiErrorCode::NotFound,
                "pack member skill release is quarantined",
            ));
        }
        let revision = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT revision_id FROM skill_releases WHERE resource_id=$1 AND version=$2",
        )
        .bind(skill_id)
        .bind(version)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "pinned skill release not found"))?;
        return Ok(ResolvedPackMember {
            skill_resource_id: skill_id,
            pinned_release_version: Some(version),
            resolved_release_version: Some(version),
            resolved_revision_id: revision,
        });
    }
    if let Some(version) = latest_release {
        if effective_quarantine_tx(tx, skill_id, Some(version))
            .await?
            .is_some()
        {
            return Err(ApiError::new(
                ApiErrorCode::NotFound,
                "pack member skill release is quarantined",
            ));
        }
        let revision = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT revision_id FROM skill_releases WHERE resource_id=$1 AND version=$2",
        )
        .bind(skill_id)
        .bind(version)
        .fetch_one(&mut **tx)
        .await
        .map_err(internal_api_error)?;
        return Ok(ResolvedPackMember {
            skill_resource_id: skill_id,
            pinned_release_version: None,
            resolved_release_version: Some(version),
            resolved_revision_id: revision,
        });
    }
    if public_audience || team_audience {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            if public_audience {
                "public pack members must have a published release"
            } else {
                "team pack members must have a team-visible published release"
            },
        ));
    }
    if effective_quarantine_tx(tx, skill_id, None).await?.is_some() {
        return Err(ApiError::new(
            ApiErrorCode::NotFound,
            "pack member skill is quarantined",
        ));
    }
    let revision = if owner_kind.as_deref() == Some("team") {
        sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT revision_id FROM skill_private_workspaces WHERE resource_id=$1 AND workspace_user_id=$2",
        )
        .bind(skill_id)
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?
    } else {
        sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT w.revision_id FROM resources r JOIN users u ON u.namespace_id=r.owner_namespace_id \
             JOIN skill_private_workspaces w ON w.resource_id=r.id AND w.workspace_user_id=u.id \
             WHERE r.id=$1",
        )
        .bind(skill_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?
    }
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "pack member workspace not found"))?;
    Ok(ResolvedPackMember {
        skill_resource_id: skill_id,
        pinned_release_version: None,
        resolved_release_version: None,
        resolved_revision_id: revision,
    })
}

pub(crate) async fn load_pack_revision_members(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pack_id: Uuid,
    version: i64,
) -> Result<Vec<ResolvedPackMember>, ApiError> {
    let rows = sqlx::query_as::<_, (Uuid, Option<i64>, Option<i64>, Vec<u8>)>(
        "SELECT skill_resource_id,pinned_release_version,resolved_release_version,resolved_revision_id \
         FROM pack_revision_members WHERE pack_resource_id=$1 AND pack_version=$2 ORDER BY ordinal",
    )
    .bind(pack_id)
    .bind(version)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(rows
        .into_iter()
        .map(|row| ResolvedPackMember {
            skill_resource_id: row.0,
            pinned_release_version: row.1,
            resolved_release_version: row.2,
            resolved_revision_id: row.3,
        })
        .collect())
}

pub(crate) async fn pack_summary(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pack: &PackRow,
) -> Result<PackSummary, ApiError> {
    let member_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM pack_revision_members WHERE pack_resource_id=$1 AND pack_version=$2",
    )
    .bind(pack.id)
    .bind(pack.current_version)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(PackSummary {
        resource_id: pack.id.to_string(),
        locator: format!("@{}/packs/{}", pack.owner, pack.name),
        generation: generation_u64(pack.generation)?,
        version: generation_u64(pack.current_version)?,
        visibility: pack.visibility.clone(),
        member_count: u64::try_from(member_count)
            .map_err(|_| ApiError::new(ApiErrorCode::Internal, "pack member count is invalid"))?,
        degraded: false,
    })
}

fn u64_version(value: Option<i64>) -> Result<Option<u64>, ApiError> {
    value.map(generation_u64).transpose()
}
