use std::{
    fs,
    path::Path,
    process::ExitCode,
    str::FromStr,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use denju_client::RegistryClient;
use denju_core::OperationId;
use denju_local::{
    BootstrapJournal, BootstrapJournalPayload, CredentialBackend, CredentialManager, HarnessConfig,
    InstallCredential, InstallationRecord, JournalState, LocalDatabase, LocalPaths,
    ResolvedHarnessRoots, ServiceInstallMode, ServiceManager, ServiceStatus, WorkspaceStatus,
    WorkspaceWatcher, detect_unmanaged_skills, ensure_local_layout, prepare_harness_roots,
    reconcile_harness_projections, recover_local_lifecycle, remove_old_codex_projection,
    resolve_harness_roots, verify_native_directory_links,
};
use denju_wire::{
    CliErrorCode, CreateInstallationRequest, RegistryCapabilities, create_installation_request_hash,
};
use serde::Serialize;
use url::Url;
use uuid::Uuid;

pub const OFFICIAL_REGISTRY: &str = "https://registry.denju.ashray.xyz";

#[derive(Debug, Clone, Serialize)]
pub struct SetupOutcome {
    pub state: &'static str,
    pub registry: String,
    pub installation_id: String,
    pub author_principal_id: String,
    pub codex_root: String,
    pub claude_root: String,
    pub credential_backend: String,
    pub service_kind: String,
    pub service_persistent: bool,
    pub service_running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_detail: Option<String>,
    pub unmanaged_skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorOutcome {
    pub healthy: bool,
    pub registry: String,
    pub repaired: Vec<String>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DaemonHealthSnapshot {
    version: u32,
    updated_at_unix_ms: i64,
    iterations: u64,
    watcher_mode: &'static str,
    last_scan_full_hash: bool,
    full_hash_scans_total: u64,
    capture_errors_total: u64,
    remote_sync_errors_total: u64,
    last_capture_duration_ms: u64,
    last_remote_sync_duration_ms: u64,
}

#[derive(Debug, Clone)]
pub enum Guidance {
    SetupRequired,
    RepairRequired,
    ClaimAvailable,
    LoginRequired(String),
    Conflict(String),
    Healthy,
}

#[derive(Debug)]
pub struct RuntimeError {
    pub code: CliErrorCode,
    pub message: String,
    pub recovery: Option<String>,
}

impl RuntimeError {
    pub(crate) fn new(code: CliErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            recovery: None,
        }
    }

    pub(crate) fn recovery(mut self, command: impl Into<String>) -> Self {
        self.recovery = Some(command.into());
        self
    }
}

pub async fn guidance() -> Guidance {
    let Ok(paths) = LocalPaths::discover() else {
        return Guidance::RepairRequired;
    };
    if !paths.state_db.is_file() {
        return Guidance::SetupRequired;
    }
    let Ok(db) = LocalDatabase::open(&paths.state_db).await else {
        return Guidance::RepairRequired;
    };
    if db.bootstrap_journal().await.ok().flatten().is_some() {
        return Guidance::RepairRequired;
    }
    let Ok(Some(_)) = db.installation().await else {
        return Guidance::SetupRequired;
    };
    let Ok(Some(_)) = db.service().await else {
        return Guidance::RepairRequired;
    };
    let Ok(service) = ServiceManager::status(&paths) else {
        return Guidance::RepairRequired;
    };
    if !service.running {
        return Guidance::RepairRequired;
    }
    match db.identity().await {
        Ok(None) => return Guidance::ClaimAvailable,
        Ok(Some(identity)) if identity.session_id.is_none() => {
            return Guidance::LoginRequired(identity.username);
        }
        Err(_) => return Guidance::RepairRequired,
        Ok(Some(_)) => {}
    }
    if let Ok(states) = db.workspace_states().await
        && let Some(conflict) = states
            .into_iter()
            .find(|state| state.status == WorkspaceStatus::Conflict)
    {
        let locator = db
            .owned_skills()
            .await
            .ok()
            .and_then(|skills| {
                skills
                    .into_iter()
                    .find(|skill| skill.resource_id == conflict.resource_id)
                    .map(|skill| skill.locator)
            })
            .unwrap_or(conflict.resource_id);
        return Guidance::Conflict(locator);
    }
    if let Ok(conflicts) = db.pack_source_conflicts().await
        && let Some(conflict) = conflicts.into_iter().next()
    {
        let locator = db
            .pack_subscriptions()
            .await
            .ok()
            .and_then(|packs| {
                packs
                    .into_iter()
                    .find(|pack| conflict.source_ids.contains(&pack.source_id))
                    .map(|pack| pack.source_label)
            })
            .unwrap_or(conflict.resource_id);
        return Guidance::Conflict(locator);
    }
    Guidance::Healthy
}

pub async fn setup(requested_registry: Option<String>) -> Result<SetupOutcome, RuntimeError> {
    let paths = LocalPaths::discover().map_err(local_error)?;
    ensure_local_layout(&paths).map_err(local_error)?;
    verify_native_directory_links(&paths).map_err(local_error)?;
    let db = LocalDatabase::open(&paths.state_db)
        .await
        .map_err(local_error)?;

    if let Some(journal) = db.bootstrap_journal().await.map_err(local_error)? {
        if journal.state == JournalState::Planned {
            db.discard_planned_bootstrap(journal.operation_id)
                .await
                .map_err(local_error)?;
        } else {
            if let Some(requested) = requested_registry.as_deref() {
                ensure_same_registry(requested, &journal.payload.registry_origin)?;
            }
            resume_bootstrap(&paths, &db, journal).await?;
        }
    }

    if let Some(existing) = db.installation().await.map_err(local_error)? {
        if let Some(requested) = requested_registry.as_deref() {
            ensure_same_registry(requested, &existing.registry_origin)?;
        }
        return reconcile_existing(&paths, &db, existing).await;
    }

    // Reject invalid/ambiguous harness configurations before generating credentials or
    // creating any registry-side identity. Local projection failures must not orphan an
    // otherwise unreachable anonymous installation.
    let recorded = db.harness_config().await.map_err(local_error)?;
    resolve_harness_roots(&paths, recorded.as_ref()).map_err(local_error)?;

    let registry_origin =
        normalize_registry(requested_registry.as_deref().unwrap_or(OFFICIAL_REGISTRY))?;
    let client = checked_registry_client(&registry_origin).await?;
    let credential = InstallCredential::generate();
    let credential_hash = credential.sha256_hex();
    let operation_id = OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let mut payload = BootstrapJournalPayload {
        registry_origin: registry_origin.clone(),
        credential_hash: credential_hash.clone(),
        credential_backend: None,
        installation_id: None,
        author_principal_id: None,
    };
    db.create_bootstrap_journal(operation_id, payload.clone(), now_unix_ms())
        .await
        .map_err(local_error)?;

    let backend = CredentialManager::store(&paths, &credential, force_file_credentials())
        .map_err(credential_error)?;
    payload.credential_backend = Some(backend.as_str().to_owned());
    db.update_bootstrap(
        operation_id,
        JournalState::Planned,
        JournalState::Staged,
        payload.clone(),
        now_unix_ms(),
    )
    .await
    .map_err(local_error)?;

    let response = register_installation(&client, operation_id, &credential_hash).await?;
    payload.installation_id = Some(response.installation_id);
    payload.author_principal_id = Some(response.author_principal_id);
    db.update_bootstrap(
        operation_id,
        JournalState::Staged,
        JournalState::Verified,
        payload.clone(),
        now_unix_ms(),
    )
    .await
    .map_err(local_error)?;

    activate_verified_bootstrap(&paths, &db, operation_id, payload).await?;
    let existing = db
        .installation()
        .await
        .map_err(local_error)?
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "setup did not persist installation state",
            )
        })?;
    reconcile_existing(&paths, &db, existing).await
}

