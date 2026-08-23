use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
};
use denju_registry::Registry;
use denju_wire::{
    CatalogSearchQuery, CatalogSearchResponse, CatalogTopQuery, FollowMutationKind,
    FollowMutationRequest, FollowMutationResponse, ProfileUpdateRequest, ProfileUpdateResponse,
    ReportResourceRequest, ReportResourceResponse, ResourceTopicsRequest, ResourceTopicsResponse,
    StarMutationKind, StarMutationRequest, StarMutationResponse, UniversalShowResponse,
};
use serde::Deserialize;

use super::{
    ApiResponseError,
    auth::{bearer_token, optional_bearer_token},
};

pub(super) fn router() -> Router<Arc<Registry>> {
    Router::new()
        .route("/v1/search", get(search_catalog))
        .route("/v1/top", get(top_catalog))
        .route("/v1/show", get(universal_show))
        .route("/v1/profile", post(update_profile))
        .route("/v1/follows", post(follow))
        .route("/v1/follows/remove", post(unfollow))
        .route("/v1/stars", post(star))
        .route("/v1/stars/remove", post(unstar))
        .route("/v1/resources/topics", post(update_resource_topics))
        .route("/v1/reports", post(report_resource))
}

async fn search_catalog(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<CatalogSearchQuery>,
) -> Result<Json<CatalogSearchResponse>, ApiResponseError> {
    registry
        .search_catalog(optional_bearer_token(&headers), &query)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn top_catalog(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<CatalogTopQuery>,
) -> Result<Json<CatalogSearchResponse>, ApiResponseError> {
    registry
        .top_catalog(optional_bearer_token(&headers), &query)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

#[derive(Debug, Deserialize)]
struct UniversalShowQuery {
    locator: String,
    followers_cursor: Option<String>,
    following_cursor: Option<String>,
}

async fn universal_show(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<UniversalShowQuery>,
) -> Result<Json<UniversalShowResponse>, ApiResponseError> {
    registry
        .universal_show(
            optional_bearer_token(&headers),
            &query.locator,
            query.followers_cursor.as_deref(),
            query.following_cursor.as_deref(),
        )
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn update_profile(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ProfileUpdateRequest>,
) -> Result<Json<ProfileUpdateResponse>, ApiResponseError> {
    registry
        .update_profile(bearer_token(&headers)?, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn follow(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<FollowMutationRequest>,
) -> Result<Json<FollowMutationResponse>, ApiResponseError> {
    registry
        .mutate_follow(
            bearer_token(&headers)?,
            FollowMutationKind::Follow,
            &request,
        )
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn unfollow(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<FollowMutationRequest>,
) -> Result<Json<FollowMutationResponse>, ApiResponseError> {
    registry
        .mutate_follow(
            bearer_token(&headers)?,
            FollowMutationKind::Unfollow,
            &request,
        )
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn star(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<StarMutationRequest>,
) -> Result<Json<StarMutationResponse>, ApiResponseError> {
    registry
        .mutate_star(bearer_token(&headers)?, StarMutationKind::Star, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn unstar(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<StarMutationRequest>,
) -> Result<Json<StarMutationResponse>, ApiResponseError> {
    registry
        .mutate_star(bearer_token(&headers)?, StarMutationKind::Unstar, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn update_resource_topics(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ResourceTopicsRequest>,
) -> Result<Json<ResourceTopicsResponse>, ApiResponseError> {
    registry
        .update_resource_topics(bearer_token(&headers)?, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn report_resource(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ReportResourceRequest>,
) -> Result<Json<ReportResourceResponse>, ApiResponseError> {
    registry
        .report_resource(bearer_token(&headers)?, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}
