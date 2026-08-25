use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use denju_client::RegistryClient;
use denju_local::{
    LocalDatabase, LocalPaths, ServiceInstallMode, ServiceManager, TEST_HOME_ENV,
    prepare_harness_roots, resolve_harness_roots, verify_native_directory_links,
};
use denju_wire::CliErrorCode;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::setup::RuntimeError;

const MANIFEST_FORMAT: &str = "denju-release-manifest-v1";
const MANIFEST_NAME: &str = "release-manifest.txt";
const OFFICIAL_RELEASES: &str = "https://github.com/amxv/denju/releases";
const OFFICIAL_SERVER_IMAGE: &str = "ghcr.io/amxv/denju-server";
const CLIENT_ASSETS: [&str; 6] = [
    "denju_darwin_amd64",
    "denju_darwin_arm64",
    "denju_linux_amd64",
    "denju_linux_arm64",
    "denju_windows_amd64.exe",
    "denju_windows_arm64.exe",
];

#[derive(Debug, Clone, Serialize)]
pub(crate) struct UpgradeOutcome {
    pub state: &'static str,
    pub source: &'static str,
    pub previous_version: String,
    pub version: String,
    pub daemon_restarted: bool,
    pub health_verified: bool,
    pub rolled_back: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseManifest {
    version: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseAsset {
    name: String,
    sha256: String,
    size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallSource {
    Npm,
    Standalone,
}

impl InstallSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Standalone => "standalone",
        }
    }
}

#[derive(Debug, Deserialize)]
struct InstallSourceFile {
    version: u32,
    source: String,
}

pub(crate) async fn upgrade(current_version: &str) -> Result<UpgradeOutcome, RuntimeError> {
    let paths = LocalPaths::discover().map_err(local_error)?;
    let source = installation_source(&paths)?;
    match source {
        InstallSource::Npm => upgrade_npm(&paths, current_version).await,
        InstallSource::Standalone => upgrade_standalone(&paths, current_version).await,
    }
}

pub(crate) fn upgrade_text(outcome: &UpgradeOutcome) -> String {
    if outcome.state == "up_to_date" {
        format!("Denju {} is already up to date.", outcome.version)
    } else {
        format!(
            "Upgraded Denju {} -> {} via {}; health verified.",
            outcome.previous_version, outcome.version, outcome.source
        )
    }
}

pub(crate) async fn health_check() -> Result<(), RuntimeError> {
    let paths = LocalPaths::discover().map_err(local_error)?;
    if std::env::var_os(TEST_HOME_ENV).is_some()
        && std::env::var_os("DENJU_TEST_UPGRADE_HEALTH_FAIL").is_some()
    {
        return Err(RuntimeError::new(
            CliErrorCode::ServiceUnavailable,
            "injected upgrade health failure",
        ));
    }
    if !paths.state_db.is_file() {
        return Ok(());
    }
    verify_native_directory_links(&paths).map_err(local_error)?;
    let db = LocalDatabase::open(&paths.state_db)
        .await
        .map_err(local_error)?;
    db.quick_check().await.map_err(local_error)?;
    let recorded = db.harness_config().await.map_err(local_error)?;
    let roots = resolve_harness_roots(&paths, recorded.as_ref()).map_err(local_error)?;
    prepare_harness_roots(&roots).map_err(local_error)?;
    let installation = db
        .installation()
        .await
        .map_err(local_error)?
        .ok_or_else(|| RuntimeError::new(CliErrorCode::SetupRequired, "Denju is not set up"))?;
    let origin = Url::parse(&installation.registry_origin)
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
    RegistryClient::new(origin)
        .map_err(registry_error)?
        .ready()
        .await
        .map_err(registry_error)
}

