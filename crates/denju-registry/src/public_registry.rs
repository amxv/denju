use std::str::FromStr;

use denju_core::{ResourceKind, ResourceLocator};
use denju_wire::{
    ApiError, ApiErrorCode, PublicSkill, PublicSkillDetail, PublicSkillSearchResponse,
    SkillDeprecation, SkillForkProvenance,
};
use uuid::Uuid;

use crate::{
    Registry, admin::active_quarantine, internal_api_error,
    lifecycle::resolve_active_skill_locator_tx,
};

impl Registry {
    pub async fn search_public_skills(
        &self,
        bearer: Option<&str>,
        query: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<PublicSkillSearchResponse, ApiError> {
        let authority = self.optional_read_authority(bearer).await?;
        let limit = limit.clamp(1, 50);
        let pattern = format!("%{}%", query.trim());
        let rows = if let Some(authority) = authority.as_ref() {
            let mut tx = self.begin_actor_tx(authority.user_id).await?;
            let rows = if let Some(cursor) = cursor {
                let cursor = SearchCursor::decode(cursor)?;
                sqlx::query_as::<_, PublicSkillSearchRow>(
                    "SELECT r.id,n.slug AS owner,r.slug AS name,r.description,r.generation, \
                            CASE WHEN r.visibility='public' OR (n.kind='team' AND team_w.resource_id IS NULL) THEN sr.version ELSE NULL END AS version, \
                            CASE WHEN r.visibility='public' THEN sr.revision_id \
                                 WHEN n.kind='team' THEN COALESCE(team_w.revision_id,sr.revision_id) \
                                 ELSE w.revision_id END AS revision_id, \
                            CASE WHEN r.visibility='public' THEN r.deprecated_at IS NOT NULL ELSE FALSE END AS deprecated, \
                            CASE WHEN r.visibility='public' THEN replacement.id ELSE NULL END AS replacement_id, \
                            CASE WHEN r.visibility='public' THEN replacement_owner.slug ELSE NULL END AS replacement_owner, \
                            CASE WHEN r.visibility='public' THEN replacement.slug ELSE NULL END AS replacement_name, \
                            (r.visibility<>'public' AND (n.kind='user' OR team_w.resource_id IS NOT NULL)) AS live_private \
                     FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
                     LEFT JOIN skill_releases sr ON sr.resource_id=r.id AND sr.version=r.latest_release_version \
                     LEFT JOIN users owner_user ON owner_user.namespace_id=r.owner_namespace_id \
                     LEFT JOIN skill_private_workspaces w ON w.resource_id=r.id AND w.workspace_user_id=owner_user.id \
                     LEFT JOIN skill_private_workspaces team_w ON team_w.resource_id=r.id AND team_w.workspace_user_id=$2 AND n.kind='team' \
                     LEFT JOIN private_skill_shares ps ON ps.resource_id=r.id AND ps.recipient_user_id=$2 \
                     LEFT JOIN team_memberships tm ON tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=$2 \
                     LEFT JOIN skill_forks f ON f.resource_id=r.id \
                     LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                     LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                     WHERE r.kind='skill' AND r.deleted_at IS NULL AND COALESCE(f.promotion_pending,FALSE)=FALSE \
                       AND ((r.visibility='public' AND sr.resource_id IS NOT NULL) OR \
                            (r.visibility<>'public' AND n.kind='user' AND w.resource_id IS NOT NULL \
                             AND (r.owner_namespace_id=$3 OR ps.resource_id IS NOT NULL)) OR \
                            (r.visibility<>'public' AND n.kind='team' AND ( \
                               team_w.resource_id IS NOT NULL OR \
                               (sr.resource_id IS NOT NULL AND (tm.user_id IS NOT NULL OR ps.resource_id IS NOT NULL)) \
                            ))) \
                       AND (r.slug ILIKE $1 OR n.slug ILIKE $1 OR r.description ILIKE $1) \
                       AND ((CASE WHEN r.visibility='public' THEN r.deprecated_at IS NOT NULL ELSE FALSE END) > $4 OR \
                            ((CASE WHEN r.visibility='public' THEN r.deprecated_at IS NOT NULL ELSE FALSE END) = $4 AND \
                             (n.slug > $5 OR (n.slug=$5 AND r.slug>$6) OR (n.slug=$5 AND r.slug=$6 AND r.id>$7)))) \
                     ORDER BY (CASE WHEN r.visibility='public' THEN r.deprecated_at IS NOT NULL ELSE FALSE END),n.slug,r.slug,r.id LIMIT $8",
                )
                .bind(&pattern)
                .bind(authority.user_id)
                .bind(authority.namespace_id)
                .bind(cursor.deprecated)
                .bind(cursor.owner)
                .bind(cursor.name)
                .bind(cursor.resource_id)
                .bind(i64::from(limit) + 1)
                .fetch_all(&mut *tx)
                .await
                .map_err(internal_api_error)?
            } else {
                sqlx::query_as::<_, PublicSkillSearchRow>(
                    "SELECT r.id,n.slug AS owner,r.slug AS name,r.description,r.generation, \
                            CASE WHEN r.visibility='public' OR (n.kind='team' AND team_w.resource_id IS NULL) THEN sr.version ELSE NULL END AS version, \
                            CASE WHEN r.visibility='public' THEN sr.revision_id \
                                 WHEN n.kind='team' THEN COALESCE(team_w.revision_id,sr.revision_id) \
                                 ELSE w.revision_id END AS revision_id, \
                            CASE WHEN r.visibility='public' THEN r.deprecated_at IS NOT NULL ELSE FALSE END AS deprecated, \
                            CASE WHEN r.visibility='public' THEN replacement.id ELSE NULL END AS replacement_id, \
                            CASE WHEN r.visibility='public' THEN replacement_owner.slug ELSE NULL END AS replacement_owner, \
                            CASE WHEN r.visibility='public' THEN replacement.slug ELSE NULL END AS replacement_name, \
                            (r.visibility<>'public' AND (n.kind='user' OR team_w.resource_id IS NOT NULL)) AS live_private \
                     FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
                     LEFT JOIN skill_releases sr ON sr.resource_id=r.id AND sr.version=r.latest_release_version \
                     LEFT JOIN users owner_user ON owner_user.namespace_id=r.owner_namespace_id \
                     LEFT JOIN skill_private_workspaces w ON w.resource_id=r.id AND w.workspace_user_id=owner_user.id \
                     LEFT JOIN skill_private_workspaces team_w ON team_w.resource_id=r.id AND team_w.workspace_user_id=$2 AND n.kind='team' \
                     LEFT JOIN private_skill_shares ps ON ps.resource_id=r.id AND ps.recipient_user_id=$2 \
                     LEFT JOIN team_memberships tm ON tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=$2 \
                     LEFT JOIN skill_forks f ON f.resource_id=r.id \
                     LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                     LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                     WHERE r.kind='skill' AND r.deleted_at IS NULL AND COALESCE(f.promotion_pending,FALSE)=FALSE \
                       AND ((r.visibility='public' AND sr.resource_id IS NOT NULL) OR \
                            (r.visibility<>'public' AND n.kind='user' AND w.resource_id IS NOT NULL \
                             AND (r.owner_namespace_id=$3 OR ps.resource_id IS NOT NULL)) OR \
                            (r.visibility<>'public' AND n.kind='team' AND ( \
                               team_w.resource_id IS NOT NULL OR \
                               (sr.resource_id IS NOT NULL AND (tm.user_id IS NOT NULL OR ps.resource_id IS NOT NULL)) \
                            ))) \
                       AND (r.slug ILIKE $1 OR n.slug ILIKE $1 OR r.description ILIKE $1) \
                     ORDER BY (CASE WHEN r.visibility='public' THEN r.deprecated_at IS NOT NULL ELSE FALSE END),n.slug,r.slug,r.id LIMIT $4",
                )
                .bind(&pattern)
                .bind(authority.user_id)
                .bind(authority.namespace_id)
                .bind(i64::from(limit) + 1)
                .fetch_all(&mut *tx)
                .await
                .map_err(internal_api_error)?
            };
            tx.commit().await.map_err(internal_api_error)?;
            rows
        } else if let Some(cursor) = cursor {
            let cursor = SearchCursor::decode(cursor)?;
            sqlx::query_as::<_, PublicSkillSearchRow>(
                "SELECT r.id, n.slug AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, \
                        r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                        replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name, FALSE AS live_private \
                 FROM resources r \
                 JOIN namespaces n ON n.id = r.owner_namespace_id \
                 JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = r.latest_release_version \
                 LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                 LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                 WHERE r.visibility = 'public' AND r.kind = 'skill' AND r.deleted_at IS NULL \
                   AND (r.slug ILIKE $1 OR n.slug ILIKE $1 OR r.description ILIKE $1) \
                   AND ((r.deprecated_at IS NOT NULL) > $2 OR ((r.deprecated_at IS NOT NULL) = $2 AND \
                        (n.slug > $3 OR (n.slug = $3 AND r.slug > $4) \
                         OR (n.slug = $3 AND r.slug = $4 AND r.id > $5)))) \
                 ORDER BY (r.deprecated_at IS NOT NULL), n.slug, r.slug, r.id LIMIT $6",
            )
            .bind(&pattern)
            .bind(cursor.deprecated)
            .bind(cursor.owner)
            .bind(cursor.name)
            .bind(cursor.resource_id)
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await
            .map_err(internal_api_error)?
        } else {
            sqlx::query_as::<_, PublicSkillSearchRow>(
                "SELECT r.id, n.slug AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, \
                        r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                        replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name, FALSE AS live_private \
                 FROM resources r \
                 JOIN namespaces n ON n.id = r.owner_namespace_id \
                 JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = r.latest_release_version \
                 LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                 LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                 WHERE r.visibility = 'public' AND r.kind = 'skill' AND r.deleted_at IS NULL \
                   AND (r.slug ILIKE $1 OR n.slug ILIKE $1 OR r.description ILIKE $1) \
                 ORDER BY (r.deprecated_at IS NOT NULL), n.slug, r.slug, r.id LIMIT $2",
            )
            .bind(&pattern)
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await
            .map_err(internal_api_error)?
        };

        let has_more = rows.len() > limit as usize;
        let visible = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
        let next_cursor = if has_more {
            visible
                .last()
                .map(SearchCursor::from_row)
                .map(|cursor| cursor.encode())
        } else {
            None
        };
        let items = visible
            .into_iter()
            .map(PublicSkillSearchRow::into_wire)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PublicSkillSearchResponse { items, next_cursor })
    }

