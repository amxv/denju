use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use clap::{Parser, Subcommand};
use denju_core::{OwnedSkillEntry, build_deterministic_skill_snapshot};
use denju_registry::{Registry, RegistrySettings};
use denju_wire::RegistryLimits;
use url::Url;
use walkdir::WalkDir;

mod http;
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

    let app = http::router(registry);
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|error| format!("failed to bind {}: {error}", config.bind))?;
    eprintln!("denju-server listening on {}", config.bind);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| error.to_string())
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