pub async fn doctor() -> Result<DoctorOutcome, RuntimeError> {
    let paths = LocalPaths::discover().map_err(local_error)?;
    let mut repaired = Vec::new();
    let mut issues = Vec::new();
    if !paths.root.is_dir() {
        ensure_local_layout(&paths).map_err(local_error)?;
        repaired.push("created missing Denju local directories".to_owned());
    }
    verify_native_directory_links(&paths).map_err(local_error)?;
    if !paths.state_db.is_file() {
        return Err(
            RuntimeError::new(CliErrorCode::SetupRequired, "Denju is not set up")
                .recovery("denju setup"),
        );
    }
    let db = LocalDatabase::open(&paths.state_db)
        .await
        .map_err(local_error)?;
    db.quick_check().await.map_err(local_error)?;

    if db.bootstrap_journal().await.map_err(local_error)?.is_some() {
        setup(None).await?;
        repaired.push("recovered interrupted setup operation".to_owned());
    }
    let installation = db
        .installation()
        .await
        .map_err(local_error)?
        .ok_or_else(|| {
            RuntimeError::new(CliErrorCode::SetupRequired, "Denju is not set up")
                .recovery("denju setup")
        })?;
    let backend =
        CredentialBackend::from_str(&installation.credential_backend).map_err(credential_error)?;
    CredentialManager::load(&paths, backend).map_err(credential_error)?;
    if backend == CredentialBackend::File {
        CredentialManager::verify_file_permissions(&paths).map_err(credential_error)?;
    }
    if let Some(identity) = db.identity().await.map_err(local_error)? {
        if let Some(session_backend) = identity.session_backend.as_deref() {
            let session_backend =
                CredentialBackend::from_str(session_backend).map_err(credential_error)?;
            let session = CredentialManager::load_session(&paths, session_backend)
                .map_err(credential_error)?;
            if session_backend == CredentialBackend::File {
                CredentialManager::verify_session_file_permissions(&paths)
                    .map_err(credential_error)?;
            }
            let origin = Url::parse(&installation.registry_origin).map_err(registry_error)?;
            let client = RegistryClient::authenticated(origin, session.bearer_token())
                .map_err(registry_error)?;
            client.identity().await.map_err(registry_error)?;
        } else {
            issues.push(format!(
                "{} has no active session; run denju login {}",
                identity.username, identity.username
            ));
        }
    }

    let recorded = db.harness_config().await.map_err(local_error)?;
    let expected = resolve_harness_roots(&paths, recorded.as_ref()).map_err(local_error)?;
    let harness_repair_needed = !expected.codex_root.is_dir()
        || !expected.claude_root.is_dir()
        || recorded.as_ref().is_none_or(|recorded| {
            recorded.codex_root != expected.codex_root.display().to_string()
                || recorded.claude_root != expected.claude_root.display().to_string()
        });
    let roots = prepare_current_harness_roots(&paths, &db).await?;
    if harness_repair_needed {
        repaired.push("repaired harness projection roots".to_owned());
    }
    recover_local_lifecycle(&paths, &db, &roots)
        .await
        .map_err(local_error)?;

    let service = ServiceManager::status(&paths).map_err(service_error)?;
    if !service.running && service_mode() == ServiceInstallMode::Start {
        let executable = std::env::current_exe().map_err(local_error)?;
        let restarted = ServiceManager::install_and_start(&paths, &executable, service_mode())
            .map_err(service_error)?;
        db.save_service(restarted.to_record())
            .await
            .map_err(local_error)?;
        repaired.push("restarted Denju background service".to_owned());
    } else if !service.running {
        issues.push("background service is not running (test install-only mode)".to_owned());
    }

    let client = checked_registry_client(&installation.registry_origin).await?;
    client.ready().await.map_err(registry_error)?;
    Ok(DoctorOutcome {
        healthy: issues.is_empty(),
        registry: installation.registry_origin,
        repaired,
        issues,
    })
}