    pub async fn show_public_skill(
        &self,
        bearer: Option<&str>,
        locator: &str,
    ) -> Result<PublicSkillDetail, ApiError> {
        let locator = ResourceLocator::from_str(locator)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        if locator.kind() != ResourceKind::Skill {
            return Err(ApiError::new(
                ApiErrorCode::NotFound,
                "public skill not found",
            ));
        }
        if let Some(authority) = self.optional_read_authority(bearer).await? {
            let mut tx = self.begin_actor_tx(authority.user_id).await?;
            let resolved = match resolve_active_skill_locator_tx(&mut tx, &locator).await {
                Ok(resolved) => Some(resolved),
                Err(error) if error.code == ApiErrorCode::NotFound => None,
                Err(error) => return Err(error),
            };
            let private = if let Some(resolved) = resolved.as_ref() {
                if active_quarantine(&self.pool, resolved.resource_id, None)
                    .await?
                    .is_some()
                {
                    return Err(ApiError::new(ApiErrorCode::NotFound, "skill not found"));
                }
                sqlx::query_as::<_, PublicSkillDetailRow>(
                    "SELECT r.id,n.slug AS owner,r.slug AS name,r.description,r.generation,NULL::bigint AS version,w.revision_id,w.manifest_json, \
                        FALSE AS deprecated,NULL::uuid AS replacement_id,NULL::text AS replacement_owner,NULL::text AS replacement_name,TRUE AS live_private \
                 FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
                 JOIN users owner_user ON owner_user.namespace_id=r.owner_namespace_id \
                 JOIN skill_private_workspaces w ON w.resource_id=r.id AND w.workspace_user_id=owner_user.id \
                 LEFT JOIN private_skill_shares ps ON ps.resource_id=r.id AND ps.recipient_user_id=$3 \
                 LEFT JOIN skill_forks f ON f.resource_id=r.id \
                 WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL AND r.visibility<>'public' \
                   AND COALESCE(f.promotion_pending,FALSE)=FALSE \
                   AND (r.owner_namespace_id=$2 OR ps.resource_id IS NOT NULL)",
                )
                .bind(resolved.resource_id)
                .bind(authority.namespace_id)
                .bind(authority.user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal_api_error)?
            } else {
                None
            };
            tx.commit().await.map_err(internal_api_error)?;
            if let Some(row) = private {
                let redirected_from = resolved
                    .as_ref()
                    .filter(|resolved| {
                        resolved.owner != locator.owner() || resolved.name != locator.name()
                    })
                    .map(|_| locator.to_string());
                return self
                    .skill_detail_with_provenance(row, redirected_from)
                    .await;
            }
            if let Some(bearer) = bearer
                && let Some(detail) = self.team_skill_detail(bearer, &locator).await?
            {
                return Ok(detail);
            }
        }
        self.public_skill_detail(locator.owner(), locator.name())
            .await
    }