async fn upgrade_standalone(
    paths: &LocalPaths,
    current_version: &str,
) -> Result<UpgradeOutcome, RuntimeError> {
    let client = release_client()?;
    let override_base = test_release_base()?;
    let manifest_url = override_base.as_ref().map_or_else(
        || format!("{OFFICIAL_RELEASES}/latest/download/{MANIFEST_NAME}"),
        |base| format!("{base}/{MANIFEST_NAME}"),
    );
    let manifest = download_text(&client, &manifest_url).await?;
    let manifest = parse_manifest(&manifest)?;
    if manifest.version == current_version {
        return Ok(UpgradeOutcome {
            state: "up_to_date",
            source: InstallSource::Standalone.as_str(),
            previous_version: current_version.to_owned(),
            version: current_version.to_owned(),
            daemon_restarted: false,
            health_verified: true,
            rolled_back: false,
        });
    }
    let asset_name = platform_asset_name()?;
    let asset = manifest
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::ContentVerification,
                format!("release manifest has no asset for {asset_name}"),
            )
        })?;
    let asset_url = override_base.map_or_else(
        || {
            format!(
                "{OFFICIAL_RELEASES}/download/v{}/{asset_name}",
                manifest.version
            )
        },
        |base| format!("{base}/{asset_name}"),
    );
    let bytes = download_bytes(&client, &asset_url).await?;
    verify_asset(asset, &bytes)?;

    let target = standalone_target()?;
    apply_standalone_upgrade(paths, current_version, &manifest.version, &target, &bytes)
}

fn apply_standalone_upgrade(
    paths: &LocalPaths,
    current_version: &str,
    next_version: &str,
    target: &Path,
    bytes: &[u8],
) -> Result<UpgradeOutcome, RuntimeError> {
    let backup = temporary_sibling(target, "upgrade-backup")?;
    fs::copy(target, &backup).map_err(local_error)?;
    let install_result = install_verified_bytes(target, bytes)
        .and_then(|()| {
            let installed_version = binary_version(target)?;
            if installed_version != next_version {
                return Err(RuntimeError::new(
                    CliErrorCode::ContentVerification,
                    format!(
                        "release manifest declared version {next_version} but the staged binary reports {installed_version}"
                    ),
                ));
            }
            restart_daemon(paths, target)
        })
        .and_then(|restarted| run_new_binary_health(target).map(|()| restarted));
    match install_result {
        Ok(daemon_restarted) => {
            let _ = fs::remove_file(&backup);
            Ok(UpgradeOutcome {
                state: "upgraded",
                source: InstallSource::Standalone.as_str(),
                previous_version: current_version.to_owned(),
                version: next_version.to_owned(),
                daemon_restarted,
                health_verified: true,
                rolled_back: false,
            })
        }
        Err(error) => {
            rollback_executable(paths, target, &backup)?;
            let restored_version = binary_version(target)?;
            if restored_version != current_version {
                return Err(RuntimeError::new(
                    CliErrorCode::ServiceUnavailable,
                    format!(
                        "upgrade failed and rollback restored unexpected Denju version {restored_version}; expected {current_version}"
                    ),
                )
                .recovery("reinstall Denju from the verified release installer"));
            }
            Err(RuntimeError::new(
                CliErrorCode::ServiceUnavailable,
                format!("upgrade health verification failed and the previous executable was restored: {}", error.message),
            )
            .recovery("denju doctor"))
        }
    }
}

async fn upgrade_npm(
    paths: &LocalPaths,
    current_version: &str,
) -> Result<UpgradeOutcome, RuntimeError> {
    let package = std::env::var("DENJU_INSTALL_PACKAGE").unwrap_or_else(|_| "denju-cli".to_owned());
    let package_version =
        std::env::var("DENJU_INSTALL_VERSION").unwrap_or_else(|_| current_version.to_owned());
    let target = std::env::var_os("DENJU_INSTALL_TARGET")
        .map(PathBuf::from)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "npm launcher did not provide the native Denju target path",
            )
            .recovery(npm_install_command(&package, &package_version))
        })?;
    let npm = npm_command();
    apply_npm_upgrade(
        paths,
        current_version,
        &package,
        &package_version,
        &target,
        &npm,
    )
}

