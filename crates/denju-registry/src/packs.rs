use std::{collections::BTreeSet, str::FromStr};

use denju_core::{OperationId, ResourceId, ResourceKind, ResourceLocator};
use denju_wire::{
    ApiError, ApiErrorCode, PackCreateRequest, PackCreateResponse, PackDetail, PackMemberTarget,
    PackMutationKind, PackMutationRequest, PackMutationResponse, PackPublishRequest,
    PackRequirement, PackRequirementKind, PackRequirementSource, PackSubscriptionCatalog,
    PackSubscriptionMutationKind, PackSubscriptionRequest, PackSubscriptionResponse, PackSummary,
    RequestHash, pack_create_request_hash, pack_mutation_request_hash, pack_publish_request_hash,
    pack_subscription_request_hash,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    Registry,
    identity_support::SubscriptionSubject,
    internal_api_error,
    lifecycle::{generation_u64, next_generation},
    outbox::enqueue_resource_wake,
    pack_drain::lock_and_catch_up_pack,
    pack_storage::{
        insert_pack_revision, load_owned_pack_for_update, load_pack_by_locator,
        load_pack_revision_members, order_resolved_members, pack_summary, resolve_all_members,
        resolve_member,
    },
    team_access::{authorize_namespace_publish, authorize_resource_publish, user_is_team_member},
};

