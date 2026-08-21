use std::{
    collections::BTreeMap,
    convert::Infallible,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::sse::{Event, KeepAlive, Sse},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::{Parser, Subcommand};
use denju_core::{OwnedSkillEntry, build_deterministic_skill_snapshot};
use denju_registry::{Registry, RegistrySettings, RegistryWake};
use denju_wire::{
    AccountDeleteRequest, AccountDeleteResponse, ApiError, ApiErrorCode,
    AutomationTokenCreateRequest, AutomationTokenCreateResponse, AutomationTokenList,
    AutomationTokenRevokeRequest, AutomationTokenRevokeResponse, ClaimIdentityRequest,
    CreateInstallationRequest, CreateInstallationResponse, DeleteSkillResponse,
    DeprecateSkillRequest, DeprecateSkillResponse, DeviceList, DeviceRevokeRequest,
    DeviceRevokeResponse, HistoryPruneResponse, IdentityBackupRequest, IdentityInfo,
    IdentitySessionResponse, LoginRequest, PrivateRevisionCommitRequest,
    PrivateRevisionPrepareResponse, PrivateRevisionRequest, PrivateRevisionResponse,
    PrivateSkillCatalog, PrivateSkillImportCommitRequest, PrivateSkillImportPrepareResponse,
    PrivateSkillImportRequest, PrivateSkillImportResponse, PublicSkillDetail,
    PublicSkillSearchResponse, PublishSkillRequest, PublishSkillResponse, RecoveryResetRequest,
    RegistryCapabilities, RegistryLimits, RenameSkillRequest, RenameSkillResponse,
    ResourceLifecycleRequest, RestoreSkillRequest, RestoreSkillResponse, SkillHistoryResponse,
    SkillRevisionDetail, SubscriptionCatalog, SubscriptionMutationKind,
    SubscriptionMutationRequest, SubscriptionMutationResponse, SyncHint, SyncReconcileRequest,
    SyncReconcileResponse, UnpublishSkillResponse, UsageResponse,
};
use futures_util::stream;
use serde::Deserialize;
use url::Url;
use walkdir::WalkDir;

mod maintenance;

#[derive(Debug, Parser)]
#[command(name = "denju-server", version = build_version())]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve,
    Migrate,
    #[command(hide = true)]
    CheckObjectStore,
    #[command(hide = true)]
    Gc {
        #[arg(long, default_value_t = 256)]
        limit: u32,
    },
    #[command(hide = true)]
    SeedPublic {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = match ServerConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("denju-server: {error}");
            return ExitCode::FAILURE;
        }
    };

    let result = match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(config).await,
        Command::Migrate => migrate(&config).await,
        Command::CheckObjectStore => check_object_store(&config).await,
        Command::Gc { limit } => maintenance::gc(&config, limit).await,
        Command::SeedPublic { owner, path } => seed_public(&config, &owner, &path).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("denju-server: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn check_object_store(config: &ServerConfig) -> Result<(), String> {
    let registry = Registry::connect(config.registry_settings())
        .await
        .map_err(|error| error.to_string())?;
    registry
        .verify_object_store_provider()
        .await
        .map_err(|error| error.to_string())?;
    println!("object-store provider conformance passed");
    Ok(())
}

async fn seed_public(config: &ServerConfig, owner: &str, path: &Path) -> Result<(), String> {
    let registry = Registry::connect(config.registry_settings())
        .await
        .map_err(|error| error.to_string())?;
    registry
        .validate_schema()
        .await
        .map_err(|error| error.to_string())?;
    let entries = read_skill_directory(path)?;
    let directory_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "seed path must end in a UTF-8 skill directory name".to_owned())?;
    let snapshot = build_deterministic_skill_snapshot(directory_name, &entries)
        .map_err(|error| error.to_string())?;
    let seeded = registry
        .seed_public_skill(owner, &snapshot, &entries)
        .await
        .map_err(|error| error.to_string())?;
    println!("{}", seeded.skill.locator);
    Ok(())
}

async fn migrate(config: &ServerConfig) -> Result<(), String> {
    Registry::migrate(&config.database_url)
        .await
        .map_err(|error| error.to_string())?;
    println!("registry migrations applied");
    Ok(())
}

async fn serve(config: ServerConfig) -> Result<(), String> {
    let registry = Arc::new(
        Registry::connect(config.registry_settings())
            .await
            .map_err(|error| error.to_string())?,
    );
    registry
        .validate_schema()
        .await
        .map_err(|error| error.to_string())?;

    let app = Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
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
        .route("/v1/search", get(search_public_skills))
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
        .route("/v1/subscriptions/remove", post(unsubscribe))
        .route("/v1/sync/reconcile", post(sync_reconcile))
        .route("/v1/events", get(events))
        .with_state(registry);
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|error| format!("failed to bind {}: {error}", config.bind))?;
    eprintln!("denju-server listening on {}", config.bind);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| error.to_string())
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
    limit: Option<u32>,
    cursor: Option<String>,
}