fn apply_npm_upgrade(
    paths: &LocalPaths,
    current_version: &str,
    package: &str,
    package_version: &str,
    target: &Path,
    npm: &std::ffi::OsStr,
) -> Result<UpgradeOutcome, RuntimeError> {
    if package_version != current_version {
        return Err(RuntimeError::new(
            CliErrorCode::ContentVerification,
            format!(
                "npm package reports version {package_version} but the running Denju binary reports {current_version}"
            ),
        )
        .recovery(npm_install_command(package, package_version)));
    }
    let status = npm_install(npm, package, "latest")?;
    if !status.success() {
        return Err(RuntimeError::new(
            CliErrorCode::ServiceUnavailable,
            format!("npm upgrade exited with {status}"),
        )
        .recovery(npm_install_command(package, "latest")));
    }
    let new_version = match binary_version(target) {
        Ok(version) => version,
        Err(error) => {
            return rollback_npm_upgrade(paths, package, package_version, target, npm, error);
        }
    };
    if new_version == current_version {
        return Ok(UpgradeOutcome {
            state: "up_to_date",
            source: InstallSource::Npm.as_str(),
            previous_version: current_version.to_owned(),
            version: new_version,
            daemon_restarted: false,
            health_verified: true,
            rolled_back: false,
        });
    }
    let verification = restart_daemon(paths, target)
        .and_then(|restarted| run_new_binary_health(target).map(|()| restarted));
    match verification {
        Ok(daemon_restarted) => Ok(UpgradeOutcome {
            state: "upgraded",
            source: InstallSource::Npm.as_str(),
            previous_version: current_version.to_owned(),
            version: new_version,
            daemon_restarted,
            health_verified: true,
            rolled_back: false,
        }),
        Err(error) => rollback_npm_upgrade(paths, package, package_version, target, npm, error),
    }
}

fn rollback_npm_upgrade(
    paths: &LocalPaths,
    package: &str,
    package_version: &str,
    target: &Path,
    npm: &std::ffi::OsStr,
    failure: RuntimeError,
) -> Result<UpgradeOutcome, RuntimeError> {
    let rollback_status = npm_install(npm, package, package_version)?;
    if !rollback_status.success() {
        return Err(RuntimeError::new(
            CliErrorCode::ServiceUnavailable,
            format!(
                "upgrade verification failed ({}) and npm rollback exited with {rollback_status}",
                failure.message
            ),
        )
        .recovery(npm_install_command(package, package_version)));
    }
    let restored_version = binary_version(target).map_err(|error| {
        RuntimeError::new(
            CliErrorCode::ServiceUnavailable,
            format!(
                "upgrade failed and npm rollback could not verify the restored Denju executable: {}",
                error.message
            ),
        )
        .recovery(npm_install_command(package, package_version))
    })?;
    if restored_version != package_version {
        return Err(RuntimeError::new(
            CliErrorCode::ServiceUnavailable,
            format!(
                "upgrade failed and npm rollback restored Denju {restored_version}; expected {package_version}"
            ),
        )
        .recovery(npm_install_command(package, package_version)));
    }
    if let Err(rollback_error) =
        restart_daemon(paths, target).and_then(|_| run_restored_binary_health(target))
    {
        return Err(RuntimeError::new(
            CliErrorCode::ServiceUnavailable,
            format!(
                "upgrade failed; npm restored Denju {package_version}, but rollback health verification also failed: {}",
                rollback_error.message
            ),
        )
        .recovery("denju doctor"));
    }
    Err(RuntimeError::new(
        CliErrorCode::ServiceUnavailable,
        format!(
            "upgrade verification failed ({}); npm restored the previous package version",
            failure.message
        ),
    )
    .recovery("denju doctor"))
}

fn installation_source(paths: &LocalPaths) -> Result<InstallSource, RuntimeError> {
    if std::env::var("DENJU_INSTALL_SOURCE").as_deref() == Ok("npm") {
        return Ok(InstallSource::Npm);
    }
    let path = paths.root.join("install-source.json");
    let bytes = fs::read(&path).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "Denju installation source is unknown",
        )
        .recovery("reinstall Denju with install/denju.sh, install/denju.ps1, or denju-cli")
    })?;
    let source: InstallSourceFile = serde_json::from_slice(&bytes).map_err(|error| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            format!("invalid install source metadata: {error}"),
        )
    })?;
    if source.version != 1 || source.source != "standalone" {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            "unsupported Denju installation source metadata",
        ));
    }
    Ok(InstallSource::Standalone)
}

fn release_client() -> Result<Client, RuntimeError> {
    Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(registry_error)
}

