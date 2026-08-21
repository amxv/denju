use denju_wire::{ApiError, ApiErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::{Registry, RegistryWake, internal_api_error, realtime::wake_as_sync_hint};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResourceWakePayload {
    resource_id: String,
    generation: u64,
}

impl Registry {
    pub async fn drain_outbox(&self, limit: u32) -> Result<usize, ApiError> {
        let limit = i64::from(limit.clamp(1, 256));
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let rows = sqlx::query(
            "SELECT event_id,event_kind,payload_json FROM outbox_events WHERE dispatched_at IS NULL \
             ORDER BY event_id FOR UPDATE SKIP LOCKED LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let count = rows.len();
        for row in rows {
            let id: i64 = row.get(0);
            let kind: String = row.get(1);
            let payload: Value = row.get(2);
            let wake = match kind.as_str() {
                "resource_dirty" => {
                    let payload: ResourceWakePayload =
                        serde_json::from_value(payload).map_err(|error| {
                            ApiError::new(ApiErrorCode::Internal, error.to_string())
                        })?;
                    RegistryWake::Resource {
                        resource_id: Uuid::parse_str(&payload.resource_id).map_err(|error| {
                            ApiError::new(ApiErrorCode::Internal, error.to_string())
                        })?,
                        generation: payload.generation,
                    }
                }
                _ => RegistryWake::ResyncAll,
            };
            let notification = serde_json::to_string(&wake_as_sync_hint(&wake))
                .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
            // NOTIFY is transactional here: other instances observe it only if the outbox
            // dispatch commit succeeds. The LISTEN side uses a direct session connection.
            sqlx::query("SELECT pg_notify('denju_wake',$1)")
                .bind(notification)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            let _ = self.wake_tx.send(wake);
            sqlx::query("UPDATE outbox_events SET dispatched_at=now() WHERE event_id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
        }
        tx.commit().await.map_err(internal_api_error)?;
        Ok(count)
    }
}

pub(crate) async fn enqueue_resource_wake(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    generation: u64,
) -> Result<(), ApiError> {
    enqueue_resource_wake_with_event(tx, resource_id, generation, "resource_changed").await
}

pub(crate) async fn enqueue_resource_wake_with_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    generation: u64,
    authority_event_kind: &str,
) -> Result<(), ApiError> {
    let payload = serde_json::to_value(ResourceWakePayload {
        resource_id: resource_id.to_string(),
        generation,
    })
    .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
    let event_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO authority_events (event_kind,resource_id,resource_generation,payload_json) \
         VALUES ($1,$2,$3,$4) RETURNING id",
    )
    .bind(authority_event_kind)
    .bind(resource_id)
    .bind(
        i64::try_from(generation).map_err(|_| {
            ApiError::new(ApiErrorCode::Internal, "generation exceeds database range")
        })?,
    )
    .bind(payload.clone())
    .fetch_one(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    sqlx::query(
        "INSERT INTO outbox_events (event_id,event_kind,payload_json) VALUES ($1,'resource_dirty',$2)",
    )
    .bind(event_id).bind(payload).execute(&mut **tx).await.map_err(internal_api_error)?;
    Ok(())
}
