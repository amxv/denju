use std::str::FromStr;

use denju_core::{OperationId, ResourceId, ResourceKind, ResourceLocator};
use denju_wire::{
    ApiError, ApiErrorCode, PublicSkill, PublicSkillDetail, PublicSkillSearchResponse, RequestHash,
    SkillDeprecation, SkillForkProvenance, SnapshotDownload, SubscribedSkill, SubscriptionCatalog,
    SubscriptionContent, SubscriptionMutationKind, SubscriptionMutationRequest,
    SubscriptionMutationResponse, SubscriptionTarget, subscription_request_hash,
};
use uuid::Uuid;

use crate::{Registry, RegistryWake, identity_support::SubscriptionSubject, internal_api_error};

impl Registry {
    pub async fn subscription_target(
        &self,
        bearer: &str,
        locator: &str,
    ) -> Result<SubscriptionTarget, ApiError> {
        let subject = self.subscription_subject(bearer).await?;
        let parsed = ResourceLocator::from_str(locator)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let resolved = self.resolve_active_skill_locator(&parsed).await?;
        let row = sqlx::query_as::<_, (String, i64, bool, Option<Uuid>, Option<String>, Option<String>)>(
            "SELECT r.visibility,r.generation,r.deprecated_at IS NOT NULL,replacement.id, \
                    replacement_owner.slug,replacement.slug \
             FROM resources r \
             LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
             LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
             WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL",
        )
        .bind(resolved.resource_id)
        .fetch_one(&self.pool)
        .await
        .map_err(internal_api_error)?;
        let shared = match subject {
            SubscriptionSubject::Installation(_) => false,
            SubscriptionSubject::User(user_id) => sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM private_skill_shares WHERE recipient_user_id=$1 AND resource_id=$2)",
            )
            .bind(user_id)
            .bind(resolved.resource_id)
            .fetch_one(&self.pool)
            .await
            .map_err(internal_api_error)?,
        };
        if row.0 != "public" && !shared {
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
        Ok(SubscriptionTarget {
            resource_id: resolved.resource_id.to_string(),
            locator: format!("@{}/{}", resolved.owner, resolved.name),
            owner: resolved.owner,
            name: resolved.name,
            description: sqlx::query_scalar::<_, String>(
                "SELECT description FROM resources WHERE id=$1",
            )
            .bind(resolved.resource_id)
            .fetch_one(&self.pool)
            .await
            .map_err(internal_api_error)?,
            generation: u64::try_from(row.1)
                .map_err(|_| ApiError::new(ApiErrorCode::Internal, "generation is invalid"))?,
            live_private: shared,
            deprecation,
        })
    }

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
            if let Some(cursor) = cursor {
                let cursor = SearchCursor::decode(cursor)?;
                sqlx::query_as::<_, PublicSkillSearchRow>(
                    "SELECT r.id,n.slug AS owner,r.slug AS name,r.description,r.generation, \
                            CASE WHEN r.visibility='public' THEN sr.version ELSE NULL END AS version, \
                            CASE WHEN r.visibility='public' THEN sr.revision_id ELSE w.revision_id END AS revision_id, \
                            CASE WHEN r.visibility='public' THEN r.deprecated_at IS NOT NULL ELSE FALSE END AS deprecated, \
                            CASE WHEN r.visibility='public' THEN replacement.id ELSE NULL END AS replacement_id, \
                            CASE WHEN r.visibility='public' THEN replacement_owner.slug ELSE NULL END AS replacement_owner, \
                            CASE WHEN r.visibility='public' THEN replacement.slug ELSE NULL END AS replacement_name, \
                            r.visibility<>'public' AS live_private \
                     FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
                     LEFT JOIN skill_releases sr ON sr.resource_id=r.id AND sr.version=r.latest_release_version \
                     LEFT JOIN skill_private_workspaces w ON w.resource_id=r.id \
                     LEFT JOIN private_skill_shares ps ON ps.resource_id=r.id AND ps.recipient_user_id=$2 \
                     LEFT JOIN skill_forks f ON f.resource_id=r.id \
                     LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                     LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                     WHERE r.kind='skill' AND r.deleted_at IS NULL AND COALESCE(f.promotion_pending,FALSE)=FALSE \
                       AND ((r.visibility='public' AND sr.resource_id IS NOT NULL) OR \
                            (r.visibility<>'public' AND w.resource_id IS NOT NULL AND (r.owner_namespace_id=$3 OR ps.resource_id IS NOT NULL))) \
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
                .fetch_all(&self.pool)
                .await
                .map_err(internal_api_error)?
            } else {
                sqlx::query_as::<_, PublicSkillSearchRow>(
                    "SELECT r.id,n.slug AS owner,r.slug AS name,r.description,r.generation, \
                            CASE WHEN r.visibility='public' THEN sr.version ELSE NULL END AS version, \
                            CASE WHEN r.visibility='public' THEN sr.revision_id ELSE w.revision_id END AS revision_id, \
                            CASE WHEN r.visibility='public' THEN r.deprecated_at IS NOT NULL ELSE FALSE END AS deprecated, \
                            CASE WHEN r.visibility='public' THEN replacement.id ELSE NULL END AS replacement_id, \
                            CASE WHEN r.visibility='public' THEN replacement_owner.slug ELSE NULL END AS replacement_owner, \
                            CASE WHEN r.visibility='public' THEN replacement.slug ELSE NULL END AS replacement_name, \
                            r.visibility<>'public' AS live_private \
                     FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
                     LEFT JOIN skill_releases sr ON sr.resource_id=r.id AND sr.version=r.latest_release_version \
                     LEFT JOIN skill_private_workspaces w ON w.resource_id=r.id \
                     LEFT JOIN private_skill_shares ps ON ps.resource_id=r.id AND ps.recipient_user_id=$2 \
                     LEFT JOIN skill_forks f ON f.resource_id=r.id \
                     LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                     LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                     WHERE r.kind='skill' AND r.deleted_at IS NULL AND COALESCE(f.promotion_pending,FALSE)=FALSE \
                       AND ((r.visibility='public' AND sr.resource_id IS NOT NULL) OR \
                            (r.visibility<>'public' AND w.resource_id IS NOT NULL AND (r.owner_namespace_id=$3 OR ps.resource_id IS NOT NULL))) \
                       AND (r.slug ILIKE $1 OR n.slug ILIKE $1 OR r.description ILIKE $1) \
                     ORDER BY (CASE WHEN r.visibility='public' THEN r.deprecated_at IS NOT NULL ELSE FALSE END),n.slug,r.slug,r.id LIMIT $4",
                )
                .bind(&pattern)
                .bind(authority.user_id)
                .bind(authority.namespace_id)
                .bind(i64::from(limit) + 1)
                .fetch_all(&self.pool)
                .await
                .map_err(internal_api_error)?
            }
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
            let private = sqlx::query_as::<_, PublicSkillDetailRow>(
                "SELECT r.id,n.slug AS owner,r.slug AS name,r.description,r.generation,NULL::bigint AS version,w.revision_id,w.manifest_json, \
                        FALSE AS deprecated,NULL::uuid AS replacement_id,NULL::text AS replacement_owner,NULL::text AS replacement_name,TRUE AS live_private \
                 FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
                 JOIN skill_private_workspaces w ON w.resource_id=r.id \
                 LEFT JOIN private_skill_shares ps ON ps.resource_id=r.id AND ps.recipient_user_id=$3 \
                 LEFT JOIN skill_forks f ON f.resource_id=r.id \
                 WHERE n.slug=$1 AND r.slug=$2 AND r.kind='skill' AND r.deleted_at IS NULL AND r.visibility<>'public' \
                   AND COALESCE(f.promotion_pending,FALSE)=FALSE \
                   AND (r.owner_namespace_id=$4 OR ps.resource_id IS NOT NULL)",
            )
            .bind(locator.owner())
            .bind(locator.name())
            .bind(authority.user_id)
            .bind(authority.namespace_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal_api_error)?;
            if let Some(row) = private {
                return self.skill_detail_with_provenance(row, None).await;
            }
        }
        self.public_skill_detail(locator.owner(), locator.name())
            .await
    }

    pub async fn mutate_subscription(
        &self,
        bearer: &str,
        kind: SubscriptionMutationKind,
        request: &SubscriptionMutationRequest,
    ) -> Result<SubscriptionMutationResponse, ApiError> {
        let subject = self.subscription_subject(bearer).await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let resource_id = ResourceId::from_str(&request.resource_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let expected_hash = subscription_request_hash(
            kind,
            &request.operation_id,
            &request.resource_id,
            request.expected_generation,
            request.release_version,
            request.retain_on_delete,
        )
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        if supplied_hash != expected_hash {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequestHash,
                "request_hash does not match the canonical request payload",
            ));
        }

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let replay = match subject {
            SubscriptionSubject::Installation(installation_id) => {
                sqlx::query_as::<_, (Vec<u8>, Uuid, bool, Option<i64>, bool)>(
                    "SELECT request_hash, resource_id, subscribed, pinned_release_version, retain_on_delete FROM subscription_operations \
                     WHERE installation_id = $1 AND operation_id = $2",
                )
                .bind(installation_id)
                .bind(operation_id.as_uuid())
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal_api_error)?
            }
            SubscriptionSubject::User(user_id) => sqlx::query_as::<
                _,
                (Vec<u8>, Uuid, bool, Option<i64>, bool),
            >(
                "SELECT request_hash, resource_id, subscribed, pinned_release_version, retain_on_delete FROM account_subscription_operations \
                     WHERE user_id = $1 AND operation_id = $2",
            )
            .bind(user_id)
            .bind(operation_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_api_error)?,
        };
        if let Some((
            stored_hash,
            stored_resource,
            subscribed,
            pinned_release_version,
            retain_on_delete,
        )) = replay
        {
            if stored_hash.as_slice() != supplied_hash.as_bytes()
                || stored_resource != resource_id.as_uuid()
            {
                return Err(ApiError::new(
                    ApiErrorCode::OperationConflict,
                    "operation_id was already used with different request content",
                ));
            }
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(SubscriptionMutationResponse {
                resource_id: stored_resource.to_string(),
                subscribed,
                pinned_release_version: pinned_release_version
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored pin is invalid"))?,
                retain_on_delete,
            });
        }

        let resource = sqlx::query_as::<_, (i64, String, bool)>(
            "SELECT generation,visibility,deleted_at IS NOT NULL FROM resources WHERE id = $1 AND kind = 'skill'",
        )
        .bind(resource_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "skill not found"))?;
        let shared_private = match subject {
            SubscriptionSubject::User(user_id) if !resource.2 => {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM private_skill_shares WHERE recipient_user_id=$1 AND resource_id=$2)",
                )
                .bind(user_id)
                .bind(resource_id.as_uuid())
                .fetch_one(&mut *tx)
                .await
                .map_err(internal_api_error)?
            }
            _ => false,
        };
        if kind == SubscriptionMutationKind::Subscribe
            && (resource.2 || (resource.1 != "public" && !shared_private))
        {
            return Err(ApiError::new(
                ApiErrorCode::NotFound,
                "skill is not subscribable",
            ));
        }
        let generation = u64::try_from(resource.0)
            .map_err(|_| ApiError::new(ApiErrorCode::Internal, "resource generation is invalid"))?;
        if generation != request.expected_generation {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("resource generation changed to {generation}"),
            ));
        }

        if kind == SubscriptionMutationKind::Unsubscribe
            && (request.release_version.is_some() || request.retain_on_delete)
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "unsubscribe does not accept release or retention options",
            ));
        }
        if kind == SubscriptionMutationKind::Subscribe
            && resource.1 != "public"
            && shared_private
            && (request.release_version.is_some() || request.retain_on_delete)
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "private shared subscriptions follow the live workspace and do not accept release pins or retain-on-delete",
            ));
        }
        let pinned_release_version = request
            .release_version
            .map(i64::try_from)
            .transpose()
            .map_err(|_| {
                ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "release version exceeds database range",
                )
            })?;
        if let Some(version) = pinned_release_version {
            let exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM skill_releases WHERE resource_id=$1 AND version=$2)",
            )
            .bind(resource_id.as_uuid())
            .bind(version)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if !exists {
                return Err(ApiError::new(
                    ApiErrorCode::NotFound,
                    "requested release does not exist",
                ));
            }
        }

        let subscribed = kind == SubscriptionMutationKind::Subscribe;
        match (subject, kind) {
            (
                SubscriptionSubject::Installation(installation_id),
                SubscriptionMutationKind::Subscribe,
            ) => {
                sqlx::query(
                    "INSERT INTO installation_subscriptions (installation_id, resource_id, pinned_release_version, retain_on_delete) \
                     VALUES ($1, $2, $3, $4) ON CONFLICT(installation_id,resource_id) DO UPDATE \
                     SET pinned_release_version=excluded.pinned_release_version,retain_on_delete=excluded.retain_on_delete",
                )
                .bind(installation_id)
                .bind(resource_id.as_uuid())
                .bind(pinned_release_version)
                .bind(request.retain_on_delete)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            (
                SubscriptionSubject::Installation(installation_id),
                SubscriptionMutationKind::Unsubscribe,
            ) => {
                sqlx::query(
                    "DELETE FROM installation_subscriptions WHERE installation_id = $1 AND resource_id = $2",
                )
                .bind(installation_id)
                .bind(resource_id.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            (SubscriptionSubject::User(user_id), SubscriptionMutationKind::Subscribe) => {
                sqlx::query(
                    "INSERT INTO account_subscriptions (user_id, resource_id, pinned_release_version, retain_on_delete) VALUES ($1, $2, $3, $4) \
                     ON CONFLICT(user_id,resource_id) DO UPDATE SET pinned_release_version=excluded.pinned_release_version,retain_on_delete=excluded.retain_on_delete",
                )
                .bind(user_id)
                .bind(resource_id.as_uuid())
                .bind(pinned_release_version)
                .bind(request.retain_on_delete)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            (SubscriptionSubject::User(user_id), SubscriptionMutationKind::Unsubscribe) => {
                sqlx::query(
                    "DELETE FROM account_subscriptions WHERE user_id = $1 AND resource_id = $2",
                )
                .bind(user_id)
                .bind(resource_id.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
        }
        let action = match kind {
            SubscriptionMutationKind::Subscribe => "subscribe",
            SubscriptionMutationKind::Unsubscribe => "unsubscribe",
        };
        match subject {
            SubscriptionSubject::Installation(installation_id) => {
                sqlx::query(
                    "INSERT INTO subscription_operations \
                     (installation_id, operation_id, request_hash, action, resource_id, subscribed, pinned_release_version, retain_on_delete) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                )
                .bind(installation_id)
                .bind(operation_id.as_uuid())
                .bind(supplied_hash.as_bytes().as_slice())
                .bind(action)
                .bind(resource_id.as_uuid())
                .bind(subscribed)
                .bind(pinned_release_version)
                .bind(request.retain_on_delete)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            SubscriptionSubject::User(user_id) => {
                sqlx::query(
                    "INSERT INTO account_subscription_operations \
                     (user_id, operation_id, request_hash, action, resource_id, subscribed, pinned_release_version, retain_on_delete) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                )
                .bind(user_id)
                .bind(operation_id.as_uuid())
                .bind(supplied_hash.as_bytes().as_slice())
                .bind(action)
                .bind(resource_id.as_uuid())
                .bind(subscribed)
                .bind(pinned_release_version)
                .bind(request.retain_on_delete)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
        }
        tx.commit().await.map_err(internal_api_error)?;
        // Subscription membership changes the per-connection reverse watch set. Wake local
        // listeners without exposing any resource ID so a long-lived daemon reconnects and
        // rebuilds that disposable index from authoritative subscription rows.
        let _ = self.wake_tx.send(RegistryWake::ResyncAll);

        Ok(SubscriptionMutationResponse {
            resource_id: resource_id.to_string(),
            subscribed,
            pinned_release_version: request.release_version,
            retain_on_delete: request.retain_on_delete,
        })
    }

    pub async fn subscription_catalog(
        &self,
        bearer: &str,
    ) -> Result<SubscriptionCatalog, ApiError> {
        let subject = self.subscription_subject(bearer).await?;
        let rows = match subject {
            SubscriptionSubject::Installation(installation_id) => {
                sqlx::query_as::<_, SubscriptionRow>(
                    "SELECT r.id, COALESCE(n.slug,r.deleted_owner_slug) AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, \
                            sr.manifest_json, sr.snapshot_key, sr.snapshot_sha256, sr.snapshot_size, s.pinned_release_version, \
                            s.retain_on_delete, r.deleted_at IS NOT NULL AS retained_after_delete, \
                            r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                            replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name, FALSE AS live_private \
                     FROM installation_subscriptions s \
                     JOIN resources r ON r.id = s.resource_id \
                     LEFT JOIN namespaces n ON n.id = r.owner_namespace_id \
                     LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                     LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                     JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = CASE \
                         WHEN r.deleted_at IS NOT NULL THEN r.tombstone_release_version \
                         ELSE COALESCE(s.pinned_release_version,r.latest_release_version) END \
                     WHERE s.installation_id = $1 AND r.kind = 'skill' AND ( \
                         (r.deleted_at IS NULL AND r.visibility = 'public') OR \
                         (r.deleted_at IS NOT NULL AND s.retain_on_delete AND r.tombstone_release_version IS NOT NULL)) \
                     ORDER BY COALESCE(n.slug,r.deleted_owner_slug), r.slug, r.id",
                )
                .bind(installation_id)
                .fetch_all(&self.pool)
                .await
                .map_err(internal_api_error)?
            }
            SubscriptionSubject::User(user_id) => {
                sqlx::query_as::<_, SubscriptionRow>(
                    "SELECT r.id, COALESCE(n.slug,r.deleted_owner_slug) AS owner, r.slug AS name, r.description, r.generation, \
                            CASE WHEN ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN NULL ELSE sr.version END AS version, \
                            CASE WHEN ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN w.revision_id ELSE sr.revision_id END AS revision_id, \
                            CASE WHEN ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN w.manifest_json ELSE sr.manifest_json END AS manifest_json, \
                            CASE WHEN ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN w.snapshot_key ELSE sr.snapshot_key END AS snapshot_key, \
                            CASE WHEN ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN w.snapshot_sha256 ELSE sr.snapshot_sha256 END AS snapshot_sha256, \
                            CASE WHEN ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN w.snapshot_size ELSE sr.snapshot_size END AS snapshot_size, s.pinned_release_version, \
                            s.retain_on_delete, r.deleted_at IS NOT NULL AS retained_after_delete, \
                            r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                            replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name, \
                            (ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete) AS live_private \
                     FROM account_subscriptions s \
                     JOIN resources r ON r.id = s.resource_id \
                     LEFT JOIN namespaces n ON n.id = r.owner_namespace_id \
                     LEFT JOIN private_skill_shares ps ON ps.resource_id=r.id AND ps.recipient_user_id=s.user_id \
                     LEFT JOIN skill_private_workspaces w ON w.resource_id=r.id \
                     LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                     LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                     LEFT JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = CASE \
                         WHEN r.deleted_at IS NOT NULL THEN r.tombstone_release_version \
                         ELSE COALESCE(s.pinned_release_version,r.latest_release_version) END \
                     WHERE s.user_id = $1 AND r.kind = 'skill' AND ( \
                         (r.deleted_at IS NULL AND (r.visibility = 'public' OR ps.resource_id IS NOT NULL)) OR \
                         (r.deleted_at IS NOT NULL AND s.retain_on_delete AND r.tombstone_release_version IS NOT NULL)) \
                     ORDER BY COALESCE(n.slug,r.deleted_owner_slug), r.slug, r.id",
                )
                .bind(user_id)
                .fetch_all(&self.pool)
                .await
                .map_err(internal_api_error)?
            }
        };

        let mut skills = Vec::with_capacity(rows.len());
        for row in rows {
            let snapshot_url = self
                .objects
                .presign_get(&row.snapshot_key)
                .await
                .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
            skills.push(row.into_wire(snapshot_url)?);
        }
        Ok(SubscriptionCatalog { skills })
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
             WHERE r.visibility = 'public' AND r.kind = 'skill' AND r.deleted_at IS NULL AND n.slug = $1 AND r.slug = $2",
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
             WHERE old_owner.slug=$1 AND rr.kind='skill' AND rr.old_slug=$2 AND r.visibility='public'",
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

    async fn skill_fork_provenance(
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

#[derive(sqlx::FromRow)]
struct SubscriptionRow {
    id: Uuid,
    owner: String,
    name: String,
    description: String,
    generation: i64,
    version: Option<i64>,
    revision_id: Vec<u8>,
    manifest_json: serde_json::Value,
    snapshot_key: String,
    snapshot_sha256: Vec<u8>,
    snapshot_size: i64,
    pinned_release_version: Option<i64>,
    retain_on_delete: bool,
    retained_after_delete: bool,
    deprecated: bool,
    replacement_id: Option<Uuid>,
    replacement_owner: Option<String>,
    replacement_name: Option<String>,
    live_private: bool,
}

impl SubscriptionRow {
    fn into_wire(self, snapshot_url: String) -> Result<SubscribedSkill, ApiError> {
        let deprecation = self.deprecated.then(|| SkillDeprecation {
            replacement_resource_id: self.replacement_id.map(|id| id.to_string()),
            replacement_locator: self
                .replacement_owner
                .zip(self.replacement_name)
                .map(|(owner, name)| format!("@{owner}/{name}")),
        });
        let generation = u64::try_from(self.generation)
            .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored generation is invalid"))?;
        let revision: [u8; 32] = self
            .revision_id
            .try_into()
            .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored revision ID is invalid"))?;
        let content = if self.live_private {
            if self.version.is_some() || self.retained_after_delete {
                return Err(ApiError::new(
                    ApiErrorCode::Internal,
                    "stored private subscription content shape is invalid",
                ));
            }
            SubscriptionContent::PrivateWorkspace
        } else {
            let version = u64::try_from(self.version.ok_or_else(|| {
                ApiError::new(ApiErrorCode::Internal, "stored release version is missing")
            })?)
            .map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "stored release version is invalid")
            })?;
            SubscriptionContent::Release {
                version,
                following_latest: !self.retained_after_delete
                    && self.pinned_release_version.is_none(),
            }
        };
        let manifest = serde_json::from_value(self.manifest_json)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let sha: [u8; 32] = self.snapshot_sha256.try_into().map_err(|_| {
            ApiError::new(
                ApiErrorCode::Internal,
                "stored snapshot checksum is invalid",
            )
        })?;
        let size_bytes = u64::try_from(self.snapshot_size).map_err(|_| {
            ApiError::new(ApiErrorCode::Internal, "stored snapshot size is invalid")
        })?;
        Ok(SubscribedSkill {
            resource_id: self.id.to_string(),
            locator: format!("@{}/{}", self.owner, self.name),
            owner: self.owner,
            name: self.name,
            description: self.description,
            generation,
            revision_id: hex::encode(revision),
            deprecation,
            content,
            manifest,
            snapshot: SnapshotDownload {
                sha256: hex::encode(sha),
                size_bytes,
                url: snapshot_url,
            },
            retained_after_delete: self.retained_after_delete,
            retain_on_delete: self.retain_on_delete,
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