fn test_release_base() -> Result<Option<String>, RuntimeError> {
    let Ok(value) = std::env::var("DENJU_RELEASE_BASE_URL") else {
        return Ok(None);
    };
    if std::env::var_os(TEST_HOME_ENV).is_none() {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "DENJU_RELEASE_BASE_URL is available only inside marked Denju test homes",
        ));
    }
    let url = Url::parse(&value)
        .map_err(|error| RuntimeError::new(CliErrorCode::InvalidArguments, error.to_string()))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "test release base must use http or https",
        ));
    }
    Ok(Some(value.trim_end_matches('/').to_owned()))
}

async fn download_text(client: &Client, url: &str) -> Result<String, RuntimeError> {
    let response = client.get(url).send().await.map_err(registry_error)?;
    if !response.status().is_success() {
        return Err(RuntimeError::new(
            CliErrorCode::RegistryUnavailable,
            format!("release manifest download returned {}", response.status()),
        ));
    }
    response.text().await.map_err(registry_error)
}

async fn download_bytes(client: &Client, url: &str) -> Result<Vec<u8>, RuntimeError> {
    let response = client.get(url).send().await.map_err(registry_error)?;
    if !response.status().is_success() {
        return Err(RuntimeError::new(
            CliErrorCode::RegistryUnavailable,
            format!("release asset download returned {}", response.status()),
        ));
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(registry_error)
}

fn parse_manifest(text: &str) -> Result<ReleaseManifest, RuntimeError> {
    let mut format = None;
    let mut version = None;
    let mut assets = Vec::new();
    let mut server_image = None;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        match fields.as_slice() {
            ["format", value] => {
                if format.replace((*value).to_owned()).is_some() {
                    return Err(manifest_error("manifest format is duplicated"));
                }
            }
            ["version", value] => {
                if version.replace((*value).to_owned()).is_some() {
                    return Err(manifest_error("manifest version is duplicated"));
                }
            }
            ["asset", name, sha256, size] => {
                if !CLIENT_ASSETS.contains(name) {
                    return Err(manifest_error(format!(
                        "manifest contains unsupported client asset {name}"
                    )));
                }
                if assets
                    .iter()
                    .any(|asset: &ReleaseAsset| asset.name == *name)
                {
                    return Err(manifest_error(format!(
                        "manifest client asset {name} is duplicated"
                    )));
                }
                if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(manifest_error("asset SHA-256 is invalid"));
                }
                let size = size
                    .parse::<u64>()
                    .map_err(|_| manifest_error("asset size is invalid"))?;
                assets.push(ReleaseAsset {
                    name: (*name).to_owned(),
                    sha256: sha256.to_ascii_lowercase(),
                    size,
                });
            }
            ["server_image", value] => {
                if server_image.replace((*value).to_owned()).is_some() {
                    return Err(manifest_error("manifest server image is duplicated"));
                }
            }
            _ => return Err(manifest_error("manifest contains an invalid line")),
        }
    }
    if format.as_deref() != Some(MANIFEST_FORMAT) {
        return Err(manifest_error("manifest format is unsupported"));
    }
    let version = version.ok_or_else(|| manifest_error("manifest version is missing"))?;
    validate_release_version(&version)?;
    if assets.len() != CLIENT_ASSETS.len() {
        return Err(manifest_error(format!(
            "manifest must contain exactly {} client assets",
            CLIENT_ASSETS.len()
        )));
    }
    for expected in CLIENT_ASSETS {
        if !assets.iter().any(|asset| asset.name == expected) {
            return Err(manifest_error(format!(
                "manifest is missing client asset {expected}"
            )));
        }
    }
    let expected_server_image = format!("{OFFICIAL_SERVER_IMAGE}:v{version}");
    if server_image.as_deref() != Some(expected_server_image.as_str()) {
        return Err(manifest_error(format!(
            "manifest server image must be {expected_server_image}"
        )));
    }
    Ok(ReleaseManifest { version, assets })
}

fn validate_release_version(version: &str) -> Result<(), RuntimeError> {
    let valid = !version.is_empty()
        && version.len() <= 64
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'));
    if valid {
        Ok(())
    } else {
        Err(manifest_error("manifest version is invalid"))
    }
}

fn verify_asset(asset: &ReleaseAsset, bytes: &[u8]) -> Result<(), RuntimeError> {
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if size != asset.size {
        return Err(manifest_error("release asset size mismatch"));
    }
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    if sha256 != asset.sha256 {
        return Err(manifest_error("release asset SHA-256 mismatch"));
    }
    Ok(())
}

