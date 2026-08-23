use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
};
use denju_registry::Registry;
use denju_wire::{
    AdminQuarantineMutationKind, AdminQuarantineRequest, AdminQuarantineResponse, AdminReportList,
    AdminResourceTarget,
};
use serde::Deserialize;

use super::{ApiResponseError, auth::bearer_token};

pub(super) fn router() -> Router<Arc<Registry>> {
    Router::new()
        .route("/v1/admin/reports", get(reports))
        .route("/v1/admin/resources/resolve", get(resolve_resource))
        .route("/v1/admin/quarantine", post(quarantine))
        .route("/v1/admin/unquarantine", post(unquarantine))
}

#[derive(Debug, Deserialize)]
struct ReportQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

async fn reports(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<ReportQuery>,
) -> Result<Json<AdminReportList>, ApiResponseError> {
    registry
        .admin_reports(
            bearer_token(&headers)?,
            query.limit.unwrap_or(50),
            query.cursor.as_deref(),
        )
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

#[derive(Debug, Deserialize)]
struct ResolveQuery {
    locator: String,
}

async fn resolve_resource(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<ResolveQuery>,
) -> Result<Json<AdminResourceTarget>, ApiResponseError> {
    registry
        .admin_resolve_resource(bearer_token(&headers)?, &query.locator)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn quarantine(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<AdminQuarantineRequest>,
) -> Result<Json<AdminQuarantineResponse>, ApiResponseError> {
    registry
        .mutate_quarantine(
            bearer_token(&headers)?,
            AdminQuarantineMutationKind::Quarantine,
            &request,
        )
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn unquarantine(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<AdminQuarantineRequest>,
) -> Result<Json<AdminQuarantineResponse>, ApiResponseError> {
    registry
        .mutate_quarantine(
            bearer_token(&headers)?,
            AdminQuarantineMutationKind::Unquarantine,
            &request,
        )
        .await
        .map(Json)
        .map_err(ApiResponseError)
}
