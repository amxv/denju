use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::HeaderMap,
    routing::{get, post},
};
use denju_registry::Registry;
use denju_wire::{
    ResourceTransferRequest, ResourceTransferResponse, TeamCreateRequest, TeamDeleteRequest,
    TeamDeleteResponse, TeamDetail, TeamInviteRequest, TeamInviteResponse, TeamInviteRevokeRequest,
    TeamJoinRequest, TeamLeaveRequest, TeamLeaveResponse, TeamList, TeamMemberRemoveRequest,
    TeamMemberRoleRequest, TeamMutationResponse, TeamOwnerTransferAcceptRequest,
    TeamOwnerTransferRequest, TeamOwnerTransferResponse, TeamPackAssignmentMutationKind,
    TeamPackAssignmentRequest, TeamPackAssignmentResponse, TeamSettingsRequest,
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
        .route("/v1/teams/packs/assign", post(assign_team_pack))
        .route("/v1/teams/packs/unassign", post(unassign_team_pack))
        .route("/v1/teams/leave", post(leave_team))
        .route("/v1/teams/owner-transfer", post(create_owner_transfer))
        .route(
            "/v1/teams/owner-transfer/accept",
            post(accept_owner_transfer),
        )
        .route("/v1/teams/delete", post(delete_team))
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

pub(super) async fn assign_team_pack(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamPackAssignmentRequest>,
) -> Result<Json<TeamPackAssignmentResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .mutate_team_pack_assignment(bearer, TeamPackAssignmentMutationKind::Assign, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn unassign_team_pack(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamPackAssignmentRequest>,
) -> Result<Json<TeamPackAssignmentResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .mutate_team_pack_assignment(bearer, TeamPackAssignmentMutationKind::Unassign, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn leave_team(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamLeaveRequest>,
) -> Result<Json<TeamLeaveResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .leave_team(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn create_owner_transfer(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamOwnerTransferRequest>,
) -> Result<Json<TeamOwnerTransferResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .create_team_owner_transfer(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn accept_owner_transfer(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamOwnerTransferAcceptRequest>,
) -> Result<Json<TeamOwnerTransferResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .accept_team_owner_transfer(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

pub(super) async fn delete_team(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<TeamDeleteRequest>,
) -> Result<Json<TeamDeleteResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .delete_team(bearer, &request)
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