async fn search_public_skills(
    State(registry): State<Arc<Registry>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<PublicSkillSearchResponse>, ApiResponseError> {
    registry
        .search_public_skills(&query.q, query.limit.unwrap_or(20), query.cursor.as_deref())
        .await
        .map(Json)
        .map_err(ApiResponseError)
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
    Query(query): Query<ShowQuery>,
) -> Result<Json<PublicSkillDetail>, ApiResponseError> {
    registry
        .show_public_skill(&query.locator)
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
) -> Result<Json<PrivateRevisionResponse>, ApiResponseError> {
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

async fn events(
    State(registry): State<Arc<Registry>>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry.ensure_wake_listener();
    let watched = registry
        .watched_resource_ids(bearer)
        .await
        .map_err(ApiResponseError)?;
    let receiver = registry.subscribe_wakes();
    let _ = registry.drain_outbox(256).await;
    let event_stream = stream::unfold((receiver, watched), |(mut receiver, watched)| async move {
        loop {
            match next_sync_hint(&mut receiver, &watched).await {
                Some(hint) => {
                    let event = Event::default()
                        .event("sync")
                        .json_data(hint)
                        .unwrap_or_else(|_| {
                            Event::default()
                                .event("sync")
                                .data("{\"kind\":\"resync_all\"}")
                        });
                    return Some((Ok(event), (receiver, watched)));
                }
                None if receiver.is_closed() => return None,
                None => continue,
            }
        }
    });
    Ok(Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

async fn next_sync_hint(
    receiver: &mut tokio::sync::broadcast::Receiver<RegistryWake>,
    watched: &std::collections::BTreeSet<uuid::Uuid>,
) -> Option<SyncHint> {
    const MAX_DIRTY: usize = 64;
    let first = match receiver.recv().await {
        Ok(wake) => wake,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
            return Some(SyncHint::ResyncAll);
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
    };
    if matches!(first, RegistryWake::ResyncAll) {
        return Some(SyncHint::ResyncAll);
    }
    let mut dirty = BTreeMap::<uuid::Uuid, u64>::new();
    if let RegistryWake::Resource {
        resource_id,
        generation,
    } = first
        && watched.contains(&resource_id)
    {
        dirty.insert(resource_id, generation);
    }
    loop {
        match receiver.try_recv() {
            Ok(RegistryWake::ResyncAll) => return Some(SyncHint::ResyncAll),
            Ok(RegistryWake::Resource {
                resource_id,
                generation,
            }) => {
                if watched.contains(&resource_id) {
                    dirty
                        .entry(resource_id)
                        .and_modify(|current| *current = (*current).max(generation))
                        .or_insert(generation);
                    if dirty.len() > MAX_DIRTY {
                        return Some(SyncHint::ResyncAll);
                    }
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                return Some(SyncHint::ResyncAll);
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
        }
    }
    if dirty.is_empty() {
        None
    } else {
        Some(SyncHint::Dirty {
            resources: dirty
                .into_iter()
                .map(|(resource_id, generation)| denju_wire::DirtyResource {
                    resource_id: resource_id.to_string(),
                    generation,
                })
                .collect(),
        })
    }
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, ApiResponseError> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiResponseError(ApiError::new(
                ApiErrorCode::Unauthorized,
                "installation credential required",
            ))
        })
}

fn optional_bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
}

async fn health_live() -> StatusCode {
    StatusCode::OK
}

async fn health_ready(State(registry): State<Arc<Registry>>) -> Response {
    match registry.readiness().await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError::new(ApiErrorCode::Unavailable, error.to_string())),
        )
            .into_response(),
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

#[derive(Debug, Clone)]
struct ServerConfig {
    bind: SocketAddr,
    public_origin: Url,
    database_url: String,
    database_listen_url: Option<String>,
    s3_bucket: String,
    s3_endpoint: Url,
    s3_region: String,
    s3_access_key_id: String,
    s3_secret_access_key: String,
    s3_force_path_style: bool,
    limits: RegistryLimits,
    gc_grace: Duration,
}

impl ServerConfig {
    fn from_env() -> Result<Self, String> {
        let bind = bind_address()
            .parse()
            .map_err(|error| format!("invalid DENJU_BIND: {error}"))?;
        let public_origin = parse_http_url("DENJU_PUBLIC_URL", &required_env("DENJU_PUBLIC_URL")?)?;
        let database_url = required_env("DENJU_DATABASE_URL")?;
        let database_listen_url = std::env::var("DENJU_DATABASE_DIRECT_URL")
            .ok()
            .filter(|value| !value.is_empty());
        let s3_bucket = required_env("DENJU_S3_BUCKET")?;
        let s3_endpoint = parse_http_url("DENJU_S3_ENDPOINT", &required_env("DENJU_S3_ENDPOINT")?)?;
        let s3_region = required_env("DENJU_S3_REGION")?;
        let s3_access_key_id = required_env("DENJU_S3_ACCESS_KEY_ID")?;
        let s3_secret_access_key = required_env("DENJU_S3_SECRET_ACCESS_KEY")?;
        let s3_force_path_style = env_or("DENJU_S3_FORCE_PATH_STYLE", "false")
            .parse::<bool>()
            .map_err(|error| format!("invalid DENJU_S3_FORCE_PATH_STYLE: {error}"))?;
        if s3_bucket.trim().is_empty() || s3_region.trim().is_empty() {
            return Err("S3 bucket and region must be non-empty".to_owned());
        }

        Ok(Self {
            bind,
            public_origin,
            database_url,
            database_listen_url,
            s3_bucket,
            s3_endpoint,
            s3_region,
            s3_access_key_id,
            s3_secret_access_key,
            s3_force_path_style,
            limits: RegistryLimits {
                max_object_bytes: env_u64("DENJU_LIMIT_MAX_OBJECT_BYTES", 16 * 1024 * 1024)?,
                max_release_bytes: env_u64("DENJU_LIMIT_MAX_RELEASE_BYTES", 10 * 1024 * 1024)?,
                namespace_storage_bytes: env_u64(
                    "DENJU_LIMIT_NAMESPACE_STORAGE_BYTES",
                    512 * 1024 * 1024,
                )?,
                max_transfer_bytes: env_u64("DENJU_LIMIT_MAX_TRANSFER_BYTES", 16 * 1024 * 1024)?,
            },
            gc_grace: Duration::from_secs(env_u64("DENJU_GC_GRACE_SECONDS", 86_400)?),
        })
    }

    fn registry_settings(&self) -> RegistrySettings {
        RegistrySettings {
            database_url: self.database_url.clone(),
            database_listen_url: self.database_listen_url.clone(),
            public_origin: self.public_origin.clone(),
            object_store_endpoint: self.s3_endpoint.clone(),
            object_store_bucket: self.s3_bucket.clone(),
            object_store_region: self.s3_region.clone(),
            object_store_access_key_id: self.s3_access_key_id.clone(),
            object_store_secret_access_key: self.s3_secret_access_key.clone(),
            object_store_force_path_style: self.s3_force_path_style,
            limits: self.limits.clone(),
            gc_grace: self.gc_grace,
        }
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn parse_http_url(name: &str, value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|error| format!("invalid {name}: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.cannot_be_a_base() {
        return Err(format!("{name} must be an http(s) origin"));
    }
    Ok(url)
}

fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn bind_address() -> String {
    if let Ok(bind) = std::env::var("DENJU_BIND") {
        return bind;
    }
    if let Ok(port) = std::env::var("PORT") {
        return format!("0.0.0.0:{port}");
    }
    "127.0.0.1:7788".to_owned()
}

fn read_skill_directory(root: &Path) -> Result<Vec<OwnedSkillEntry>, String> {
    if !root.is_dir() {
        return Err(format!("seed path is not a directory: {}", root.display()));
    }
    let mut entries = Vec::new();
    for item in WalkDir::new(root).follow_links(false).min_depth(1) {
        let item = item.map_err(|error| error.to_string())?;
        let relative = item
            .path()
            .strip_prefix(root)
            .map_err(|error| error.to_string())?;
        let path = relative
            .to_str()
            .ok_or_else(|| "seed skill contains a non-UTF-8 path".to_owned())?
            .replace('\\', "/");
        let file_type = item.file_type();
        if file_type.is_symlink() {
            let target = fs::read_link(item.path()).map_err(|error| error.to_string())?;
            let target = target
                .to_str()
                .ok_or_else(|| "seed skill contains a non-UTF-8 symlink target".to_owned())?
                .replace('\\', "/");
            entries.push(OwnedSkillEntry::Symlink { path, target });
        } else if file_type.is_dir() {
            entries.push(OwnedSkillEntry::Directory { path });
        } else if file_type.is_file() {
            let bytes = fs::read(item.path()).map_err(|error| error.to_string())?;
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                item.metadata()
                    .map_err(|error| error.to_string())?
                    .permissions()
                    .mode()
                    & 0o111
                    != 0
            };
            #[cfg(not(unix))]
            let executable = false;
            entries.push(OwnedSkillEntry::File {
                path,
                bytes,
                executable,
            });
        } else {
            return Err(format!("unsupported seed entry: {}", item.path().display()));
        }
    }
    Ok(entries)
}

fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| format!("invalid {name}: {error}")),
        Err(_) => Ok(default),
    }
}

fn build_version() -> &'static str {
    option_env!("DENJU_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests;