pub async fn daemon() -> Result<ExitCode, RuntimeError> {
    let paths = LocalPaths::discover().map_err(local_error)?;
    ensure_local_layout(&paths).map_err(local_error)?;
    let db = LocalDatabase::open(&paths.state_db)
        .await
        .map_err(local_error)?;
    if db.installation().await.map_err(local_error)?.is_none() {
        return Err(RuntimeError::new(
            CliErrorCode::SetupRequired,
            "daemon cannot start before setup",
        )
        .recovery("denju setup"));
    }
    let roots = prepare_current_harness_roots(&paths, &db).await?;

    fs::write(paths.run.join("daemon.pid"), std::process::id().to_string()).map_err(local_error)?;
    let _guard = RunFileGuard(paths.run.join("daemon.pid"));
    let mut watcher = WorkspaceWatcher::start(&paths).ok();
    let mut force_full_hash = false;
    let mut polling_ticks = 0_u8;
    let mut iterations = 0_u64;
    let mut full_hash_scans = 0_u64;
    let mut capture_errors = 0_u64;
    let mut remote_sync_errors = 0_u64;
    loop {
        db.quick_check().await.map_err(local_error)?;
        recover_local_lifecycle(&paths, &db, &roots)
            .await
            .map_err(local_error)?;
        fs::write(paths.run.join("daemon.health"), now_unix_ms().to_string())
            .map_err(local_error)?;
        if daemon_once() {
            return Ok(ExitCode::SUCCESS);
        }
        // Filesystem notifications are only latency hints. This bounded scan is the
        // authoritative fallback and also lets valid owned edits become durable local
        // revisions while the registry is temporarily unavailable.
        let scan_full_hash = force_full_hash;
        full_hash_scans = full_hash_scans.saturating_add(u64::from(scan_full_hash));
        let capture_started = Instant::now();
        if crate::workspace::capture_local_edits(&paths, &db, scan_full_hash)
            .await
            .is_err()
        {
            capture_errors = capture_errors.saturating_add(1);
        }
        let capture_duration_ms = duration_millis(capture_started.elapsed());
        // Remote synchronization is opportunistic background work. The daemon stays alive
        // through registry/network outages; the foreground `denju sync` path surfaces them.
        let remote_sync_started = Instant::now();
        if crate::proposals::sync_once().await.is_err() {
            remote_sync_errors = remote_sync_errors.saturating_add(1);
        }
        let remote_sync_duration_ms = duration_millis(remote_sync_started.elapsed());
        iterations = iterations.saturating_add(1);
        write_daemon_health(
            &paths,
            &DaemonHealthSnapshot {
                version: 1,
                updated_at_unix_ms: now_unix_ms(),
                iterations,
                watcher_mode: if watcher.is_some() {
                    "native"
                } else {
                    "polling"
                },
                last_scan_full_hash: scan_full_hash,
                full_hash_scans_total: full_hash_scans,
                capture_errors_total: capture_errors,
                remote_sync_errors_total: remote_sync_errors,
                last_capture_duration_ms: capture_duration_ms,
                last_remote_sync_duration_ms: remote_sync_duration_ms,
            },
        )?;
        if let Some(native) = watcher.as_mut() {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return Ok(ExitCode::SUCCESS),
                remote = crate::public::wait_for_remote_hint() => {
                    if remote.is_err() {
                        // Provider recycle, EOF, and network loss are normal for SSE. The next
                        // loop performs an authoritative reconcile before reconnecting; a short
                        // delay prevents an unavailable registry from becoming a busy loop.
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    force_full_hash = false;
                }
                hint = native.changed() => {
                    // Collapse editor temp-file/write/rename bursts into one coherent scan.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    force_full_hash = native.drain_full_scan_hint(hint);
                }
                _ = tokio::time::sleep(Duration::from_secs(10)) => {
                    polling_ticks = polling_ticks.wrapping_add(1);
                    // Low-frequency content verification catches timestamp-preserving writes
                    // and any missed native events without making every poll rehash the tree.
                    force_full_hash = polling_ticks.is_multiple_of(30);
                }
            }
        } else {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return Ok(ExitCode::SUCCESS),
                remote = crate::public::wait_for_remote_hint() => {
                    if remote.is_err() {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    force_full_hash = false;
                }
                _ = tokio::time::sleep(Duration::from_secs(10)) => {
                    polling_ticks = polling_ticks.wrapping_add(1);
                    force_full_hash = polling_ticks.is_multiple_of(30);
                }
            }
        }
    }
}

