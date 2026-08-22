use std::str::FromStr;

use denju_core::RevisionId;
use denju_wire::{ApiError, ApiErrorCode, PackDrainResponse};
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    Registry, internal_api_error,
    lifecycle::{generation_u64, next_generation},
    outbox::enqueue_resource_wake,
    pack_storage::{PackRow, load_pack_revision_members},
};

const PACK_MUTATION_CATCH_UP_LIMIT: usize = 1024;

#[derive(Debug, Deserialize)]
struct ReleaseEventPayload {
    resource_id: String,
    release_version: u64,
    revision_id: String,
}

impl Registry {
    /// Bounded, deployment-neutral drain for durable skill-release -> pack advancement.
    /// It processes authority events strictly in event order and never coalesces two releases.
    pub async fn drain_pack_release_events(
        &self,
        max_pack_revisions: u32,
    ) -> Result<PackDrainResponse, ApiError> {
        let budget = usize::try_from(max_pack_revisions.clamp(1, 256))
            .map_err(|_| ApiError::new(ApiErrorCode::Internal, "invalid pack drain limit"))?;
        let mut processed_pack_revisions = 0usize;
        let mut completed_release_events = 0usize;
        loop {
            let event = load_earliest_pending_release_event(&self.pool).await?;
            let Some(event) = event else {
                return Ok(PackDrainResponse {
                    processed_pack_revisions: processed_pack_revisions as u64,
                    completed_release_events: completed_release_events as u64,
                    pending_release_event_id: None,
                });
            };
            if processed_pack_revisions >= budget {
                return Ok(PackDrainResponse {
                    processed_pack_revisions: processed_pack_revisions as u64,
                    completed_release_events: completed_release_events as u64,
                    pending_release_event_id: Some(event.id as u64),
                });
            }
            let pack_id = next_pack_for_release_event(&self.pool, &event).await?;
            if let Some(pack_id) = pack_id {
                if self.advance_pack_for_release_event(pack_id, &event).await? {
                    processed_pack_revisions += 1;
                }
                continue;
            }
            sqlx::query(
                "INSERT INTO pack_release_event_completions (event_id) VALUES ($1) ON CONFLICT DO NOTHING",
            )
            .bind(event.id)
            .execute(&self.pool)
            .await
            .map_err(internal_api_error)?;
            completed_release_events += 1;
        }
    }

    async fn advance_pack_for_release_event(
        &self,
        pack_id: Uuid,
        event: &ReleaseEvent,
    ) -> Result<bool, ApiError> {
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        lock_pack_resource(&mut tx, pack_id).await?;
        lock_skill_resource(&mut tx, event.skill_resource_id).await?;
        let advanced = advance_pack_event_in_tx(&mut tx, pack_id, event).await?;
        if let Some(generation) = advanced {
            enqueue_resource_wake(&mut tx, pack_id, generation).await?;
        }
        tx.commit().await.map_err(internal_api_error)?;
        if advanced.is_some() {
            let _ = self.drain_outbox(32).await;
        }
        Ok(advanced.is_some())
    }
}

#[derive(Debug, Clone)]
struct ReleaseEvent {
    id: i64,
    skill_resource_id: Uuid,
    release_version: i64,
    revision_id: Vec<u8>,
}

async fn load_earliest_pending_release_event(
    pool: &sqlx::PgPool,
) -> Result<Option<ReleaseEvent>, ApiError> {
    let row = sqlx::query(
        "SELECT ae.id,ae.resource_id,ae.payload_json FROM authority_events ae \
         LEFT JOIN pack_release_event_completions done ON done.event_id=ae.id \
         WHERE ae.event_kind='skill_release_published' AND done.event_id IS NULL \
         ORDER BY ae.id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(internal_api_error)?;
    row.map(parse_release_event).transpose()
}

async fn next_pack_for_release_event(
    pool: &sqlx::PgPool,
    event: &ReleaseEvent,
) -> Result<Option<Uuid>, ApiError> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT pm.pack_resource_id FROM pack_members pm JOIN resources p ON p.id=pm.pack_resource_id \
         WHERE pm.skill_resource_id=$1 AND pm.pinned_release_version IS NULL \
         AND pm.follow_after_event_id < $2 AND p.deleted_at IS NULL \
         AND NOT EXISTS(SELECT 1 FROM pack_revisions pr WHERE pr.pack_resource_id=pm.pack_resource_id AND pr.source_release_event_id=$2) \
         ORDER BY pm.pack_resource_id LIMIT 1",
    )
    .bind(event.skill_resource_id)
    .bind(event.id)
    .fetch_optional(pool)
    .await
    .map_err(internal_api_error)
}

fn parse_release_event(row: sqlx::postgres::PgRow) -> Result<ReleaseEvent, ApiError> {
    let id: i64 = row.get(0);
    let resource_id: Uuid = row.get(1);
    let payload: ReleaseEventPayload = serde_json::from_value(row.get(2)).map_err(|error| {
        ApiError::new(
            ApiErrorCode::Internal,
            format!("invalid release event {id}: {error}"),
        )
    })?;
    if payload.resource_id != resource_id.to_string() {
        return Err(ApiError::new(
            ApiErrorCode::Internal,
            format!("release event {id} resource mismatch"),
        ));
    }
    let release_version = i64::try_from(payload.release_version)
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "release event version is invalid"))?;
    let revision = RevisionId::from_str(&payload.revision_id)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    Ok(ReleaseEvent {
        id,
        skill_resource_id: resource_id,
        release_version,
        revision_id: revision.as_bytes().to_vec(),
    })
}

