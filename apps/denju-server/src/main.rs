use std::{net::SocketAddr, process::ExitCode, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::{Parser, Subcommand};
use denju_registry::{Registry, RegistrySettings};
use denju_wire::{
    ApiError, ApiErrorCode, CreateInstallationRequest, CreateInstallationResponse,
    RegistryCapabilities, RegistryLimits,
};
use url::Url;

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
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("denju-server: {error}");
            ExitCode::FAILURE
        }
    }
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
    s3_bucket: String,
    s3_endpoint: Url,
    s3_region: String,
    s3_access_key_id: String,
    s3_secret_access_key: String,
    s3_force_path_style: bool,
    limits: RegistryLimits,
}

impl ServerConfig {
    fn from_env() -> Result<Self, String> {
        let bind = env_or("DENJU_BIND", "127.0.0.1:7788")
            .parse()
            .map_err(|error| format!("invalid DENJU_BIND: {error}"))?;
        let public_origin = parse_http_url("DENJU_PUBLIC_URL", &required_env("DENJU_PUBLIC_URL")?)?;
        let database_url = required_env("DENJU_DATABASE_URL")?;
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
        })
    }

    fn registry_settings(&self) -> RegistrySettings {
        // S3 credentials are deliberately validated here but are not passed into the
        // registry domain until Phase 5 introduces object transfers.
        let _ = (
            &self.s3_bucket,
            &self.s3_region,
            &self.s3_access_key_id,
            &self.s3_secret_access_key,
            self.s3_force_path_style,
        );
        RegistrySettings {
            database_url: self.database_url.clone(),
            public_origin: self.public_origin.clone(),
            object_store_endpoint: self.s3_endpoint.clone(),
            limits: self.limits.clone(),
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
