use std::str::FromStr;

use denju_core::{OperationId, ResourceId};
use denju_wire::{
    ApiError, ApiErrorCode, QuarantinedResource, RequestHash, SkillDeprecation, SnapshotDownload,
    SubscribedSkill, SubscriptionCatalog, SubscriptionContent, SubscriptionMutationKind,
    SubscriptionMutationRequest, SubscriptionMutationResponse, subscription_request_hash,
};
use uuid::Uuid;

use crate::{
    Registry, RegistryWake,
    admin::{active_quarantines_for_resources, effective_quarantine_tx},
    identity_support::SubscriptionSubject,
    internal_api_error,
    lifecycle::generation_u64,
    team_access::user_is_team_member,
};

impl Registry {
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

        let mut tx = match subject {
            SubscriptionSubject::Installation(installation_id) => {
                self.begin_installation_actor_tx(installation_id).await?
            }
            SubscriptionSubject::User(user_id) => self.begin_actor_tx(user_id).await?,
        };
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

        let resource = sqlx::query_as::<_, (i64, String, bool, Uuid, Option<i64>, String)>(
            "SELECT r.generation,r.visibility,r.deleted_at IS NOT NULL,r.owner_namespace_id, \
                    r.latest_release_version,n.kind \
             FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
             WHERE r.id = $1 AND r.kind = 'skill'",
        )
        .bind(resource_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "skill not found"))?;
        let shared = match subject {
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
        let personal_live_share = shared && resource.5 == "user";
        let team_private = match subject {
            SubscriptionSubject::User(user_id)
                if !resource.2 && resource.5 == "team" && resource.4.is_some() =>
            {
                shared || user_is_team_member(&mut tx, user_id, resource.3).await?
            }
            _ => false,
        };
        if kind == SubscriptionMutationKind::Subscribe
            && (resource.2 || (resource.1 != "public" && !personal_live_share && !team_private))
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
            && personal_live_share
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
        if kind == SubscriptionMutationKind::Subscribe {
            let desired_release = if personal_live_share {
                None
            } else {
                pinned_release_version.or(resource.4)
            };
            if effective_quarantine_tx(&mut tx, resource_id.as_uuid(), desired_release)
                .await?
                .is_some()
            {
                return Err(ApiError::new(
                    ApiErrorCode::NotFound,
                    "skill is quarantined",
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
        let mut tx = match subject {
            SubscriptionSubject::Installation(installation_id) => {
                self.begin_installation_actor_tx(installation_id).await?
            }
            SubscriptionSubject::User(user_id) => self.begin_actor_tx(user_id).await?,
        };
        // Resolve quarantine tombstones before joining content-bearing release/workspace rows.
        // RLS intentionally hides those rows as soon as quarantine becomes effective; using an
        // inner join to discover desired state would otherwise make the relationship look deleted
        // and prevent clients from preserving/removing the local projection safely.
        let quarantine_candidates = match subject {
            SubscriptionSubject::Installation(installation_id) => {
                sqlx::query_as::<_, (Uuid, String, String, Option<i64>, bool)>(
                    "SELECT r.id,COALESCE(n.slug,r.deleted_owner_slug),r.slug, \
                            CASE WHEN r.deleted_at IS NOT NULL THEN r.tombstone_release_version \
                                 ELSE COALESCE(s.pinned_release_version,r.latest_release_version) END, \
                            FALSE \
                     FROM installation_subscriptions s \
                     JOIN resources r ON r.id=s.resource_id \
                     LEFT JOIN namespaces n ON n.id=r.owner_namespace_id \
                     WHERE s.installation_id=$1 AND r.kind='skill' AND ( \
                         (r.deleted_at IS NULL AND r.visibility='public') OR \
                         (r.deleted_at IS NOT NULL AND s.retain_on_delete AND r.tombstone_release_version IS NOT NULL)) \
                     ORDER BY COALESCE(n.slug,r.deleted_owner_slug),r.slug,r.id",
                )
                .bind(installation_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(internal_api_error)?
            }
            SubscriptionSubject::User(user_id) => {
                sqlx::query_as::<_, (Uuid, String, String, Option<i64>, bool)>(
                    "SELECT r.id,COALESCE(n.slug,r.deleted_owner_slug),r.slug, \
                            CASE WHEN n.kind='user' AND ps.resource_id IS NOT NULL AND r.deleted_at IS NULL \
                                           AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN NULL \
                                 WHEN r.deleted_at IS NOT NULL THEN r.tombstone_release_version \
                                 ELSE COALESCE(s.pinned_release_version,r.latest_release_version) END, \
                            (n.kind='user' AND ps.resource_id IS NOT NULL AND r.deleted_at IS NULL \
                                AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete) \
                     FROM account_subscriptions s \
                     JOIN resources r ON r.id=s.resource_id \
                     LEFT JOIN namespaces n ON n.id=r.owner_namespace_id \
                     LEFT JOIN private_skill_shares ps ON ps.resource_id=r.id AND ps.recipient_user_id=s.user_id \
                     LEFT JOIN team_memberships tm ON tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=s.user_id \
                     WHERE s.user_id=$1 AND r.kind='skill' AND ( \
                         (r.deleted_at IS NULL AND (r.visibility='public' OR \
                           (ps.resource_id IS NOT NULL AND (n.kind='user' OR r.latest_release_version IS NOT NULL)) \
                           OR (tm.user_id IS NOT NULL AND r.latest_release_version IS NOT NULL))) OR \
                         (r.deleted_at IS NOT NULL AND s.retain_on_delete AND r.tombstone_release_version IS NOT NULL)) \
                     ORDER BY COALESCE(n.slug,r.deleted_owner_slug),r.slug,r.id",
                )
                .bind(user_id)
                .fetch_all(&mut *tx)
                .await
                .map_err(internal_api_error)?
            }
        };
        let candidate_resource_ids = quarantine_candidates
            .iter()
            .map(|candidate| candidate.0)
            .collect::<Vec<_>>();
        let active_quarantines =
            active_quarantines_for_resources(&mut tx, &candidate_resource_ids).await?;
        let mut quarantined = Vec::new();
        let mut quarantined_ids = std::collections::BTreeSet::new();
        for (resource_id, owner, name, desired_release, live_private) in quarantine_candidates {
            let Some(active) = active_quarantines.get(&resource_id).and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.release_version.is_none())
                    .or_else(|| {
                        (!live_private).then(|| {
                            desired_release.and_then(|version| {
                                entries
                                    .iter()
                                    .find(|entry| entry.release_version == Some(version))
                            })
                        })?
                    })
            }) else {
                continue;
            };
            quarantined_ids.insert(resource_id);
            quarantined.push(QuarantinedResource {
                resource_id: resource_id.to_string(),
                locator: format!("@{owner}/{name}"),
                release_version: active.release_version.map(generation_u64).transpose()?,
                // Content RLS intentionally withholds the quarantined release/workspace row.
                // Existing clients already know their local desired/materialized revision and
                // use release scope below to decide which bytes must be preserved.
                revision_id: None,
                reason: active.reason.clone(),
            });
        }
        let quarantined_resource_ids = quarantined_ids.iter().copied().collect::<Vec<_>>();
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
                       AND NOT (r.id = ANY($2::uuid[])) \
                     ORDER BY COALESCE(n.slug,r.deleted_owner_slug), r.slug, r.id",
                )
                .bind(installation_id)
                .bind(&quarantined_resource_ids)
                .fetch_all(&mut *tx)
                .await
                .map_err(internal_api_error)?
            }
            SubscriptionSubject::User(user_id) => {
                sqlx::query_as::<_, SubscriptionRow>(
                    "SELECT r.id, COALESCE(n.slug,r.deleted_owner_slug) AS owner, r.slug AS name, \
                            CASE WHEN n.kind='user' AND ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN w.description ELSE r.description END AS description, r.generation, \
                            CASE WHEN n.kind='user' AND ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN NULL ELSE sr.version END AS version, \
                            CASE WHEN n.kind='user' AND ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN w.revision_id ELSE sr.revision_id END AS revision_id, \
                            CASE WHEN n.kind='user' AND ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN w.manifest_json ELSE sr.manifest_json END AS manifest_json, \
                            CASE WHEN n.kind='user' AND ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN w.snapshot_key ELSE sr.snapshot_key END AS snapshot_key, \
                            CASE WHEN n.kind='user' AND ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN w.snapshot_sha256 ELSE sr.snapshot_sha256 END AS snapshot_sha256, \
                            CASE WHEN n.kind='user' AND ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete THEN w.snapshot_size ELSE sr.snapshot_size END AS snapshot_size, s.pinned_release_version, \
                            s.retain_on_delete, r.deleted_at IS NOT NULL AS retained_after_delete, \
                            r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                            replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name, \
                            (n.kind='user' AND ps.resource_id IS NOT NULL AND r.deleted_at IS NULL AND s.pinned_release_version IS NULL AND NOT s.retain_on_delete) AS live_private \
                     FROM account_subscriptions s \
                     JOIN resources r ON r.id = s.resource_id \
                     LEFT JOIN namespaces n ON n.id = r.owner_namespace_id \
                     LEFT JOIN users owner_user ON owner_user.namespace_id=r.owner_namespace_id \
                     LEFT JOIN private_skill_shares ps ON ps.resource_id=r.id AND ps.recipient_user_id=s.user_id \
                     LEFT JOIN skill_private_workspaces w ON w.resource_id=r.id AND w.workspace_user_id=owner_user.id \
                     LEFT JOIN team_memberships tm ON tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=s.user_id \
                     LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                     LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                     LEFT JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = CASE \
                         WHEN r.deleted_at IS NOT NULL THEN r.tombstone_release_version \
                         ELSE COALESCE(s.pinned_release_version,r.latest_release_version) END \
                     WHERE s.user_id = $1 AND r.kind = 'skill' AND ( \
                         (r.deleted_at IS NULL AND (r.visibility = 'public' OR \
                           (ps.resource_id IS NOT NULL AND (n.kind='user' OR r.latest_release_version IS NOT NULL)) \
                           OR (tm.user_id IS NOT NULL AND r.latest_release_version IS NOT NULL))) OR \
                         (r.deleted_at IS NOT NULL AND s.retain_on_delete AND r.tombstone_release_version IS NOT NULL)) \
                       AND NOT (r.id = ANY($2::uuid[])) \
                     ORDER BY COALESCE(n.slug,r.deleted_owner_slug), r.slug, r.id",
                )
                .bind(user_id)
                .bind(&quarantined_resource_ids)
                .fetch_all(&mut *tx)
                .await
                .map_err(internal_api_error)?
            }
        };
        tx.commit().await.map_err(internal_api_error)?;
        let mut skills = Vec::with_capacity(rows.len());
        for row in rows {
            if quarantined_ids.contains(&row.id) {
                continue;
            }
            let snapshot_url = self
                .objects
                .presign_get(&row.snapshot_key)
                .await
                .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
            skills.push(row.into_wire(snapshot_url)?);
        }
        Ok(SubscriptionCatalog {
            skills,
            quarantined,
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