impl Registry {
    pub async fn create_pack(
        &self,
        bearer: &str,
        request: &PackCreateRequest,
    ) -> Result<PackCreateResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let locator = format!("@{}/packs/{}", request.owner, request.name)
            .parse::<ResourceLocator>()
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        if locator.kind() != ResourceKind::Pack {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "expected a pack locator",
            ));
        }
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied_hash = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied_hash,
            pack_create_request_hash(&request.operation_id, &request.owner, &request.name)
                .map_err(hash_error)?,
        )?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let owner_authority =
            authorize_namespace_publish(&mut tx, &authority, locator.owner()).await?;
        if let Some(outcome) = replay_pack_operation::<PackCreateResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            "create",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let occupied = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE owner_namespace_id=$1 AND kind='pack' AND slug=$2 AND deleted_at IS NULL)",
        )
        .bind(owner_authority.namespace_id)
        .bind(locator.name())
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if occupied {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "pack name is already in use",
            ));
        }
        let resource_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO resources (id,owner_namespace_id,slug,kind,visibility,description,generation,latest_release_version) \
             VALUES ($1,$2,$3,'pack','private','',1,NULL)",
        )
        .bind(resource_id)
        .bind(owner_authority.namespace_id)
        .bind(locator.name())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query("INSERT INTO pack_state (resource_id,current_version) VALUES ($1,1)")
            .bind(resource_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("INSERT INTO pack_revisions (pack_resource_id,version) VALUES ($1,1)")
            .bind(resource_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        let outcome = PackCreateResponse {
            pack: PackSummary {
                resource_id: resource_id.to_string(),
                locator: locator.to_string(),
                generation: 1,
                version: 1,
                visibility: "private".to_owned(),
                member_count: 0,
                degraded: false,
            },
        };
        record_pack_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            resource_id,
            "create",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn mutate_pack(
        &self,
        bearer: &str,
        kind: PackMutationKind,
        request: &PackMutationRequest,
    ) -> Result<PackMutationResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        if request.members.is_empty() {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "pack mutation requires at least one skill",
            ));
        }
        validate_unique_members(&request.members)?;
        let operation_id = parse_operation(&request.operation_id)?;
        let resource_id = parse_resource(&request.resource_id)?;
        let supplied_hash = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied_hash,
            pack_mutation_request_hash(
                kind,
                &request.operation_id,
                &request.resource_id,
                request.expected_generation,
                &request.members,
            )
            .map_err(hash_error)?,
        )?;
        let action = match kind {
            PackMutationKind::Add => "add",
            PackMutationKind::Remove => "remove",
        };
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_pack_operation::<PackMutationResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            action,
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let resource_authority =
            authorize_resource_publish(&mut tx, &authority, resource_id.as_uuid()).await?;
        let mut pack = load_owned_pack_for_update(
            &mut tx,
            resource_id.as_uuid(),
            resource_authority.namespace_id,
        )
        .await?;
        let extra_skill_ids = request
            .members
            .iter()
            .map(|member| parse_resource(&member.resource_id).map(|id| id.as_uuid()))
            .collect::<Result<Vec<_>, _>>()?;
        let caught_up = lock_and_catch_up_pack(&mut tx, &mut pack, &extra_skill_ids).await?;
        if let Some(generation) = caught_up.last().copied() {
            enqueue_resource_wake(&mut tx, pack.id, generation).await?;
            tx.commit().await.map_err(internal_api_error)?;
            let _ = self.drain_outbox(32).await;
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!(
                    "pack advanced through pending skill releases to generation {generation}; retry from current state"
                ),
            ));
        }
        ensure_generation(pack.generation, request.expected_generation)?;
        let mut resolved_members =
            load_pack_revision_members(&mut tx, pack.id, pack.current_version).await?;
        let follow_after_event_id =
            sqlx::query_scalar::<_, i64>("SELECT COALESCE(max(id),0) FROM authority_events")
                .fetch_one(&mut *tx)
                .await
                .map_err(internal_api_error)?;
        let mut changed = false;
        for target in &request.members {
            let skill_id = parse_resource(&target.resource_id)?;
            match kind {
                PackMutationKind::Add => {
                    // Validate readability and pin before authored intent changes. The full
                    // resolved member is changed only when this authored intent changes;
                    // unrelated members preserve the exact revision already recorded by the
                    // current pack version.
                    let resolved = resolve_member(
                        &mut tx,
                        authority.user_id,
                        resource_authority.namespace_id,
                        pack.visibility == "public",
                        resource_authority.is_team,
                        skill_id.as_uuid(),
                        target.release_version,
                    )
                    .await?;
                    let pinned = i64_version(target.release_version)?;
                    let previous = sqlx::query_scalar::<_, Option<i64>>(
                        "SELECT pinned_release_version FROM pack_members WHERE pack_resource_id=$1 AND skill_resource_id=$2",
                    )
                    .bind(pack.id)
                    .bind(skill_id.as_uuid())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(internal_api_error)?;
                    if previous != Some(pinned) {
                        sqlx::query(
                            "INSERT INTO pack_members (pack_resource_id,skill_resource_id,pinned_release_version,follow_after_event_id) \
                             VALUES ($1,$2,$3,$4) ON CONFLICT(pack_resource_id,skill_resource_id) DO UPDATE SET \
                             pinned_release_version=excluded.pinned_release_version,follow_after_event_id=excluded.follow_after_event_id,updated_at=now()",
                        )
                        .bind(pack.id)
                        .bind(skill_id.as_uuid())
                        .bind(pinned)
                        .bind(follow_after_event_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(internal_api_error)?;
                        if let Some(existing) = resolved_members
                            .iter_mut()
                            .find(|member| member.skill_resource_id == skill_id.as_uuid())
                        {
                            *existing = resolved;
                        } else {
                            resolved_members.push(resolved);
                        }
                        changed = true;
                    }
                }
                PackMutationKind::Remove => {
                    let affected = sqlx::query(
                        "DELETE FROM pack_members WHERE pack_resource_id=$1 AND skill_resource_id=$2",
                    )
                    .bind(pack.id)
                    .bind(skill_id.as_uuid())
                    .execute(&mut *tx)
                    .await
                    .map_err(internal_api_error)?
                    .rows_affected();
                    if affected > 0 {
                        resolved_members
                            .retain(|member| member.skill_resource_id != skill_id.as_uuid());
                        changed = true;
                    }
                }
            }
        }
        let pack = if changed {
            order_resolved_members(&mut tx, &mut resolved_members).await?;
            insert_pack_revision(&mut tx, &pack, &resolved_members, None).await?
        } else {
            pack
        };
        let outcome = PackMutationResponse {
            pack: pack_summary(&mut tx, &pack).await?,
            changed,
        };
        record_pack_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            pack.id,
            action,
            &outcome,
        )
        .await?;
        if changed {
            enqueue_resource_wake(&mut tx, pack.id, generation_u64(pack.generation)?).await?;
        }
        tx.commit().await.map_err(internal_api_error)?;
        if changed {
            let _ = self.drain_outbox(64).await;
        }
        Ok(outcome)
    }

    pub async fn publish_pack(
        &self,
        bearer: &str,
        request: &PackPublishRequest,
    ) -> Result<PackMutationResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let resource_id = parse_resource(&request.resource_id)?;
        let supplied_hash = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied_hash,
            pack_publish_request_hash(
                &request.operation_id,
                &request.resource_id,
                request.expected_generation,
                request.public,
            )
            .map_err(hash_error)?,
        )?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_pack_operation::<PackMutationResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            "publish",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let resource_authority =
            authorize_resource_publish(&mut tx, &authority, resource_id.as_uuid()).await?;
        let mut pack = load_owned_pack_for_update(
            &mut tx,
            resource_id.as_uuid(),
            resource_authority.namespace_id,
        )
        .await?;
        let caught_up = lock_and_catch_up_pack(&mut tx, &mut pack, &[]).await?;
        if let Some(generation) = caught_up.last().copied() {
            enqueue_resource_wake(&mut tx, pack.id, generation).await?;
            tx.commit().await.map_err(internal_api_error)?;
            let _ = self.drain_outbox(32).await;
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!(
                    "pack advanced through pending skill releases to generation {generation}; retry from current state"
                ),
            ));
        }
        ensure_generation(pack.generation, request.expected_generation)?;
        let current = load_pack_revision_members(&mut tx, pack.id, pack.current_version).await?;
        // Public packs may expose only public immutable skill releases. If a private pack
        // currently resolves a follow-latest member to a private workspace revision, public
        // publication creates one exact public-safe pack version rather than leaking that draft.
        let make_public =
            !resource_authority.is_team || pack.visibility == "public" || request.public;
        let resolved = resolve_all_members(
            &mut tx,
            authority.user_id,
            resource_authority.namespace_id,
            make_public,
            resource_authority.is_team,
            pack.id,
        )
        .await?;
        let resolution_changed = current != resolved;
        if resolution_changed {
            pack = insert_pack_revision(&mut tx, &pack, &resolved, None).await?;
        }
        let visibility_changed = make_public && pack.visibility != "public";
        if visibility_changed {
            pack.generation = next_generation(pack.generation)?;
            pack.visibility = "public".to_owned();
            sqlx::query(
                "UPDATE resources SET visibility='public',generation=$1,latest_release_version=$2 WHERE id=$3",
            )
            .bind(pack.generation)
            .bind(pack.current_version)
            .bind(pack.id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        } else if resolution_changed {
            sqlx::query("UPDATE resources SET latest_release_version=$1 WHERE id=$2")
                .bind(pack.current_version)
                .bind(pack.id)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
        }
        let changed = visibility_changed || resolution_changed;
        let outcome = PackMutationResponse {
            pack: pack_summary(&mut tx, &pack).await?,
            changed,
        };
        record_pack_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            pack.id,
            "publish",
            &outcome,
        )
        .await?;
        if changed {
            enqueue_resource_wake(&mut tx, pack.id, generation_u64(pack.generation)?).await?;
        }
        tx.commit().await.map_err(internal_api_error)?;
        if changed {
            let _ = self.drain_outbox(64).await;
        }
        Ok(outcome)
    }

    pub async fn pack_detail(
        &self,
        bearer: Option<&str>,
        locator: &str,
    ) -> Result<PackDetail, ApiError> {
        let parsed = ResourceLocator::from_str(locator)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        if parsed.kind() != ResourceKind::Pack {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "expected a pack locator",
            ));
        }
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let pack = load_pack_by_locator(&mut tx, &parsed).await?;
        let actor = if pack.visibility == "public" {
            None
        } else {
            let bearer =
                bearer.ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "pack not found"))?;
            let authority = self.user_authority(bearer, "skills:read").await?;
            let team_member =
                user_is_team_member(&self.pool, authority.user_id, pack.owner_namespace_id).await?;
            if authority.namespace_id != pack.owner_namespace_id && !team_member {
                return Err(ApiError::new(ApiErrorCode::NotFound, "pack not found"));
            }
            Some((
                authority.user_id,
                if team_member {
                    pack.owner_namespace_id
                } else {
                    authority.namespace_id
                },
            ))
        };
        let member_rows =
            load_pack_revision_members(&mut tx, pack.id, pack.current_version).await?;
        let summary = pack_summary(&mut tx, &pack).await?;
        let mut members = Vec::with_capacity(member_rows.len());
        for member in member_rows {
            members.push(
                self.pack_member_detail(&mut tx, &pack, actor, member)
                    .await?,
            );
        }
        tx.commit().await.map_err(internal_api_error)?;
        Ok(PackDetail {
            pack: PackSummary {
                degraded: members
                    .iter()
                    .any(|member| member.unavailable_reason.is_some()),
                ..summary
            },
            members,
        })
    }

    pub async fn mutate_pack_subscription(
        &self,
        bearer: &str,
        kind: PackSubscriptionMutationKind,
        request: &PackSubscriptionRequest,
    ) -> Result<PackSubscriptionResponse, ApiError> {
        let subject = self.subscription_subject(bearer).await?;
        let resource_id = parse_resource(&request.resource_id)?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied_hash = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied_hash,
            pack_subscription_request_hash(
                kind,
                &request.operation_id,
                &request.resource_id,
                request.expected_generation,
            )
            .map_err(hash_error)?,
        )?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let action = match kind {
            PackSubscriptionMutationKind::Subscribe => "subscribe",
            PackSubscriptionMutationKind::Unsubscribe => "unsubscribe",
        };
        if let Some(outcome) = replay_subscription_operation::<PackSubscriptionResponse>(
            &mut tx,
            subject,
            operation_id,
            supplied_hash,
            action,
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let row = sqlx::query(
            "SELECT r.generation,r.visibility,r.deleted_at IS NOT NULL,n.slug,r.slug,ps.current_version,r.owner_namespace_id \
             FROM resources r LEFT JOIN namespaces n ON n.id=r.owner_namespace_id \
             JOIN pack_state ps ON ps.resource_id=r.id WHERE r.id=$1 AND r.kind='pack' FOR UPDATE OF r,ps",
        )
        .bind(resource_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "pack not found"))?;
        let generation: i64 = row.get(0);
        let visibility: String = row.get(1);
        let deleted: bool = row.get(2);
        let owner: Option<String> = row.get(3);
        let name: String = row.get(4);
        let version: i64 = row.get(5);
        let owner_namespace_id: Option<Uuid> = row.get(6);
        ensure_generation(generation, request.expected_generation)?;
        if kind == PackSubscriptionMutationKind::Subscribe {
            let readable = match subject {
                SubscriptionSubject::Installation(_) => visibility == "public" && !deleted,
                SubscriptionSubject::User(user_id) => {
                    if visibility == "public" && !deleted {
                        true
                    } else if !deleted {
                        let namespace = sqlx::query_scalar::<_, Uuid>(
                            "SELECT namespace_id FROM users WHERE id=$1",
                        )
                        .bind(user_id)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(internal_api_error)?;
                        if owner_namespace_id == Some(namespace) {
                            true
                        } else if let Some(owner_namespace_id) = owner_namespace_id {
                            user_is_team_member(&self.pool, user_id, owner_namespace_id).await?
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                }
            };
            if !readable {
                return Err(ApiError::new(
                    ApiErrorCode::NotFound,
                    "pack is not subscribable",
                ));
            }
        }
        if kind == PackSubscriptionMutationKind::Unsubscribe
            && let SubscriptionSubject::User(user_id) = subject
        {
            let assigning_team = sqlx::query_scalar::<_, String>(
                "SELECT n.slug FROM team_pack_assignments a \
                 JOIN team_memberships tm ON tm.team_namespace_id=a.team_namespace_id AND tm.user_id=$1 \
                 JOIN namespaces n ON n.id=a.team_namespace_id \
                 WHERE a.pack_resource_id=$2 ORDER BY n.slug LIMIT 1",
            )
            .bind(user_id)
            .bind(resource_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if let Some(team) = assigning_team {
                return Err(ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    format!(
                        "pack is enforced by @{team}; the team owner must unassign it before members can unsubscribe"
                    ),
                ));
            }
        }
        mutate_generic_subscription_row(&mut tx, subject, resource_id.as_uuid(), kind).await?;
        let outcome = PackSubscriptionResponse {
            resource_id: resource_id.to_string(),
            locator: format!(
                "@{}/packs/{name}",
                owner.unwrap_or_else(|| "deleted".to_owned())
            ),
            subscribed: kind == PackSubscriptionMutationKind::Subscribe,
            generation: generation_u64(generation)?,
            version: generation_u64(version)?,
        };
        record_subscription_operation(
            &mut tx,
            subject,
            operation_id,
            supplied_hash,
            action,
            resource_id.as_uuid(),
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.wake_tx.send(crate::RegistryWake::ResyncAll);
        Ok(outcome)
    }

    pub async fn pack_subscription_catalog(
        &self,
        bearer: &str,
    ) -> Result<PackSubscriptionCatalog, ApiError> {
        let subject = self.subscription_subject(bearer).await?;
        let requirements = match subject {
            SubscriptionSubject::Installation(id) => sqlx::query_scalar::<_, Uuid>(
                "SELECT s.resource_id FROM installation_subscriptions s JOIN resources r ON r.id=s.resource_id \
                 WHERE s.installation_id=$1 AND r.kind='pack' AND r.visibility='public' AND r.deleted_at IS NULL ORDER BY r.id",
            )
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(internal_api_error)?
            .into_iter()
            .map(|pack_id| {
                (
                    pack_id,
                    PackRequirementSource {
                        source_id: format!("direct:{pack_id}"),
                        kind: PackRequirementKind::Direct,
                        label: "direct subscription".to_owned(),
                        team_namespace_id: None,
                    },
                )
            })
            .collect::<Vec<_>>(),
            SubscriptionSubject::User(user_id) => {
                let direct = sqlx::query_scalar::<_, Uuid>(
                    "SELECT s.resource_id FROM account_subscriptions s JOIN resources r ON r.id=s.resource_id \
                     JOIN users u ON u.id=s.user_id WHERE s.user_id=$1 AND r.kind='pack' AND r.deleted_at IS NULL \
                     AND (r.visibility='public' OR r.owner_namespace_id=u.namespace_id OR EXISTS( \
                       SELECT 1 FROM team_memberships tm WHERE tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=s.user_id \
                     )) ORDER BY r.id",
                )
                .bind(user_id)
                .fetch_all(&self.pool)
                .await
                .map_err(internal_api_error)?;
                let assigned = sqlx::query_as::<_, (Uuid, Uuid, String)>(
                    "SELECT a.pack_resource_id,a.team_namespace_id,n.slug \
                     FROM team_pack_assignments a \
                     JOIN team_memberships tm ON tm.team_namespace_id=a.team_namespace_id AND tm.user_id=$1 \
                     JOIN namespaces n ON n.id=a.team_namespace_id \
                     JOIN resources p ON p.id=a.pack_resource_id AND p.kind='pack' AND p.deleted_at IS NULL \
                     WHERE p.visibility='public' OR p.owner_namespace_id=a.team_namespace_id \
                     ORDER BY n.slug,a.team_namespace_id,a.pack_resource_id",
                )
                .bind(user_id)
                .fetch_all(&self.pool)
                .await
                .map_err(internal_api_error)?;
                let mut requirements = direct
                    .into_iter()
                    .map(|pack_id| {
                        (
                            pack_id,
                            PackRequirementSource {
                                source_id: format!("direct:{pack_id}"),
                                kind: PackRequirementKind::Direct,
                                label: "direct subscription".to_owned(),
                                team_namespace_id: None,
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                requirements.extend(assigned.into_iter().map(|(pack_id, team_id, team)| {
                    (
                        pack_id,
                        PackRequirementSource {
                            source_id: format!("team:{team_id}:pack:{pack_id}"),
                            kind: PackRequirementKind::TeamAssignment,
                            label: format!("@{team}"),
                            team_namespace_id: Some(team_id.to_string()),
                        },
                    )
                }));
                requirements
            }
        };
        let mut packs = Vec::with_capacity(requirements.len());
        for (id, source) in requirements {
            let locator = sqlx::query_as::<_, (String, String)>(
                "SELECT n.slug,r.slug FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id WHERE r.id=$1",
            )
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(internal_api_error)?;
            packs.push(PackRequirement {
                source,
                pack: self
                    .pack_detail(Some(bearer), &format!("@{}/packs/{}", locator.0, locator.1))
                    .await?,
            });
        }
        Ok(PackSubscriptionCatalog { packs })
    }
}

fn validate_unique_members(members: &[PackMemberTarget]) -> Result<(), ApiError> {
    let mut seen = BTreeSet::new();
    for member in members {
        if !seen.insert(member.resource_id.as_str()) {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "pack mutation contains the same skill more than once",
            ));
        }
    }
    Ok(())
}

async fn mutate_generic_subscription_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubscriptionSubject,
    resource_id: Uuid,
    kind: PackSubscriptionMutationKind,
) -> Result<(), ApiError> {
    match (subject, kind) {
        (SubscriptionSubject::Installation(id), PackSubscriptionMutationKind::Subscribe) => {
            sqlx::query(
                "INSERT INTO installation_subscriptions (installation_id,resource_id,pinned_release_version,retain_on_delete) \
                 VALUES ($1,$2,NULL,FALSE) ON CONFLICT(installation_id,resource_id) DO NOTHING",
            )
            .bind(id)
            .bind(resource_id)
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
        }
        (SubscriptionSubject::Installation(id), PackSubscriptionMutationKind::Unsubscribe) => {
            sqlx::query("DELETE FROM installation_subscriptions WHERE installation_id=$1 AND resource_id=$2")
                .bind(id)
                .bind(resource_id)
                .execute(&mut **tx)
                .await
                .map_err(internal_api_error)?;
        }
        (SubscriptionSubject::User(id), PackSubscriptionMutationKind::Subscribe) => {
            sqlx::query(
                "INSERT INTO account_subscriptions (user_id,resource_id,pinned_release_version,retain_on_delete) \
                 VALUES ($1,$2,NULL,FALSE) ON CONFLICT(user_id,resource_id) DO NOTHING",
            )
            .bind(id)
            .bind(resource_id)
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
        }
        (SubscriptionSubject::User(id), PackSubscriptionMutationKind::Unsubscribe) => {
            sqlx::query("DELETE FROM account_subscriptions WHERE user_id=$1 AND resource_id=$2")
                .bind(id)
                .bind(resource_id)
                .execute(&mut **tx)
                .await
                .map_err(internal_api_error)?;
        }
    }
    Ok(())
}

pub(crate) async fn replay_pack_operation<T: DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    kind: &str,
) -> Result<Option<T>, ApiError> {
    let row = sqlx::query(
        "SELECT request_hash,operation_kind,outcome_json FROM pack_operations WHERE user_id=$1 AND operation_id=$2",
    )
    .bind(user_id)
    .bind(operation_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let Some(row) = row else { return Ok(None) };
    let stored_hash: Vec<u8> = row.get(0);
    let stored_kind: String = row.get(1);
    if stored_hash.as_slice() != request_hash.as_bytes() || stored_kind != kind {
        return Err(ApiError::new(
            ApiErrorCode::OperationConflict,
            "operation_id was already used with different pack content",
        ));
    }
    serde_json::from_value(row.get(2))
        .map(Some)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}

pub(crate) async fn record_pack_operation<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    resource_id: Uuid,
    kind: &str,
    outcome: &T,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO pack_operations (user_id,operation_id,request_hash,resource_id,operation_kind,outcome_json) \
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(user_id)
    .bind(operation_id.as_uuid())
    .bind(request_hash.as_bytes().as_slice())
    .bind(resource_id)
    .bind(kind)
    .bind(serde_json::to_value(outcome).map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

async fn replay_subscription_operation<T: DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubscriptionSubject,
    operation_id: OperationId,
    request_hash: RequestHash,
    action: &str,
) -> Result<Option<T>, ApiError> {
    let row = match subject {
        SubscriptionSubject::Installation(id) => sqlx::query(
            "SELECT request_hash,action,pack_outcome_json FROM subscription_operations WHERE installation_id=$1 AND operation_id=$2",
        )
        .bind(id)
        .bind(operation_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?,
        SubscriptionSubject::User(id) => sqlx::query(
            "SELECT request_hash,action,pack_outcome_json FROM account_subscription_operations WHERE user_id=$1 AND operation_id=$2",
        )
        .bind(id)
        .bind(operation_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?,
    };
    let Some(row) = row else { return Ok(None) };
    let stored_hash: Vec<u8> = row.get(0);
    let stored_action: String = row.get(1);
    if stored_hash.as_slice() != request_hash.as_bytes() || stored_action != action {
        return Err(ApiError::new(
            ApiErrorCode::OperationConflict,
            "operation_id was already used with different subscription content",
        ));
    }
    let outcome: Option<serde_json::Value> = row.get(2);
    let outcome = outcome.ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::Internal,
            "pack subscription replay is missing its committed outcome",
        )
    })?;
    serde_json::from_value(outcome)
        .map(Some)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}

async fn record_subscription_operation<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubscriptionSubject,
    operation_id: OperationId,
    request_hash: RequestHash,
    action: &str,
    resource_id: Uuid,
    outcome: &T,
) -> Result<(), ApiError> {
    let subscribed = serde_json::to_value(outcome)
        .ok()
        .and_then(|value| value.get("subscribed").and_then(serde_json::Value::as_bool))
        .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "subscription outcome is invalid"))?;
    match subject {
        SubscriptionSubject::Installation(id) => sqlx::query(
            "INSERT INTO subscription_operations \
             (installation_id,operation_id,request_hash,action,resource_id,subscribed,pinned_release_version,retain_on_delete,pack_outcome_json) \
             VALUES ($1,$2,$3,$4,$5,$6,NULL,FALSE,$7)",
        )
        .bind(id)
        .bind(operation_id.as_uuid())
        .bind(request_hash.as_bytes().as_slice())
        .bind(action)
        .bind(resource_id)
        .bind(subscribed)
        .bind(serde_json::to_value(outcome).map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?)
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?,
        SubscriptionSubject::User(id) => sqlx::query(
            "INSERT INTO account_subscription_operations \
             (user_id,operation_id,request_hash,action,resource_id,subscribed,pinned_release_version,retain_on_delete,pack_outcome_json) \
             VALUES ($1,$2,$3,$4,$5,$6,NULL,FALSE,$7)",
        )
        .bind(id)
        .bind(operation_id.as_uuid())
        .bind(request_hash.as_bytes().as_slice())
        .bind(action)
        .bind(resource_id)
        .bind(subscribed)
        .bind(serde_json::to_value(outcome).map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?)
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?,
    };
    Ok(())
}

fn parse_operation(value: &str) -> Result<OperationId, ApiError> {
    OperationId::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
}

fn parse_resource(value: &str) -> Result<ResourceId, ApiError> {
    ResourceId::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
}

fn parse_hash(value: &str) -> Result<RequestHash, ApiError> {
    RequestHash::from_str(value).map_err(hash_error)
}

fn hash_error(error: denju_wire::RequestHashError) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string())
}

fn ensure_hash(actual: RequestHash, expected: RequestHash) -> Result<(), ApiError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::InvalidRequestHash,
            "request_hash does not match the canonical request payload",
        ))
    }
}

fn ensure_generation(stored: i64, expected: u64) -> Result<(), ApiError> {
    if generation_u64(stored)? == expected {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::GenerationConflict,
            format!("resource generation changed to {}", generation_u64(stored)?),
        ))
    }
}

fn i64_version(value: Option<u64>) -> Result<Option<i64>, ApiError> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|_| {
                ApiError::new(ApiErrorCode::InvalidRequest, "release version is too large")
            })
        })
        .transpose()
}
