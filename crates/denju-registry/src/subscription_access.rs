use std::str::FromStr;

use denju_core::ResourceLocator;
use denju_wire::{
    ApiError, ApiErrorCode, PublicSkill, PublicSkillDetail, PublicSkillManifest, SkillDeprecation,
    SubscriptionTarget,
};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    Registry, admin::active_quarantine, identity_support::SubscriptionSubject, internal_api_error,
    lifecycle::resolve_active_skill_locator_tx,
};

impl Registry {
    pub(crate) async fn team_skill_detail(
        &self,
        bearer: &str,
        locator: &ResourceLocator,
    ) -> Result<Option<PublicSkillDetail>, ApiError> {
        let authority = self.user_authority(bearer, "skills:read").await?;
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        let resolved = resolve_active_skill_locator_tx(&mut tx, locator).await?;
        let row = sqlx::query(
            "SELECT r.id,n.slug,r.slug,COALESCE(w.description,r.description),r.generation, \
                    CASE WHEN w.resource_id IS NOT NULL THEN NULL::bigint ELSE sr.version END, \
                    COALESCE(w.revision_id,sr.revision_id),COALESCE(w.manifest_json,sr.manifest_json), \
                    CASE WHEN w.resource_id IS NOT NULL THEN FALSE ELSE r.deprecated_at IS NOT NULL END, \
                    CASE WHEN w.resource_id IS NOT NULL THEN NULL::uuid ELSE replacement.id END, \
                    CASE WHEN w.resource_id IS NOT NULL THEN NULL::text ELSE replacement_owner.slug END, \
                    CASE WHEN w.resource_id IS NOT NULL THEN NULL::text ELSE replacement.slug END, \
                    w.resource_id IS NOT NULL \
             FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id AND n.kind='team' \
             LEFT JOIN skill_private_workspaces w ON w.resource_id=r.id AND w.workspace_user_id=$2 \
             LEFT JOIN team_memberships tm ON tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=$2 \
             LEFT JOIN private_skill_shares ps ON ps.resource_id=r.id AND ps.recipient_user_id=$2 \
             LEFT JOIN skill_releases sr ON sr.resource_id=r.id AND sr.version=r.latest_release_version \
             LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
             LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
             WHERE r.id=$1 AND r.kind='skill' AND r.visibility<>'public' AND r.deleted_at IS NULL \
               AND (w.resource_id IS NOT NULL OR (sr.resource_id IS NOT NULL AND (tm.user_id IS NOT NULL OR ps.resource_id IS NOT NULL)))",
        )
        .bind(resolved.resource_id)
        .bind(authority.user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        tx.commit().await.map_err(internal_api_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let live_private = row.get::<bool, _>(12);
        let version = row.get::<Option<i64>, _>(5);
        if active_quarantine(
            &self.pool,
            resolved.resource_id,
            if live_private { None } else { version },
        )
        .await?
        .is_some()
        {
            return Ok(None);
        }
        let generation = u64::try_from(row.get::<i64, _>(4))
            .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored generation is invalid"))?;
        let version = version.map(u64::try_from).transpose().map_err(|_| {
            ApiError::new(ApiErrorCode::Internal, "stored release version is invalid")
        })?;
        let revision = crate::ingest::decode_32(&row.get::<Vec<u8>, _>(6), "stored revision ID")?;
        let deprecation = row.get::<bool, _>(8).then(|| SkillDeprecation {
            replacement_resource_id: row.get::<Option<Uuid>, _>(9).map(|id| id.to_string()),
            replacement_locator: row
                .get::<Option<String>, _>(10)
                .zip(row.get::<Option<String>, _>(11))
                .map(|(owner, name)| format!("@{owner}/{name}")),
        });
        let mut detail = PublicSkillDetail {
            skill: PublicSkill {
                resource_id: resolved.resource_id.to_string(),
                locator: format!("@{}/{}", resolved.owner, resolved.name),
                owner: row.get(1),
                name: row.get(2),
                description: row.get(3),
                generation,
                version,
                live_private,
                revision_id: hex::encode(revision),
                deprecation,
            },
            manifest: serde_json::from_value::<PublicSkillManifest>(row.get(7))
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?,
            fork: None,
            redirected_from: (resolved.owner != locator.owner() || resolved.name != locator.name())
                .then(|| locator.to_string()),
        };
        detail.fork = self.skill_fork_provenance(resolved.resource_id).await?;
        Ok(Some(detail))
    }

    pub async fn subscription_target(
        &self,
        bearer: &str,
        locator: &str,
    ) -> Result<SubscriptionTarget, ApiError> {
        let subject = self.subscription_subject(bearer).await?;
        let parsed = ResourceLocator::from_str(locator)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let mut tx = match subject {
            SubscriptionSubject::Installation(installation_id) => {
                self.begin_installation_actor_tx(installation_id).await?
            }
            SubscriptionSubject::User(user_id) => self.begin_actor_tx(user_id).await?,
        };
        let resolved = resolve_active_skill_locator_tx(&mut tx, &parsed).await?;
        let row = sqlx::query_as::<
            _,
            (
                String,
                i64,
                bool,
                Option<Uuid>,
                Option<String>,
                Option<String>,
                Uuid,
                Option<i64>,
                String,
            ),
        >(
            "SELECT r.visibility,r.generation,r.deprecated_at IS NOT NULL,replacement.id, \
                    replacement_owner.slug,replacement.slug,r.owner_namespace_id,r.latest_release_version,n.kind \
             FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
             LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
             LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
             WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL",
        )
        .bind(resolved.resource_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let shared = match subject {
            SubscriptionSubject::Installation(_) => false,
            SubscriptionSubject::User(user_id) => sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM private_skill_shares WHERE recipient_user_id=$1 AND resource_id=$2)",
            )
            .bind(user_id)
            .bind(resolved.resource_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?,
        };
        let personal_live_share = shared && row.8 == "user";
        let team_release = match subject {
            SubscriptionSubject::User(user_id) if row.8 == "team" && row.7.is_some() => {
                shared
                    || sqlx::query_scalar::<_, bool>(
                        "SELECT EXISTS(SELECT 1 FROM team_memberships WHERE team_namespace_id=$1 AND user_id=$2)",
                    )
                    .bind(row.6)
                    .bind(user_id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(internal_api_error)?
            }
            _ => false,
        };
        if row.0 != "public" && !personal_live_share && !team_release {
            return Err(ApiError::new(
                ApiErrorCode::NotFound,
                "skill is not subscribable",
            ));
        }
        let deprecation = row.2.then(|| SkillDeprecation {
            replacement_resource_id: row.3.map(|id| id.to_string()),
            replacement_locator: row
                .4
                .zip(row.5)
                .map(|(owner, name)| format!("@{owner}/{name}")),
        });
        let description =
            sqlx::query_scalar::<_, String>("SELECT description FROM resources WHERE id=$1")
                .bind(resolved.resource_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(internal_api_error)?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(SubscriptionTarget {
            resource_id: resolved.resource_id.to_string(),
            locator: format!("@{}/{}", resolved.owner, resolved.name),
            owner: resolved.owner,
            name: resolved.name,
            description,
            generation: u64::try_from(row.1)
                .map_err(|_| ApiError::new(ApiErrorCode::Internal, "generation is invalid"))?,
            live_private: personal_live_share,
            deprecation,
        })
    }
}