pub(crate) async fn lock_skill_resource(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("denju-skill:{resource_id}"))
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    Ok(())
}

async fn lock_pack_resource(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("denju-pack:{resource_id}"))
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    Ok(())
}

pub(crate) async fn lock_and_catch_up_pack(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pack: &mut PackRow,
    extra_skill_ids: &[Uuid],
) -> Result<Vec<u64>, ApiError> {
    lock_pack_resource(tx, pack.id).await?;
    let mut skill_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT skill_resource_id FROM pack_members WHERE pack_resource_id=$1",
    )
    .bind(pack.id)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    skill_ids.extend_from_slice(extra_skill_ids);
    skill_ids.sort_unstable();
    skill_ids.dedup();
    for skill_id in skill_ids {
        lock_skill_resource(tx, skill_id).await?;
    }
    let events = sqlx::query(
        "SELECT ae.id,ae.resource_id,ae.payload_json FROM authority_events ae \
         JOIN pack_members pm ON pm.skill_resource_id=ae.resource_id \
         WHERE pm.pack_resource_id=$1 AND pm.pinned_release_version IS NULL \
         AND ae.event_kind='skill_release_published' AND pm.follow_after_event_id < ae.id \
         AND NOT EXISTS(SELECT 1 FROM pack_revisions pr WHERE pr.pack_resource_id=$1 AND pr.source_release_event_id=ae.id) \
         ORDER BY ae.id LIMIT $2",
    )
    .bind(pack.id)
    .bind(i64::try_from(PACK_MUTATION_CATCH_UP_LIMIT + 1).unwrap_or(i64::MAX))
    .fetch_all(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    if events.len() > PACK_MUTATION_CATCH_UP_LIMIT {
        return Err(ApiError::new(
            ApiErrorCode::Unavailable,
            "pack has too many pending release events; drain pack updates and retry",
        ));
    }
    let mut generations = Vec::with_capacity(events.len());
    for row in events {
        let event = parse_release_event(row)?;
        if let Some(generation) = advance_pack_event_in_tx(tx, pack.id, &event).await? {
            pack.generation = i64::try_from(generation)
                .map_err(|_| ApiError::new(ApiErrorCode::Internal, "pack generation is invalid"))?;
            pack.current_version = next_generation(pack.current_version)?;
            generations.push(generation);
        }
    }
    Ok(generations)
}

async fn advance_pack_event_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pack_id: Uuid,
    event: &ReleaseEvent,
) -> Result<Option<u64>, ApiError> {
    let member = sqlx::query(
        "SELECT pinned_release_version,follow_after_event_id FROM pack_members WHERE pack_resource_id=$1 AND skill_resource_id=$2",
    )
    .bind(pack_id)
    .bind(event.skill_resource_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let Some(member) = member else {
        return Ok(None);
    };
    let pinned: Option<i64> = member.get(0);
    let follow_after: i64 = member.get(1);
    if pinned.is_some() || follow_after >= event.id {
        return Ok(None);
    }
    let already = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM pack_revisions WHERE pack_resource_id=$1 AND source_release_event_id=$2)",
    )
    .bind(pack_id)
    .bind(event.id)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    if already {
        return Ok(None);
    }
    let stored_revision = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT revision_id FROM skill_releases WHERE resource_id=$1 AND version=$2",
    )
    .bind(event.skill_resource_id)
    .bind(event.release_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::Internal,
            "release event points to a missing release",
        )
    })?;
    if stored_revision != event.revision_id {
        return Err(ApiError::new(
            ApiErrorCode::Internal,
            "release event revision does not match immutable release authority",
        ));
    }
    let row = sqlx::query(
        "SELECT r.owner_namespace_id,n.slug,r.slug,r.generation,r.visibility,ps.current_version \
         FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id JOIN pack_state ps ON ps.resource_id=r.id \
         WHERE r.id=$1 AND r.kind='pack' AND r.deleted_at IS NULL FOR UPDATE OF r,ps",
    )
    .bind(pack_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let Some(row) = row else { return Ok(None) };
    let pack = PackRow {
        id: pack_id,
        owner_namespace_id: row.get(0),
        owner: row.get(1),
        name: row.get(2),
        generation: row.get(3),
        visibility: row.get(4),
        current_version: row.get(5),
    };
    let mut members = load_pack_revision_members(tx, pack.id, pack.current_version).await?;
    let Some(target) = members
        .iter_mut()
        .find(|member| member.skill_resource_id == event.skill_resource_id)
    else {
        return Err(ApiError::new(
            ApiErrorCode::Internal,
            "current pack intent and revision membership diverged",
        ));
    };
    target.resolved_release_version = Some(event.release_version);
    target.resolved_revision_id = event.revision_id.clone();
    let version = next_generation(pack.current_version)?;
    let generation = next_generation(pack.generation)?;
    sqlx::query(
        "INSERT INTO pack_revisions (pack_resource_id,version,source_release_event_id) VALUES ($1,$2,$3)",
    )
    .bind(pack.id)
    .bind(version)
    .bind(event.id)
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
    Ok(Some(generation_u64(generation)?))
}