fn standalone_target() -> Result<PathBuf, RuntimeError> {
    if let Some(target) = std::env::var_os("DENJU_INSTALL_TARGET") {
        if std::env::var_os(TEST_HOME_ENV).is_none() {
            return Err(RuntimeError::new(
                CliErrorCode::InvalidArguments,
                "DENJU_INSTALL_TARGET is available only inside marked Denju test homes",
            ));
        }
        return Ok(PathBuf::from(target));
    }
    std::env::current_exe().map_err(local_error)
}

fn install_verified_bytes(target: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    target.parent().ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "Denju executable has no parent directory",
        )
    })?;
    let staged = temporary_sibling(target, "upgrade-stage")?;
    fs::write(&staged, bytes).map_err(local_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).map_err(local_error)?;
    }
    #[cfg(windows)]
    {
        let retired = temporary_sibling(target, "upgrade-retired")?;
        fs::rename(target, &retired).map_err(|error| {
            RuntimeError::new(
                CliErrorCode::ServiceUnavailable,
                format!("cannot replace the running Denju executable on Windows: {error}"),
            )
            .recovery("rerun install/denju.ps1 after closing Denju processes")
        })?;
        if let Err(error) = fs::rename(&staged, target) {
            let _ = fs::rename(&retired, target);
            return Err(local_error(error));
        }
        let _ = fs::remove_file(retired);
    }
    #[cfg(not(windows))]
    fs::rename(&staged, target).map_err(local_error)?;
    Ok(())
}

fn temporary_sibling(target: &Path, purpose: &str) -> Result<PathBuf, RuntimeError> {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "Denju executable file name is not valid UTF-8",
            )
        })?;
    let nonce = hex::encode(rand::random::<[u8; 8]>());
    Ok(target.with_file_name(format!(
        ".denju-{purpose}-{}-{nonce}-{file_name}",
        std::process::id()
    )))
}

fn rollback_executable(
    paths: &LocalPaths,
    target: &Path,
    backup: &Path,
) -> Result<(), RuntimeError> {
    let bytes = fs::read(backup).map_err(local_error)?;
    install_verified_bytes(target, &bytes)?;
    let _ = fs::remove_file(backup);
    restart_daemon(paths, target)?;
    run_restored_binary_health(target)?;
    Ok(())
}

fn restart_daemon(paths: &LocalPaths, target: &Path) -> Result<bool, RuntimeError> {
    if !paths.state_db.is_file() {
        return Ok(false);
    }
    let mode = if std::env::var_os(TEST_HOME_ENV).is_some() {
        ServiceInstallMode::InstallOnly
    } else {
        ServiceInstallMode::Start
    };
    ServiceManager::install_and_start(paths, target, mode)
        .map(|status| status.running)
        .map_err(service_error)
}

fn run_new_binary_health(target: &Path) -> Result<(), RuntimeError> {
    run_binary_health(target, true)
}

fn run_restored_binary_health(target: &Path) -> Result<(), RuntimeError> {
    run_binary_health(target, false)
}

fn run_binary_health(target: &Path, allow_injected_failure: bool) -> Result<(), RuntimeError> {
    let mut command = Command::new(target);
    command
        .arg("upgrade-health")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if !allow_injected_failure && std::env::var_os(TEST_HOME_ENV).is_some() {
        command.env_remove("DENJU_TEST_UPGRADE_HEALTH_FAIL");
    }
    let status = command.status().map_err(service_error)?;
    if status.success() {
        Ok(())
    } else {
        Err(RuntimeError::new(
            CliErrorCode::ServiceUnavailable,
            format!("upgraded Denju health check exited with {status}"),
        ))
    }
}

fn binary_version(target: &Path) -> Result<String, RuntimeError> {
    let output = Command::new(target)
        .arg("--version")
        .output()
        .map_err(service_error)?;
    if !output.status.success() {
        return Err(RuntimeError::new(
            CliErrorCode::ServiceUnavailable,
            format!("upgraded Denju version check exited with {}", output.status),
        ));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|error| RuntimeError::new(CliErrorCode::ServiceUnavailable, error.to_string()))?;
    text.trim()
        .strip_prefix("denju ")
        .map(str::to_owned)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::ServiceUnavailable,
                "invalid Denju version output",
            )
        })
}