async fn resume_bootstrap(
    paths: &LocalPaths,
    db: &LocalDatabase,
    mut journal: BootstrapJournal,
) -> Result<(), RuntimeError> {
    let backend_name = journal
        .payload
        .credential_backend
        .as_deref()
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "staged setup is missing its credential backend",
            )
        })?;
    let backend = CredentialBackend::from_str(backend_name).map_err(credential_error)?;
    let credential = CredentialManager::load(paths, backend).map_err(credential_error)?;
    if credential.sha256_hex() != journal.payload.credential_hash {
        return Err(RuntimeError::new(
            CliErrorCode::CredentialUnavailable,
            "stored installation credential does not match interrupted setup state",
        )
        .recovery("denju doctor"));
    }

    if journal.state == JournalState::Staged {
        let client = checked_registry_client(&journal.payload.registry_origin).await?;
        let response = register_installation(
            &client,
            journal.operation_id,
            &journal.payload.credential_hash,
        )
        .await?;
        journal.payload.installation_id = Some(response.installation_id);
        journal.payload.author_principal_id = Some(response.author_principal_id);
        db.update_bootstrap(
            journal.operation_id,
            JournalState::Staged,
            JournalState::Verified,
            journal.payload.clone(),
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
        journal.state = JournalState::Verified;
    }
    if journal.state == JournalState::Verified {
        activate_verified_bootstrap(paths, db, journal.operation_id, journal.payload).await?;
    } else if journal.state == JournalState::Switched {
        verify_switched_bootstrap(paths, db, journal.operation_id, journal.payload).await?;
    }
    Ok(())
}