    async fn public_skill_detail(
        &self,
        owner: &str,
        name: &str,
    ) -> Result<PublicSkillDetail, ApiError> {
        let active = sqlx::query_as::<_, PublicSkillDetailRow>(
            "SELECT r.id, n.slug AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, sr.manifest_json, \
                    r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                    replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name, FALSE AS live_private \
             FROM resources r \
             JOIN namespaces n ON n.id = r.owner_namespace_id \
             JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = r.latest_release_version \
             LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
             LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
             WHERE r.visibility = 'public' AND r.kind = 'skill' AND r.deleted_at IS NULL AND n.slug = $1 AND r.slug = $2 \
               AND NOT EXISTS(SELECT 1 FROM resource_quarantines rq WHERE rq.resource_id=r.id AND rq.lifted_at IS NULL \
                 AND (rq.release_version IS NULL OR rq.release_version=sr.version))",
        )
        .bind(owner)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?;
        if let Some(row) = active {
            return self.skill_detail_with_provenance(row, None).await;
        }
        let redirected = sqlx::query_as::<_, PublicSkillDetailRow>(
            "SELECT r.id, target_owner.slug AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, sr.manifest_json, \
                    r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                    replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name, FALSE AS live_private \
             FROM resource_redirects rr JOIN namespaces old_owner ON old_owner.id=rr.namespace_id \
             JOIN resources r ON r.id=rr.target_resource_id AND r.deleted_at IS NULL \
             JOIN namespaces target_owner ON target_owner.id=r.owner_namespace_id \
             JOIN skill_releases sr ON sr.resource_id=r.id AND sr.version=r.latest_release_version \
             LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
             LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
             WHERE old_owner.slug=$1 AND rr.kind='skill' AND rr.old_slug=$2 AND r.visibility='public' \
               AND NOT EXISTS(SELECT 1 FROM resource_quarantines rq WHERE rq.resource_id=r.id AND rq.lifted_at IS NULL \
                 AND (rq.release_version IS NULL OR rq.release_version=sr.version))",
        )
        .bind(owner)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "public skill not found"))?;
        self.skill_detail_with_provenance(redirected, Some(format!("@{owner}/{name}")))
            .await
    }

    async fn skill_detail_with_provenance(
        &self,
        row: PublicSkillDetailRow,
        redirected_from: Option<String>,
    ) -> Result<PublicSkillDetail, ApiError> {
        let resource_id = row.id;
        let mut detail = row.into_wire(redirected_from)?;
        detail.fork = self.skill_fork_provenance(resource_id).await?;
        Ok(detail)
    }

    pub(crate) async fn skill_fork_provenance(
        &self,
        resource_id: Uuid,
    ) -> Result<Option<SkillForkProvenance>, ApiError> {
        let row = sqlx::query_as::<_, (Uuid, Option<String>, String, Vec<u8>, Vec<u8>)>(
            "SELECT f.upstream_resource_id,COALESCE(n.slug,upstream.deleted_owner_slug),upstream.slug, \
                    f.created_from_revision_id,f.sync_base_revision_id \
             FROM skill_forks f \
             JOIN resources upstream ON upstream.id=f.upstream_resource_id \
             LEFT JOIN namespaces n ON n.id=upstream.owner_namespace_id \
             WHERE f.resource_id=$1 AND f.promotion_pending=FALSE",
        )
        .bind(resource_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?;
        let Some((upstream_resource_id, owner, name, created, sync_base)) = row else {
            return Ok(None);
        };
        let owner = owner.ok_or_else(|| {
            ApiError::new(
                ApiErrorCode::Internal,
                "stored fork upstream owner is unavailable",
            )
        })?;
        let created = crate::ingest::decode_32(&created, "stored fork creation revision")?;
        let sync_base = crate::ingest::decode_32(&sync_base, "stored fork sync base")?;
        Ok(Some(SkillForkProvenance {
            upstream_resource_id: upstream_resource_id.to_string(),
            upstream_locator: format!("@{owner}/{name}"),
            created_from_revision_id: hex::encode(created),
            sync_base_revision_id: hex::encode(sync_base),
        }))
    }
}

