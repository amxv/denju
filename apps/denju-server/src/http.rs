use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use denju_registry::Registry;
use denju_wire::{
    AccountDeleteRequest, AccountDeleteResponse, ApiError, ApiErrorCode,
    AutomationTokenCreateRequest, AutomationTokenCreateResponse, AutomationTokenList,
    AutomationTokenRevokeRequest, AutomationTokenRevokeResponse, ClaimIdentityRequest,
    CreateInstallationRequest, CreateInstallationResponse, DeleteSkillResponse,
    DeprecateSkillRequest, DeprecateSkillResponse, DeviceList, DeviceRevokeRequest,
    DeviceRevokeResponse, HistoryPruneResponse, IdentityBackupRequest, IdentityInfo,
    IdentitySessionResponse, LoginRequest, PackCreateRequest, PackCreateResponse, PackDetail,
    PackDrainRequest, PackDrainResponse, PackLifecycleRequest, PackLifecycleResponse,
    PackMutationKind, PackMutationRequest, PackMutationResponse, PackPublishRequest,
    PackRenameRequest, PackSubscriptionCatalog, PackSubscriptionMutationKind,
    PackSubscriptionRequest, PackSubscriptionResponse, PrivateRevisionCommitRequest,
    PrivateRevisionCommitResponse, PrivateRevisionPrepareResponse, PrivateRevisionRequest,
    PrivateSkillCatalog, PrivateSkillImportCommitRequest, PrivateSkillImportPrepareResponse,
    PrivateSkillImportRequest, PrivateSkillImportResponse, ProposalAcceptRequest,
    ProposalCloseRequest, ProposalCreateRequest, PublicSkillDetail, PublishSkillRequest,
    PublishSkillResponse, RecoveryResetRequest, RegistryCapabilities, RenameSkillRequest,
    RenameSkillResponse, ResourceLifecycleRequest, RestoreSkillRequest, RestoreSkillResponse,
    ShareMutationKind, ShareSkillRequest, ShareSkillResponse, SkillHistoryResponse, SkillProposal,
    SkillProposalDetail, SkillProposalList, SkillRevisionDetail, SubscriptionCatalog,
    SubscriptionMutationKind, SubscriptionMutationRequest, SubscriptionMutationResponse,
    SubscriptionTarget, SyncReconcileRequest, SyncReconcileResponse, UnpublishSkillResponse,
    UsageResponse,
};
use serde::{Deserialize, Serialize};

mod admin_routes;
mod auth;
mod discovery_routes;
pub(crate) mod realtime_routes;
mod team_routes;

use crate::observability::HttpMetrics;
use auth::{bearer_token, optional_bearer_token, recovery_bearer_token};