async fn activate_verified_bootstrap(
    paths: &LocalPaths,
    db: &LocalDatabase,
    operation_id: OperationId,
    payload: BootstrapJournalPayload,
) -> Result<(), RuntimeError> {
    let installation_id = payload.installation_id.clone().ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "verified setup is missing installation_id",
        )
    })?;
    let author_principal_id = payload.author_principal_id.clone().ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "verified setup is missing author_principal_id",
        )
    })?;
    let credential_backend = payload.credential_backend.clone().ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "verified setup is missing credential backend",
        )
    })?;
    prepare_current_harness_roots(paths, db).await?;

    let executable = std::env::current_exe().map_err(local_error)?;
    let service = ServiceManager::install_and_start(paths, &executable, service_mode())
        .map_err(service_error)?;
    db.save_service(service.to_record())
        .await
        .map_err(local_error)?;
    db.save_installation(InstallationRecord {
        registry_origin: payload.registry_origin.clone(),
        installation_id,
        author_principal_id,
        credential_backend,
        created_at_unix_ms: now_unix_ms(),
    })
    .await
    .map_err(local_error)?;
    db.update_bootstrap(
        operation_id,
        JournalState::Verified,
        JournalState::Switched,
        payload.clone(),
        now_unix_ms(),
    )
    .await
    .map_err(local_error)?;
    verify_switched_bootstrap(paths, db, operation_id, payload).await
}

