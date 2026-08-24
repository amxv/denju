use std::sync::Arc;

use axum::{Json, extract::State, http::HeaderMap};
use denju_registry::Registry;
use denju_wire::{PackDrainRequest, PackDrainResponse};
use serde::{Deserialize, Serialize};

use super::{ApiResponseError, auth::recovery_bearer_token};

#[derive(Debug, Deserialize)]
pub(super) struct OutboxDrainRequest {
    limit: u32,
}

#[derive(Debug, Serialize)]
pub(super) struct OutboxDrainResponse {
    dispatched: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct RecoveryDrainResponse {
    outbox_dispatched: usize,
    pack_revisions_processed: u64,
    pack_release_events_completed: u64,
    pack_release_event_pending: bool,
}

pub(super) async fn recover(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
) -> Result<Json<RecoveryDrainResponse>, ApiResponseError> {
    recovery_bearer_token(&headers)?;
    let outbox_dispatched = registry.drain_outbox(256).await.map_err(ApiResponseError)?;
    let packs = registry
        .drain_pack_release_events(256)
        .await
        .map_err(ApiResponseError)?;
    Ok(Json(RecoveryDrainResponse {
        outbox_dispatched,
        pack_revisions_processed: packs.processed_pack_revisions,
        pack_release_events_completed: packs.completed_release_events,
        pack_release_event_pending: packs.pending_release_event_id.is_some(),
    }))
}

pub(super) async fn drain_packs(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PackDrainRequest>,
) -> Result<Json<PackDrainResponse>, ApiResponseError> {
    recovery_bearer_token(&headers)?;
    registry
        .drain_pack_release_events(request.limit)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn drain_outbox(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<OutboxDrainRequest>,
) -> Result<Json<OutboxDrainResponse>, ApiResponseError> {
    recovery_bearer_token(&headers)?;
    registry
        .drain_outbox(request.limit)
        .await
        .map(|dispatched| Json(OutboxDrainResponse { dispatched }))
        .map_err(ApiResponseError)
}