pub(super) fn router(registry: Arc<Registry>) -> Router {
    let metrics = Arc::new(HttpMetrics::new());
    Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/health/metrics", get(crate::observability::health_metrics))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/installations", post(create_installation))
        .route("/v1/identity/claim", post(claim_identity))
        .route("/v1/identity/login", post(login))
        .route("/v1/identity/recover", post(recovery_reset))
        .route("/v1/identity/backup", post(identity_backup))
        .route("/v1/identity", get(identity_info))
        .route("/v1/devices", get(devices))
        .route("/v1/devices/revoke", post(revoke_device))
        .route(
            "/v1/tokens",
            get(automation_tokens).post(create_automation_token),
        )
        .route("/v1/tokens/revoke", post(revoke_automation_token))
        .route("/v1/account/delete", post(delete_account))
        .merge(admin_routes::router())
        .merge(discovery_routes::router())
        .merge(team_routes::router())
        .route("/v1/skills/show", get(show_public_skill))
        .route("/v1/skills/publish", post(publish_skill))
        .route("/v1/skills/history", get(skill_history))
        .route("/v1/skills/revision", get(skill_revision))
        .route("/v1/skills/restore", post(restore_skill))
        .route("/v1/skills/rename", post(rename_skill))
        .route("/v1/skills/unpublish", post(unpublish_skill))
        .route("/v1/skills/delete", post(delete_skill))
        .route("/v1/skills/deprecate", post(deprecate_skill))
        .route("/v1/skills/history/prune", post(prune_skill_history))
        .route("/v1/usage", get(usage))
        .route("/v1/private-skills", get(private_skills))
        .route(
            "/v1/private-skills/imports/prepare",
            post(prepare_private_skill_import),
        )
        .route(
            "/v1/private-skills/imports/commit",
            post(commit_private_skill_import),
        )
        .route(
            "/v1/private-skills/revisions/prepare",
            post(prepare_private_revision),
        )
        .route(
            "/v1/private-skills/revisions/commit",
            post(commit_private_revision),
        )
        .route(
            "/v1/subscriptions",
            get(subscription_catalog).post(subscribe),
        )
        .route("/v1/subscriptions/resolve", get(subscription_target))
        .route("/v1/subscriptions/remove", post(unsubscribe))
        .route("/v1/shares", post(share_skill))
        .route("/v1/shares/remove", post(unshare_skill))
        .route("/v1/proposals", get(proposals).post(create_proposal))
        .route("/v1/proposals/show", get(show_proposal))
        .route("/v1/proposals/accept", post(accept_proposal))
        .route("/v1/proposals/reject", post(reject_proposal))
        .route("/v1/proposals/withdraw", post(withdraw_proposal))
        .route("/v1/packs", get(show_pack).post(create_pack))
        .route("/v1/packs/add", post(add_pack_members))
        .route("/v1/packs/remove", post(remove_pack_members))
        .route("/v1/packs/publish", post(publish_pack))
        .route("/v1/packs/rename", post(rename_pack))
        .route("/v1/packs/unpublish", post(unpublish_pack))
        .route("/v1/packs/delete", post(delete_pack))
        .route(
            "/v1/pack-subscriptions",
            get(pack_subscriptions).post(subscribe_pack),
        )
        .route("/v1/pack-subscriptions/remove", post(unsubscribe_pack))
        .route("/v1/internal/outbox/drain", post(drain_outbox))
        .route("/v1/internal/packs/drain", post(drain_packs))
        .route("/v1/sync/reconcile", post(sync_reconcile))
        .route("/v1/events", get(realtime_routes::events))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(
            metrics.clone(),
            crate::observability::observe_request,
        ))
        .layer(Extension(metrics))
        .with_state(registry)
}