fn npm_command() -> std::ffi::OsString {
    if std::env::var_os(TEST_HOME_ENV).is_some() {
        std::env::var_os("DENJU_NPM_COMMAND").unwrap_or_else(|| "npm".into())
    } else {
        "npm".into()
    }
}

fn npm_install(
    npm: &std::ffi::OsStr,
    package: &str,
    version: &str,
) -> Result<std::process::ExitStatus, RuntimeError> {
    let allow_scripts = format!("--allow-scripts={package}");
    Command::new(npm)
        .args([
            "install",
            "-g",
            allow_scripts.as_str(),
            &format!("{package}@{version}"),
        ])
        // npm is an operational child process, not part of Denju's result stream. Keep both
        // streams private so `denju --json upgrade` remains exactly one JSON envelope with no
        // progress contamination; Denju reports the child exit status and recovery command on
        // failure instead.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(service_error)
}

fn npm_install_command(package: &str, version: &str) -> String {
    format!("npm install -g --allow-scripts={package} {package}@{version}")
}

fn platform_asset_name() -> Result<String, RuntimeError> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        other => {
            return Err(RuntimeError::new(
                CliErrorCode::InvalidArguments,
                format!("unsupported Denju operating system: {other}"),
            ));
        }
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => {
            return Err(RuntimeError::new(
                CliErrorCode::InvalidArguments,
                format!("unsupported Denju architecture: {other}"),
            ));
        }
    };
    let extension = if os == "windows" { ".exe" } else { "" };
    Ok(format!("denju_{os}_{arch}{extension}"))
}

fn manifest_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::new(CliErrorCode::ContentVerification, message)
}

fn local_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::LocalState, error.to_string()).recovery("denju doctor")
}

fn registry_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::RegistryUnavailable, error.to_string()).recovery("denju doctor")
}