#[derive(sqlx::FromRow)]
struct PublicSkillSearchRow {
    id: Uuid,
    owner: String,
    name: String,
    description: String,
    generation: i64,
    version: Option<i64>,
    revision_id: Vec<u8>,
    deprecated: bool,
    replacement_id: Option<Uuid>,
    replacement_owner: Option<String>,
    replacement_name: Option<String>,
    live_private: bool,
}

impl PublicSkillSearchRow {
    fn into_wire(self) -> Result<PublicSkill, ApiError> {
        let deprecation = self.deprecated.then(|| SkillDeprecation {
            replacement_resource_id: self.replacement_id.map(|id| id.to_string()),
            replacement_locator: self
                .replacement_owner
                .zip(self.replacement_name)
                .map(|(owner, name)| format!("@{owner}/{name}")),
        });
        public_skill_from_parts(PublicSkillParts {
            id: self.id,
            owner: self.owner,
            name: self.name,
            description: self.description,
            generation: self.generation,
            version: self.version,
            live_private: self.live_private,
            revision_id: self.revision_id,
            deprecation,
        })
    }
}

// Keep the actual query row alias as the tuple; this type alias is referenced below by
// helper functions but never serialized.
#[derive(sqlx::FromRow)]
struct PublicSkillDetailRow {
    id: Uuid,
    owner: String,
    name: String,
    description: String,
    generation: i64,
    version: Option<i64>,
    revision_id: Vec<u8>,
    manifest_json: serde_json::Value,
    deprecated: bool,
    replacement_id: Option<Uuid>,
    replacement_owner: Option<String>,
    replacement_name: Option<String>,
    live_private: bool,
}