async fn claim_identity(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ClaimIdentityRequest>,
) -> Result<Json<IdentitySessionResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .claim_identity(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn login(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Json<IdentitySessionResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .login(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn recovery_reset(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<RecoveryResetRequest>,
) -> Result<Json<IdentitySessionResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .recovery_reset(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn identity_backup(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<IdentityBackupRequest>,
) -> Result<StatusCode, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .identity_backup(bearer, &request)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(ApiResponseError)
}

async fn identity_info(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
) -> Result<Json<IdentityInfo>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .whoami(bearer)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn devices(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
) -> Result<Json<DeviceList>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .devices(bearer)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn revoke_device(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<DeviceRevokeRequest>,
) -> Result<Json<DeviceRevokeResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .revoke_device(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn create_automation_token(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<AutomationTokenCreateRequest>,
) -> Result<Json<AutomationTokenCreateResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .create_automation_token(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn automation_tokens(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
) -> Result<Json<AutomationTokenList>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .automation_tokens(bearer)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn revoke_automation_token(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<AutomationTokenRevokeRequest>,
) -> Result<Json<AutomationTokenRevokeResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .revoke_automation_token(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn delete_account(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<AccountDeleteRequest>,
) -> Result<Json<AccountDeleteResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .delete_account(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

#[derive(Debug, Deserialize)]
struct ShowQuery {
    locator: String,
}

async fn show_public_skill(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<ShowQuery>,
) -> Result<Json<PublicSkillDetail>, ApiResponseError> {
    registry
        .show_public_skill(optional_bearer_token(&headers), &query.locator)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn publish_skill(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PublishSkillRequest>,
) -> Result<Json<PublishSkillResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .publish_skill(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn skill_history(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<ShowQuery>,
) -> Result<Json<SkillHistoryResponse>, ApiResponseError> {
    registry
        .skill_history(optional_bearer_token(&headers), &query.locator)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

#[derive(Debug, Deserialize)]
struct RevisionQuery {
    locator: String,
    revision: String,
}

async fn skill_revision(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<RevisionQuery>,
) -> Result<Json<SkillRevisionDetail>, ApiResponseError> {
    registry
        .skill_revision_detail(
            optional_bearer_token(&headers),
            &query.locator,
            &query.revision,
        )
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn restore_skill(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<RestoreSkillRequest>,
) -> Result<Json<RestoreSkillResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .restore_skill(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn rename_skill(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<RenameSkillRequest>,
) -> Result<Json<RenameSkillResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .rename_skill(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn unpublish_skill(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ResourceLifecycleRequest>,
) -> Result<Json<UnpublishSkillResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .unpublish_skill(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn delete_skill(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ResourceLifecycleRequest>,
) -> Result<Json<DeleteSkillResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .delete_skill(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn deprecate_skill(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<DeprecateSkillRequest>,
) -> Result<Json<DeprecateSkillResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .deprecate_skill(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn prune_skill_history(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ResourceLifecycleRequest>,
) -> Result<Json<HistoryPruneResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .prune_skill_history(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn usage(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
) -> Result<Json<UsageResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .usage(bearer)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn private_skills(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
) -> Result<Json<PrivateSkillCatalog>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .private_skill_catalog(bearer)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn prepare_private_skill_import(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PrivateSkillImportRequest>,
) -> Result<Json<PrivateSkillImportPrepareResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .prepare_private_skill_import(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn commit_private_skill_import(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PrivateSkillImportCommitRequest>,
) -> Result<Json<PrivateSkillImportResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .commit_private_skill_import(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn prepare_private_revision(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PrivateRevisionRequest>,
) -> Result<Json<PrivateRevisionPrepareResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .prepare_private_revision(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn commit_private_revision(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PrivateRevisionCommitRequest>,
) -> Result<Json<PrivateRevisionCommitResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .commit_private_revision(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn subscribe(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<SubscriptionMutationRequest>,
) -> Result<Json<SubscriptionMutationResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .mutate_subscription(bearer, SubscriptionMutationKind::Subscribe, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn unsubscribe(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<SubscriptionMutationRequest>,
) -> Result<Json<SubscriptionMutationResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .mutate_subscription(bearer, SubscriptionMutationKind::Unsubscribe, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn subscription_catalog(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
) -> Result<Json<SubscriptionCatalog>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .subscription_catalog(bearer)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn subscription_target(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<ShowQuery>,
) -> Result<Json<SubscriptionTarget>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .subscription_target(bearer, &query.locator)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn share_skill(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ShareSkillRequest>,
) -> Result<Json<ShareSkillResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .mutate_private_share(bearer, ShareMutationKind::Share, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn unshare_skill(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ShareSkillRequest>,
) -> Result<Json<ShareSkillResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .mutate_private_share(bearer, ShareMutationKind::Unshare, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

#[derive(Debug, Deserialize)]
struct ProposalQuery {
    id: String,
}

async fn proposals(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
) -> Result<Json<SkillProposalList>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .proposals(bearer)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn show_proposal(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<ProposalQuery>,
) -> Result<Json<SkillProposalDetail>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .proposal_detail(bearer, &query.id)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn create_proposal(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ProposalCreateRequest>,
) -> Result<Json<SkillProposal>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .create_proposal(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn accept_proposal(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ProposalAcceptRequest>,
) -> Result<Json<SkillProposal>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .accept_proposal(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn reject_proposal(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ProposalCloseRequest>,
) -> Result<Json<SkillProposal>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .reject_proposal(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn withdraw_proposal(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<ProposalCloseRequest>,
) -> Result<Json<SkillProposal>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .withdraw_proposal(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn create_pack(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PackCreateRequest>,
) -> Result<Json<PackCreateResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .create_pack(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn show_pack(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Query(query): Query<ShowQuery>,
) -> Result<Json<PackDetail>, ApiResponseError> {
    registry
        .pack_detail(optional_bearer_token(&headers), &query.locator)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn add_pack_members(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PackMutationRequest>,
) -> Result<Json<PackMutationResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .mutate_pack(bearer, PackMutationKind::Add, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn remove_pack_members(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PackMutationRequest>,
) -> Result<Json<PackMutationResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .mutate_pack(bearer, PackMutationKind::Remove, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn publish_pack(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PackPublishRequest>,
) -> Result<Json<PackMutationResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .publish_pack(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn rename_pack(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PackRenameRequest>,
) -> Result<Json<PackLifecycleResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .rename_pack(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn unpublish_pack(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PackLifecycleRequest>,
) -> Result<Json<PackLifecycleResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .unpublish_pack(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn delete_pack(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PackLifecycleRequest>,
) -> Result<Json<PackLifecycleResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .delete_pack(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn subscribe_pack(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PackSubscriptionRequest>,
) -> Result<Json<PackSubscriptionResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .mutate_pack_subscription(bearer, PackSubscriptionMutationKind::Subscribe, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn unsubscribe_pack(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<PackSubscriptionRequest>,
) -> Result<Json<PackSubscriptionResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .mutate_pack_subscription(bearer, PackSubscriptionMutationKind::Unsubscribe, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn pack_subscriptions(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
) -> Result<Json<PackSubscriptionCatalog>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .pack_subscription_catalog(bearer)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn drain_packs(
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

#[derive(Debug, Deserialize)]
struct OutboxDrainRequest {
    limit: u32,
}

#[derive(Debug, Serialize)]
struct OutboxDrainResponse {
    dispatched: usize,
}

async fn drain_outbox(
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

async fn sync_reconcile(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
    Json(request): Json<SyncReconcileRequest>,
) -> Result<Json<SyncReconcileResponse>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry
        .reconcile_subscriptions(bearer, &request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

async fn health_live() -> StatusCode {
    StatusCode::OK
}

async fn health_ready(State(registry): State<Arc<Registry>>) -> Response {
    match registry.readiness().await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(_) => {
            tracing::warn!(target: "denju_server::health", "registry_readiness_failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiError::new(
                    ApiErrorCode::Unavailable,
                    "registry dependencies are unavailable",
                )),
            )
                .into_response()
        }
    }
}

async fn capabilities(State(registry): State<Arc<Registry>>) -> Json<RegistryCapabilities> {
    Json(registry.capabilities())
}

async fn create_installation(
    State(registry): State<Arc<Registry>>,
    Json(request): Json<CreateInstallationRequest>,
) -> Result<Json<CreateInstallationResponse>, ApiResponseError> {
    registry
        .create_installation(&request)
        .await
        .map(Json)
        .map_err(ApiResponseError)
}

struct ApiResponseError(ApiError);

impl IntoResponse for ApiResponseError {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            ApiErrorCode::InvalidRequest | ApiErrorCode::InvalidRequestHash => {
                StatusCode::BAD_REQUEST
            }
            ApiErrorCode::OperationConflict => StatusCode::CONFLICT,
            ApiErrorCode::GenerationConflict => StatusCode::CONFLICT,
            ApiErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiErrorCode::NotFound => StatusCode::NOT_FOUND,
            ApiErrorCode::QuotaExceeded => StatusCode::PAYLOAD_TOO_LARGE,
            ApiErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            ApiErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self.0)).into_response()
    }
}
