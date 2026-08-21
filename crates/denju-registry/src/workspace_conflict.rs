use std::collections::BTreeMap;

use denju_core::RevisionId;
use denju_wire::{ApiError, ApiErrorCode, PrivateWorkspaceConflict};
use sqlx::Row;
use uuid::Uuid;

use crate::internal_api_error;

pub(crate) async fn validate_merge_conflict(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    conflict_id: Uuid,
    resource_id: Uuid,
    parents: &[RevisionId],
) -> Result<(), ApiError> {
    if parents.len() != 2 {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "merge revisions require exactly two parents",
        ));
    }
    let row = sqlx::query(
        "SELECT head_a_revision_id,head_b_revision_id FROM skill_workspace_conflicts \
         WHERE conflict_id=$1 AND resource_id=$2 AND resolved_at IS NULL FOR UPDATE",
    )
    .bind(conflict_id)
    .bind(resource_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::GenerationConflict,
            "workspace conflict is no longer active; reconcile before merging",
        )
    })?;
    let expected = vec![
        decode_revision(row.get(0), "stored conflict head")?,
        decode_revision(row.get(1), "stored conflict head")?,
    ];
    if expected != parents {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "merge parents do not match the active workspace conflict",
        ));
    }
    Ok(())
}

pub(crate) async fn record_workspace_conflict(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    base_revision_id: &[u8; 32],
    detached_revision_id: &[u8; 32],
    active_revision_id: &[u8; 32],
    generation: i64,
) -> Result<PrivateWorkspaceConflict, ApiError> {
    let mut heads = [*detached_revision_id, *active_revision_id];
    heads.sort();
    let new_conflict_id = Uuid::now_v7();
    let row = sqlx::query(
        "INSERT INTO skill_workspace_conflicts \
         (conflict_id,resource_id,base_revision_id,head_a_revision_id,head_b_revision_id,active_revision_id,detected_generation) \
         VALUES ($1,$2,$3,$4,$5,$6,$7) \
         ON CONFLICT (resource_id,head_a_revision_id,head_b_revision_id) WHERE resolved_at IS NULL \
         DO UPDATE SET active_revision_id=excluded.active_revision_id,detected_generation=excluded.detected_generation \
         RETURNING conflict_id,base_revision_id,head_a_revision_id,head_b_revision_id,active_revision_id",
    )
    .bind(new_conflict_id)
    .bind(resource_id)
    .bind(base_revision_id.as_slice())
    .bind(heads[0].as_slice())
    .bind(heads[1].as_slice())
    .bind(active_revision_id.as_slice())
    .bind(generation)
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    conflict_from_row(
        row,
        resource_id,
        u64::try_from(generation)
            .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored generation is invalid"))?,
    )
}

pub(crate) async fn unresolved_workspace_conflicts_for_resources(
    pool: &sqlx::PgPool,
    generations: &BTreeMap<Uuid, u64>,
) -> Result<BTreeMap<Uuid, Vec<PrivateWorkspaceConflict>>, ApiError> {
    if generations.is_empty() {
        return Ok(BTreeMap::new());
    }
    let resource_ids = generations.keys().copied().collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT resource_id,conflict_id,base_revision_id,head_a_revision_id,head_b_revision_id,active_revision_id \
         FROM skill_workspace_conflicts WHERE resource_id = ANY($1) AND resolved_at IS NULL \
         ORDER BY resource_id,created_at,conflict_id",
    )
    .bind(&resource_ids)
    .fetch_all(pool)
    .await
    .map_err(internal_api_error)?;
    let mut conflicts = BTreeMap::<Uuid, Vec<PrivateWorkspaceConflict>>::new();
    for row in rows {
        let resource_id: Uuid = row.get(0);
        let generation = generations.get(&resource_id).copied().ok_or_else(|| {
            ApiError::new(
                ApiErrorCode::Internal,
                "workspace conflict references an unexpected resource",
            )
        })?;
        conflicts
            .entry(resource_id)
            .or_default()
            .push(conflict_from_values(
                row.get(1),
                resource_id,
                generation,
                row.get(2),
                row.get(3),
                row.get(4),
                row.get(5),
            )?);
    }
    Ok(conflicts)
}

pub(crate) async fn resolve_workspace_conflict(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    conflict_id: Uuid,
    resource_id: Uuid,
    resolution_revision_id: &[u8; 32],
) -> Result<(), ApiError> {
    let changed = sqlx::query(
        "UPDATE skill_workspace_conflicts SET resolution_revision_id=$1,resolved_at=now() \
         WHERE conflict_id=$2 AND resource_id=$3 AND resolved_at IS NULL",
    )
    .bind(resolution_revision_id.as_slice())
    .bind(conflict_id)
    .bind(resource_id)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .rows_affected();
    if changed == 1 {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::GenerationConflict,
            "workspace conflict was resolved concurrently",
        ))
    }
}

fn conflict_from_row(
    row: sqlx::postgres::PgRow,
    resource_id: Uuid,
    generation: u64,
) -> Result<PrivateWorkspaceConflict, ApiError> {
    conflict_from_values(
        row.get(0),
        resource_id,
        generation,
        row.get(1),
        row.get(2),
        row.get(3),
        row.get(4),
    )
}

fn conflict_from_values(
    conflict_id: Uuid,
    resource_id: Uuid,
    generation: u64,
    base_revision_id: Vec<u8>,
    head_a_revision_id: Vec<u8>,
    head_b_revision_id: Vec<u8>,
    active_revision_id: Vec<u8>,
) -> Result<PrivateWorkspaceConflict, ApiError> {
    let base = decode_revision(base_revision_id, "stored conflict base")?;
    let head_a = decode_revision(head_a_revision_id, "stored conflict head")?;
    let head_b = decode_revision(head_b_revision_id, "stored conflict head")?;
    let active = decode_revision(active_revision_id, "stored active conflict head")?;
    Ok(PrivateWorkspaceConflict {
        conflict_id: conflict_id.to_string(),
        resource_id: resource_id.to_string(),
        base_revision_id: base.to_string(),
        head_revision_ids: vec![head_a.to_string(), head_b.to_string()],
        active_revision_id: active.to_string(),
        generation,
        resolution_revision_id: None,
    })
}

fn decode_revision(bytes: Vec<u8>, field: &str) -> Result<RevisionId, ApiError> {
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| {
        ApiError::new(
            ApiErrorCode::Internal,
            format!("{field} has invalid length"),
        )
    })?;
    Ok(RevisionId::from_bytes(bytes))
}
