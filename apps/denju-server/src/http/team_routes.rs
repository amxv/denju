use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
};
use denju_registry::Registry;
use denju_wire::{
    ResourceTransferRequest, ResourceTransferResponse, TeamCreateRequest, TeamDetail,
    TeamInviteRequest, TeamInviteResponse, TeamInviteRevokeRequest, TeamJoinRequest, TeamList,
    TeamMemberRemoveRequest, TeamMemberRoleRequest, TeamMutationResponse, TeamSettingsRequest,
};
use serde::Deserialize;

use super::{ApiResponseError, auth::bearer_token};

pub(super) fn router() -> Router<Arc<Registry>> {
    Router::new()
        .route("/v1/teams", get(teams).post(create_team))
        .route("/v1/teams/show", get(show_team))
        .route("/v1/teams/invites", post(create_team_invite))
        .route("/v1/teams/invites/revoke", post(revoke_team_invite))
        .route("/v1/teams/join", post(join_team))
        .route("/v1/teams/members/role", post(change_team_member_role))
        .route("/v1/teams/members/remove", post(remove_team_member))
        .route("/v1/teams/settings", post(update_team_settings))
        .route("/v1/resources/transfer", post(transfer_resource))
}

pub(super) async fn create_team(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamCreateRequest>,
) -> Result<Json<TeamMutationResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .create_team(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn teams(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
) -> Result<Json<TeamList>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .teams(bearer)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

#[derive(Debug, Deserialize)]
pub(super) struct TeamQuery {
    team: String,
}

pub(super) async fn show_team(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<TeamQuery>,
) -> Result<Json<TeamDetail>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .team_detail(bearer, &query.team)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn create_team_invite(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamInviteRequest>,
) -> Result<Json<TeamInviteResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .create_team_invite(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn revoke_team_invite(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamInviteRevokeRequest>,
) -> Result<Json<TeamMutationResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .revoke_team_invite(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn join_team(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamJoinRequest>,
) -> Result<Json<TeamMutationResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .join_team(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn change_team_member_role(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamMemberRoleRequest>,
) -> Result<Json<TeamMutationResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .change_team_member_role(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn remove_team_member(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamMemberRemoveRequest>,
) -> Result<Json<TeamMutationResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .remove_team_member(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn update_team_settings(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamSettingsRequest>,
) -> Result<Json<TeamMutationResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .update_team_settings(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn transfer_resource(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ResourceTransferRequest>,
) -> Result<Json<ResourceTransferResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .transfer_resource(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}