async fn verify_switched_bootstrap(
    paths: &LocalPaths,
    db: &LocalDatabase,
    operation_id: OperationId,
    payload: BootstrapJournalPayload,
) -> Result<(), RuntimeError> {
    db.quick_check().await.map_err(local_error)?;
    let installation = db
        .installation()
        .await
        .map_err(local_error)?
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "switched setup has no installation record",
            )
        })?;
    let backend =
        CredentialBackend::from_str(&installation.credential_backend).map_err(credential_error)?;
    let credential = CredentialManager::load(paths, backend).map_err(credential_error)?;
    if credential.sha256_hex() != payload.credential_hash {
        return Err(RuntimeError::new(
            CliErrorCode::CredentialUnavailable,
            "stored credential failed setup verification",
        ));
    }
    let harness = db
        .harness_config()
        .await
        .map_err(local_error)?
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "switched setup has no harness configuration",
            )
        })?;
    if !Path::new(&harness.codex_root).is_dir() || !Path::new(&harness.claude_root).is_dir() {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            "configured harness roots are unavailable after setup",
        ));
    }
    db.update_bootstrap(
        operation_id,
        JournalState::Switched,
        JournalState::Complete,
        payload,
        now_unix_ms(),
    )
    .await
    .map_err(local_error)
}

async fn reconcile_existing(
    paths: &LocalPaths,
    db: &LocalDatabase,
    installation: InstallationRecord,
) -> Result<SetupOutcome, RuntimeError> {
    let client = checked_registry_client(&installation.registry_origin).await?;
    client.ready().await.map_err(registry_error)?;
    let backend =
        CredentialBackend::from_str(&installation.credential_backend).map_err(credential_error)?;
    CredentialManager::load(paths, backend).map_err(credential_error)?;
    if backend == CredentialBackend::File {
        CredentialManager::verify_file_permissions(paths).map_err(credential_error)?;
    }

    let roots = prepare_current_harness_roots(paths, db).await?;
    let executable = std::env::current_exe().map_err(local_error)?;
    let service = ServiceManager::install_and_start(paths, &executable, service_mode())
        .map_err(service_error)?;
    db.save_service(service.to_record())
        .await
        .map_err(local_error)?;
    build_setup_outcome(paths, installation, roots, service, backend)
}

fn build_setup_outcome(
    paths: &LocalPaths,
    installation: InstallationRecord,
    roots: ResolvedHarnessRoots,
    service: ServiceStatus,
    backend: CredentialBackend,
) -> Result<SetupOutcome, RuntimeError> {
    let unmanaged = detect_unmanaged_skills(paths, &roots).map_err(local_error)?;
    Ok(SetupOutcome {
        state: "ready",
        registry: installation.registry_origin,
        installation_id: installation.installation_id,
        author_principal_id: installation.author_principal_id,
        codex_root: roots.codex_root.display().to_string(),
        claude_root: roots.claude_root.display().to_string(),
        credential_backend: backend.as_str().to_owned(),
        service_kind: service.kind.as_str().to_owned(),
        service_persistent: service.persistent,
        service_running: service.running,
        service_detail: service.detail,
        unmanaged_skills: unmanaged
            .into_iter()
            .map(|path| path.display().to_string())
            .collect(),
    })
}

async fn checked_registry_client(origin: &str) -> Result<RegistryClient, RuntimeError> {
    let url = Url::parse(origin).map_err(|error| {
        RuntimeError::new(
            CliErrorCode::InvalidArguments,
            format!("invalid registry URL: {error}"),
        )
    })?;
    let client = RegistryClient::new(url).map_err(registry_error)?;
    client.ready().await.map_err(registry_error)?;
    let capabilities = client.capabilities().await.map_err(registry_error)?;
    validate_capabilities(&capabilities)?;
    Ok(client)
}

fn validate_capabilities(capabilities: &RegistryCapabilities) -> Result<(), RuntimeError> {
    if capabilities.api_version != "v1" || !capabilities.object_store_required {
        return Err(RuntimeError::new(
            CliErrorCode::RegistryUnavailable,
            "registry does not satisfy the Denju v1 capability contract",
        ));
    }
    Ok(())
}