fn service_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::ServiceUnavailable, error.to_string()).recovery("denju doctor")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_manifest_binds_exact_size_and_sha() {
        let bytes = b"denju-binary";
        let sha = format!("{:x}", Sha256::digest(bytes));
        let text = valid_manifest("2.3.4", &sha, bytes.len());
        let manifest = parse_manifest(&text).unwrap();
        assert_eq!(manifest.version, "2.3.4");
        let asset = manifest
            .assets
            .iter()
            .find(|asset| asset.name == "denju_linux_amd64")
            .unwrap();
        verify_asset(asset, bytes).unwrap();
        assert!(verify_asset(asset, b"denju-binarx").is_err());
    }

    #[test]
    fn malformed_release_manifest_fails_closed() {
        assert!(parse_manifest("version 1.0.0\n").is_err());
        assert!(
            parse_manifest("format denju-release-manifest-v1\nversion 1.0.0\nasset x nope 10\n")
                .is_err()
        );
        let sha = "0".repeat(64);
        let manifest = valid_manifest("1.0.0", &sha, 10);
        assert!(parse_manifest(&format!("{manifest}future_field nope\n")).is_err());
        assert!(parse_manifest(&format!("{manifest}version 1.0.0\n")).is_err());
        assert!(
            parse_manifest(&manifest.replace(
                "server_image ghcr.io/amxv/denju-server:v1.0.0",
                "server_image ghcr.io/example/denju-server:v1.0.0"
            ))
            .is_err()
        );
        assert!(parse_manifest(&manifest.replace("version 1.0.0", "version ../1.0.0")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn standalone_upgrade_replaces_only_after_verification_and_runs_new_health() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = LocalPaths::from_home(temporary.path().to_path_buf());
        let target = temporary.path().join("denju");
        write_fake_binary(&target, "1.0.0", true);
        let next = fake_binary("2.0.0", true);

        let outcome = apply_standalone_upgrade(&paths, "1.0.0", "2.0.0", &target, &next).unwrap();

        assert_eq!(outcome.state, "upgraded");
        assert_eq!(outcome.previous_version, "1.0.0");
        assert_eq!(outcome.version, "2.0.0");
        assert!(outcome.health_verified);
        assert!(!outcome.daemon_restarted);
        assert_eq!(binary_version(&target).unwrap(), "2.0.0");
    }

    #[cfg(unix)]
    #[test]
    fn failed_new_binary_health_restores_the_exact_previous_executable() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = LocalPaths::from_home(temporary.path().to_path_buf());
        let target = temporary.path().join("denju");
        let previous = fake_binary("1.0.0", true);
        write_executable(&target, &previous);
        let unhealthy = fake_binary("2.0.0", false);

        let error =
            apply_standalone_upgrade(&paths, "1.0.0", "2.0.0", &target, &unhealthy).unwrap_err();

        assert!(error.message.contains("previous executable was restored"));
        assert_eq!(binary_version(&target).unwrap(), "1.0.0");
        assert_eq!(fs::read(&target).unwrap(), previous);
    }

    #[cfg(unix)]
    #[test]
    fn npm_upgrade_runs_new_health_and_reports_the_new_version() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = LocalPaths::from_home(temporary.path().to_path_buf());
        let target = temporary.path().join("denju");
        let previous = temporary.path().join("previous");
        let next = temporary.path().join("next");
        let npm = temporary.path().join("npm");
        write_fake_binary(&target, "1.0.0", true);
        write_fake_binary(&previous, "1.0.0", true);
        write_fake_binary(&next, "2.0.0", true);
        write_fake_npm(&npm, &target, &previous, &next);

        let outcome = apply_npm_upgrade(
            &paths,
            "1.0.0",
            "denju-cli",
            "1.0.0",
            &target,
            npm.as_os_str(),
        )
        .unwrap();

        assert_eq!(outcome.state, "upgraded");
        assert_eq!(outcome.version, "2.0.0");
        assert_eq!(binary_version(&target).unwrap(), "2.0.0");
    }

    #[cfg(unix)]
    #[test]
    fn npm_upgrade_verification_failure_reinstalls_and_health_checks_previous_version() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = LocalPaths::from_home(temporary.path().to_path_buf());
        let target = temporary.path().join("denju");
        let previous = temporary.path().join("previous");
        let next = temporary.path().join("next");
        let npm = temporary.path().join("npm");
        write_fake_binary(&target, "1.0.0", true);
        write_fake_binary(&previous, "1.0.0", true);
        write_fake_binary(&next, "2.0.0", false);
        write_fake_npm(&npm, &target, &previous, &next);

        let error = apply_npm_upgrade(
            &paths,
            "1.0.0",
            "denju-cli",
            "1.0.0",
            &target,
            npm.as_os_str(),
        )
        .unwrap_err();

        assert!(
            error
                .message
                .contains("npm restored the previous package version")
        );
        assert_eq!(fs::read(&target).unwrap(), fs::read(&previous).unwrap());
        assert_eq!(binary_version(&target).unwrap(), "1.0.0");
    }

    #[cfg(unix)]
    fn write_fake_binary(path: &Path, version: &str, healthy: bool) {
        write_executable(path, &fake_binary(version, healthy));
    }

    #[cfg(unix)]
    fn write_executable(path: &Path, bytes: &[u8]) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    fn write_fake_npm(npm: &Path, target: &Path, previous: &Path, next: &Path) {
        let script = format!(
            "#!/bin/sh\nset -eu\ncase \"$4\" in\n  denju-cli@latest) cp '{}' '{}' ;;\n  denju-cli@1.0.0) cp '{}' '{}' ;;\n  *) exit 3 ;;\nesac\nchmod 755 '{}'\n",
            next.display(),
            target.display(),
            previous.display(),
            target.display(),
            target.display()
        );
        write_executable(npm, script.as_bytes());
    }

    #[cfg(unix)]
    fn fake_binary(version: &str, healthy: bool) -> Vec<u8> {
        let health = if healthy { 0 } else { 1 };
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) echo 'denju {version}' ;;\n  upgrade-health) exit {health} ;;\n  *) exit 2 ;;\nesac\n"
        )
        .into_bytes()
    }

    fn valid_manifest(version: &str, sha: &str, size: usize) -> String {
        let mut text = format!("format {MANIFEST_FORMAT}\nversion {version}\n");
        for name in CLIENT_ASSETS {
            text.push_str(&format!("asset {name} {sha} {size}\n"));
        }
        text.push_str(&format!(
            "server_image {OFFICIAL_SERVER_IMAGE}:v{version}\n"
        ));
        text
    }
}