impl PublicSkillDetailRow {
    fn into_wire(self, redirected_from: Option<String>) -> Result<PublicSkillDetail, ApiError> {
        let deprecation = self.deprecated.then(|| SkillDeprecation {
            replacement_resource_id: self.replacement_id.map(|id| id.to_string()),
            replacement_locator: self
                .replacement_owner
                .zip(self.replacement_name)
                .map(|(owner, name)| format!("@{owner}/{name}")),
        });
        let skill = public_skill_from_parts(PublicSkillParts {
            id: self.id,
            owner: self.owner,
            name: self.name,
            description: self.description,
            generation: self.generation,
            version: self.version,
            live_private: self.live_private,
            revision_id: self.revision_id,
            deprecation,
        })?;
        let manifest = serde_json::from_value(self.manifest_json)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        Ok(PublicSkillDetail {
            skill,
            manifest,
            fork: None,
            redirected_from,
        })
    }
}

struct PublicSkillParts {
    id: Uuid,
    owner: String,
    name: String,
    description: String,
    generation: i64,
    version: Option<i64>,
    live_private: bool,
    revision_id: Vec<u8>,
    deprecation: Option<SkillDeprecation>,
}

fn public_skill_from_parts(parts: PublicSkillParts) -> Result<PublicSkill, ApiError> {
    let generation = u64::try_from(parts.generation)
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored generation is invalid"))?;
    let version =
        parts.version.map(u64::try_from).transpose().map_err(|_| {
            ApiError::new(ApiErrorCode::Internal, "stored release version is invalid")
        })?;
    if parts.live_private == version.is_some() {
        return Err(ApiError::new(
            ApiErrorCode::Internal,
            "catalog skill content shape is invalid",
        ));
    }
    let revision: [u8; 32] = parts
        .revision_id
        .try_into()
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored revision ID is invalid"))?;
    Ok(PublicSkill {
        resource_id: parts.id.to_string(),
        locator: format!("@{}/{}", parts.owner, parts.name),
        owner: parts.owner,
        name: parts.name,
        description: parts.description,
        generation,
        version,
        live_private: parts.live_private,
        revision_id: hex::encode(revision),
        deprecation: parts.deprecation,
    })
}

#[derive(Debug)]
struct SearchCursor {
    deprecated: bool,
    owner: String,
    name: String,
    resource_id: Uuid,
}

impl SearchCursor {
    fn from_parts(deprecated: bool, owner: &str, name: &str, resource_id: Uuid) -> Self {
        Self {
            deprecated,
            owner: owner.to_owned(),
            name: name.to_owned(),
            resource_id,
        }
    }

    fn from_row(row: &PublicSkillSearchRow) -> Self {
        Self::from_parts(row.deprecated, &row.owner, &row.name, row.id)
    }

    fn encode(&self) -> String {
        hex::encode(format!(
            "{}\0{}\0{}\0{}",
            u8::from(self.deprecated),
            self.owner,
            self.name,
            self.resource_id
        ))
    }

    fn decode(value: &str) -> Result<Self, ApiError> {
        let bytes = hex::decode(value)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor"))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor"))?;
        let mut parts = text.split('\0');
        let deprecated = parts.next().unwrap_or_default();
        let owner = parts.next().unwrap_or_default();
        let name = parts.next().unwrap_or_default();
        let id = parts.next().unwrap_or_default();
        if !matches!(deprecated, "0" | "1")
            || owner.is_empty()
            || name.is_empty()
            || id.is_empty()
            || parts.next().is_some()
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "invalid search cursor",
            ));
        }
        Ok(Self {
            deprecated: deprecated == "1",
            owner: owner.to_owned(),
            name: name.to_owned(),
            resource_id: Uuid::parse_str(id).map_err(|_| {
                ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor")
            })?,
        })
    }
}