async fn register_installation(
    client: &RegistryClient,
    operation_id: OperationId,
    credential_hash: &str,
) -> Result<denju_wire::CreateInstallationResponse, RuntimeError> {
    let request_hash = create_installation_request_hash(&operation_id.to_string(), credential_hash)
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    client
        .create_installation(&CreateInstallationRequest {
            operation_id: operation_id.to_string(),
            credential_hash: credential_hash.to_owned(),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(registry_error)
}

fn harness_config(roots: &ResolvedHarnessRoots) -> HarnessConfig {
    HarnessConfig {
        codex_root: roots.codex_root.display().to_string(),
        claude_root: roots.claude_root.display().to_string(),
    }
}

pub(crate) async fn prepare_current_harness_roots(
    paths: &LocalPaths,
    db: &LocalDatabase,
) -> Result<ResolvedHarnessRoots, RuntimeError> {
    let recorded = db.harness_config().await.map_err(local_error)?;
    let roots = resolve_harness_roots(paths, recorded.as_ref()).map_err(local_error)?;

    // Build and validate the new view before removing a recorded legacy Codex projection.
    // This keeps root migration fail-closed and makes a binary upgrade enough to move users
    // from $CODEX_HOME/skills/denju to the shared ~/.agents/skills location.
    prepare_harness_roots(&roots).map_err(local_error)?;
    recover_local_lifecycle(paths, db, &roots)
        .await
        .map_err(local_error)?;
    reconcile_harness_projections(paths, db, &roots)
        .await
        .map_err(local_error)?;
    remove_old_codex_projection(recorded.as_ref(), &roots).map_err(local_error)?;
    db.save_harness_config(harness_config(&roots))
        .await
        .map_err(local_error)?;
    Ok(roots)
}

fn ensure_same_registry(requested: &str, recorded: &str) -> Result<(), RuntimeError> {
    let requested = normalize_registry(requested)?;
    let recorded = normalize_registry(recorded)?;
    if requested != recorded {
        return Err(RuntimeError::new(
            CliErrorCode::RegistryLocked,
            format!("this installation is already bound to {recorded}"),
        ));
    }
    Ok(())
}

fn normalize_registry(value: &str) -> Result<String, RuntimeError> {
    let mut url = Url::parse(value).map_err(|error| {
        RuntimeError::new(
            CliErrorCode::InvalidArguments,
            format!("invalid registry URL: {error}"),
        )
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.cannot_be_a_base()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "registry must be an http(s) origin",
        ));
    }
    if url.path() != "/" && !url.path().is_empty() {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "registry URL must be an origin without an API path",
        ));
    }
    url.set_path("");
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn now_unix_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn write_daemon_health(
    paths: &LocalPaths,
    snapshot: &DaemonHealthSnapshot,
) -> Result<(), RuntimeError> {
    let bytes = serde_json::to_vec(snapshot)
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let temporary = paths.run.join("daemon.metrics.json.tmp");
    fs::write(&temporary, bytes).map_err(local_error)?;
    fs::rename(temporary, paths.run.join("daemon.metrics.json")).map_err(local_error)
}

fn force_file_credentials() -> bool {
    std::env::var_os(denju_local::TEST_HOME_ENV).is_some()
        || std::env::var_os("DENJU_TEST_FILE_CREDENTIALS").is_some()
}

fn service_mode() -> ServiceInstallMode {
    if std::env::var_os(denju_local::TEST_HOME_ENV).is_some()
        || std::env::var_os("DENJU_TEST_SERVICE_INSTALL_ONLY").is_some()
    {
        ServiceInstallMode::InstallOnly
    } else {
        ServiceInstallMode::Start
    }
}

fn daemon_once() -> bool {
    std::env::var_os("DENJU_DAEMON_ONCE").is_some()
}

fn local_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::LocalState, error.to_string()).recovery("denju doctor")
}

fn credential_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::CredentialUnavailable, error.to_string())
        .recovery("denju doctor")
}

fn service_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::ServiceUnavailable, error.to_string()).recovery("denju doctor")
}

fn registry_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::RegistryUnavailable, error.to_string()).recovery("denju doctor")
}

struct RunFileGuard(std::path::PathBuf);

impl Drop for RunFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
