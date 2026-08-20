use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    process::{Command, Stdio},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use denju_client::{ClientError, RegistryClient};
use denju_core::OperationId;
use denju_local::{
    CredentialBackend, CredentialManager, IdentityRecord, InstallCredential, InstallationRecord,
    LocalDatabase, LocalPaths, SessionCredential,
};
use denju_wire::{
    AccountDeleteRequest, ApiErrorCode, AutomationTokenCreateRequest,
    AutomationTokenCreateResponse, AutomationTokenList, AutomationTokenRevokeRequest,
    AutomationTokenRevokeResponse, ClaimIdentityRequest, CliErrorCode, DeviceList,
    DeviceRevokeRequest, DeviceRevokeResponse, IdentityBackupRequest, IdentityInfo,
    IdentityMutationDomain, IdentitySessionResponse, LoginRequest, RecoveryResetRequest,
    identity_mutation_request_hash,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use crate::{public, setup::RuntimeError};

#[derive(Debug, Clone, Serialize)]
pub struct ClaimOutcome {
    pub state: &'static str,
    #[serde(flatten)]
    pub identity: IdentitySessionResponse,
    pub recovery_secret: String,
    pub sync: public::SyncOutcome,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginOutcome {
    pub state: &'static str,
    #[serde(flatten)]
    pub identity: IdentitySessionResponse,
    pub sync: public::SyncOutcome,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryOutcome {
    pub state: &'static str,
    #[serde(flatten)]
    pub identity: IdentitySessionResponse,
    pub recovery_secret: String,
    pub sync: public::SyncOutcome,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupOutcome {
    pub state: &'static str,
    pub recovery_secret: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutomationTokenOutcome {
    #[serde(flatten)]
    pub token: AutomationTokenCreateResponse,
    pub secret: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteOutcome {
    pub state: &'static str,
    pub username: String,
    pub removed_local_skills: usize,
}

pub async fn claim(username: &str, json: bool) -> Result<ClaimOutcome, RuntimeError> {
    require_interactive(json, "denju claim requires hidden password input")?;
    let context = installation_context().await?;
    if context.db.identity().await.map_err(local_error)?.is_some() {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "this device is already associated with an identity",
        ));
    }
    let password = prompt_new_password()?;
    let session = SessionCredential::generate();
    let recovery = OneTimeSecret::generate();
    let operation_id = new_operation_id()?;
    let session_hash = session.sha256_hex();
    let recovery_hash = recovery.sha256_hex();
    let device_name = device_name();
    let request_hash = identity_mutation_request_hash(
        &operation_id,
        IdentityMutationDomain::Claim,
        &(
            username,
            session_hash.as_str(),
            recovery_hash.as_str(),
            device_name.as_str(),
        ),
    )
    .map_err(internal_error)?;
    let response = context
        .client
        .claim_identity(&ClaimIdentityRequest {
            operation_id,
            username: username.to_owned(),
            password,
            session_token_hash: session_hash,
            recovery_secret_hash: recovery_hash,
            device_name,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    persist_session(&context, &response, &session).await?;
    let sync = public::sync_once().await?;
    Ok(ClaimOutcome {
        state: "claimed",
        identity: response,
        recovery_secret: recovery.value(),
        sync,
    })
}

pub async fn login(username: &str, json: bool) -> Result<LoginOutcome, RuntimeError> {
    require_interactive(json, "denju login requires hidden password input")?;
    let context = installation_context().await?;
    let password = prompt_password("Password: ")?;
    let session = SessionCredential::generate();
    let operation_id = new_operation_id()?;
    let session_hash = session.sha256_hex();
    let device_name = device_name();
    let request_hash = identity_mutation_request_hash(
        &operation_id,
        IdentityMutationDomain::Login,
        &(username, session_hash.as_str(), device_name.as_str()),
    )
    .map_err(internal_error)?;
    let response = context
        .client
        .login(&LoginRequest {
            operation_id,
            username: username.to_owned(),
            password,
            session_token_hash: session_hash,
            device_name,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    persist_session(&context, &response, &session).await?;
    let sync = public::sync_once().await?;
    Ok(LoginOutcome {
        state: "logged_in",
        identity: response,
        sync,
    })
}

pub async fn recover(username: &str, json: bool) -> Result<RecoveryOutcome, RuntimeError> {
    require_interactive(
        json,
        "identity recovery requires hidden secret and password input",
    )?;
    let context = installation_context().await?;
    let recovery_secret = prompt_password("Recovery secret: ")?;
    let password = prompt_new_password()?;
    let session = SessionCredential::generate();
    let replacement_recovery = OneTimeSecret::generate();
    let operation_id = new_operation_id()?;
    let session_hash = session.sha256_hex();
    let replacement_hash = replacement_recovery.sha256_hex();
    let device_name = device_name();
    let request_hash = identity_mutation_request_hash(
        &operation_id,
        IdentityMutationDomain::RecoveryReset,
        &(
            username,
            session_hash.as_str(),
            replacement_hash.as_str(),
            device_name.as_str(),
        ),
    )
    .map_err(internal_error)?;
    let response = context
        .client
        .recovery_reset(&RecoveryResetRequest {
            operation_id,
            username: username.to_owned(),
            recovery_secret,
            new_password: password,
            session_token_hash: session_hash,
            replacement_recovery_secret_hash: replacement_hash,
            device_name,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    persist_session(&context, &response, &session).await?;
    let sync = public::sync_once().await?;
    Ok(RecoveryOutcome {
        state: "recovered",
        identity: response,
        recovery_secret: replacement_recovery.value(),
        sync,
    })
}

pub async fn backup(json: bool) -> Result<BackupOutcome, RuntimeError> {
    require_interactive(json, "identity backup requires hidden password input")?;
    let context = session_context().await?;
    let password = prompt_password("Password: ")?;
    let recovery = OneTimeSecret::generate();
    let replacement_hash = recovery.sha256_hex();
    let operation_id = new_operation_id()?;
    let request_hash = identity_mutation_request_hash(
        &operation_id,
        IdentityMutationDomain::Backup,
        &replacement_hash,
    )
    .map_err(internal_error)?;
    context
        .client
        .identity_backup(&IdentityBackupRequest {
            operation_id,
            password,
            replacement_recovery_secret_hash: replacement_hash,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    Ok(BackupOutcome {
        state: "recovery_secret_rotated",
        recovery_secret: recovery.value(),
    })
}

pub async fn info() -> Result<IdentityInfo, RuntimeError> {
    session_context()
        .await?
        .client
        .identity()
        .await
        .map_err(client_error)
}

pub async fn devices() -> Result<DeviceList, RuntimeError> {
    session_context()
        .await?
        .client
        .devices()
        .await
        .map_err(client_error)
}

pub async fn revoke_device(session_id: &str) -> Result<DeviceRevokeResponse, RuntimeError> {
    let context = session_context().await?;
    let operation_id = new_operation_id()?;
    let request_hash = identity_mutation_request_hash(
        &operation_id,
        IdentityMutationDomain::DeviceRevoke,
        &session_id,
    )
    .map_err(internal_error)?;
    let response = context
        .client
        .revoke_device(&DeviceRevokeRequest {
            operation_id,
            session_id: session_id.to_owned(),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    if response.revoked
        && context.identity.session_id.as_deref() == Some(response.session_id.as_str())
    {
        let backend = context.session_backend;
        CredentialManager::delete_session(&context.paths, backend).map_err(credential_error)?;
        context
            .db
            .clear_identity_session(now_unix_ms())
            .await
            .map_err(local_error)?;
    }
    Ok(response)
}

pub async fn create_automation_token(
    scopes: Vec<String>,
    expires_in_seconds: u64,
) -> Result<AutomationTokenOutcome, RuntimeError> {
    let context = session_context().await?;
    let secret = OneTimeSecret::generate();
    let token_hash = secret.sha256_hex();
    let operation_id = new_operation_id()?;
    let request_hash = identity_mutation_request_hash(
        &operation_id,
        IdentityMutationDomain::TokenCreate,
        &(token_hash.as_str(), &scopes, expires_in_seconds),
    )
    .map_err(internal_error)?;
    let token = context
        .client
        .create_automation_token(&AutomationTokenCreateRequest {
            operation_id,
            token_hash,
            scopes,
            expires_in_seconds,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    Ok(AutomationTokenOutcome {
        token,
        secret: secret.value(),
    })
}

pub async fn automation_tokens() -> Result<AutomationTokenList, RuntimeError> {
    session_context()
        .await?
        .client
        .automation_tokens()
        .await
        .map_err(client_error)
}

pub async fn revoke_automation_token(
    token_id: &str,
) -> Result<AutomationTokenRevokeResponse, RuntimeError> {
    let context = session_context().await?;
    let operation_id = new_operation_id()?;
    let request_hash = identity_mutation_request_hash(
        &operation_id,
        IdentityMutationDomain::TokenRevoke,
        &token_id,
    )
    .map_err(internal_error)?;
    context
        .client
        .revoke_automation_token(&AutomationTokenRevokeRequest {
            operation_id,
            token_id: token_id.to_owned(),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub async fn delete_account(json: bool, yes: bool) -> Result<DeleteOutcome, RuntimeError> {
    require_interactive(
        json,
        "account deletion requires confirmation and hidden password input",
    )?;
    if !yes && !confirm("Delete this Denju account? [y/N] ")? {
        return Err(RuntimeError::new(
            CliErrorCode::ConfirmationRequired,
            "account deletion was not confirmed",
        ));
    }
    let context = session_context().await?;
    let password = prompt_password("Password: ")?;
    let operation_id = new_operation_id()?;
    let request_hash =
        identity_mutation_request_hash(&operation_id, IdentityMutationDomain::AccountDelete, &())
            .map_err(internal_error)?;
    let response = context
        .client
        .delete_account(&AccountDeleteRequest {
            operation_id,
            password,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    let removed_local_skills = public::clear_local_managed_state().await?;
    CredentialManager::delete_session(&context.paths, context.session_backend)
        .map_err(credential_error)?;
    let install_backend = CredentialBackend::from_str(&context.installation.credential_backend)
        .map_err(credential_error)?;
    CredentialManager::delete_installation(&context.paths, install_backend)
        .map_err(credential_error)?;
    context.db.clear_identity().await.map_err(local_error)?;
    context.db.clear_installation().await.map_err(local_error)?;
    Ok(DeleteOutcome {
        state: "deleted",
        username: response.username,
        removed_local_skills,
    })
}

struct InstallationContext {
    paths: LocalPaths,
    db: LocalDatabase,
    installation: InstallationRecord,
    client: RegistryClient,
}

struct SessionContext {
    paths: LocalPaths,
    db: LocalDatabase,
    installation: InstallationRecord,
    identity: IdentityRecord,
    session_backend: CredentialBackend,
    client: RegistryClient,
}

async fn installation_context() -> Result<InstallationContext, RuntimeError> {
    let paths = LocalPaths::discover().map_err(local_error)?;
    if !paths.state_db.is_file() {
        return Err(
            RuntimeError::new(CliErrorCode::SetupRequired, "Denju is not set up")
                .recovery("denju setup"),
        );
    }
    let db = LocalDatabase::open(&paths.state_db)
        .await
        .map_err(local_error)?;
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
    let credential: InstallCredential =
        CredentialManager::load(&paths, backend).map_err(credential_error)?;
    let origin = Url::parse(&installation.registry_origin).map_err(local_error)?;
    let client =
        RegistryClient::authenticated(origin, credential.bearer_token()).map_err(client_error)?;
    Ok(InstallationContext {
        paths,
        db,
        installation,
        client,
    })
}

async fn session_context() -> Result<SessionContext, RuntimeError> {
    let installation = installation_context().await?;
    let identity = installation
        .db
        .identity()
        .await
        .map_err(local_error)?
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::CredentialUnavailable,
                "this device has no claimed identity session",
            )
            .recovery("denju claim <@username> or denju login <@username>")
        })?;
    let backend_name = identity.session_backend.as_deref().ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::CredentialUnavailable,
            format!("{} is not logged in on this device", identity.username),
        )
        .recovery(format!("denju login {}", identity.username))
    })?;
    let session_backend = CredentialBackend::from_str(backend_name).map_err(credential_error)?;
    let session = CredentialManager::load_session(&installation.paths, session_backend)
        .map_err(credential_error)?;
    let origin = Url::parse(&installation.installation.registry_origin).map_err(local_error)?;
    let client =
        RegistryClient::authenticated(origin, session.bearer_token()).map_err(client_error)?;
    Ok(SessionContext {
        paths: installation.paths,
        db: installation.db,
        installation: installation.installation,
        identity,
        session_backend,
        client,
    })
}

async fn persist_session(
    context: &InstallationContext,
    response: &IdentitySessionResponse,
    session: &SessionCredential,
) -> Result<(), RuntimeError> {
    let backend =
        CredentialManager::store_session(&context.paths, session, force_file_credentials())
            .map_err(credential_error)?;
    context
        .db
        .save_identity(
            IdentityRecord {
                user_id: response.user_id.clone(),
                namespace_id: response.namespace_id.clone(),
                username: response.username.clone(),
                session_id: Some(response.session_id.clone()),
                session_backend: Some(backend.as_str().to_owned()),
                author_principal_id: Some(response.author_principal_id.clone()),
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)
}

fn prompt_new_password() -> Result<String, RuntimeError> {
    let password = prompt_password("New password: ")?;
    if password.is_empty() {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "password cannot be empty",
        ));
    }
    let confirmation = prompt_password("Confirm password: ")?;
    if password != confirmation {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "password confirmation did not match",
        ));
    }
    Ok(password)
}

fn prompt_password(prompt: &str) -> Result<String, RuntimeError> {
    #[cfg(unix)]
    let result = {
        let _guard = UnixEchoGuard::disable()?;
        eprint!("{prompt}");
        io::stderr().flush().map_err(local_error)?;
        rpassword::read_password()
    };
    #[cfg(not(unix))]
    let result = rpassword::prompt_password(prompt);
    result.map_err(|error| {
        RuntimeError::new(
            CliErrorCode::InteractiveRequired,
            format!("cannot read hidden input: {error}"),
        )
    })
}

#[cfg(unix)]
struct UnixEchoGuard {
    tty: File,
    original: String,
}

#[cfg(unix)]
impl UnixEchoGuard {
    fn disable() -> Result<Self, RuntimeError> {
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(local_error)?;
        let original = stty_capture(&tty, &["-g"])?;
        stty_status(&tty, &["-echo"])?;
        Ok(Self { tty, original })
    }
}

#[cfg(unix)]
impl Drop for UnixEchoGuard {
    fn drop(&mut self) {
        let _ = stty_status(&self.tty, &[self.original.trim()]);
    }
}

#[cfg(unix)]
fn stty_capture(tty: &File, args: &[&str]) -> Result<String, RuntimeError> {
    let output = Command::new("stty")
        .args(args)
        .stdin(Stdio::from(tty.try_clone().map_err(local_error)?))
        .output()
        .map_err(local_error)?;
    if !output.status.success() {
        return Err(RuntimeError::new(
            CliErrorCode::InteractiveRequired,
            "failed to configure hidden terminal input",
        ));
    }
    String::from_utf8(output.stdout).map_err(local_error)
}

#[cfg(unix)]
fn stty_status(tty: &File, args: &[&str]) -> Result<(), RuntimeError> {
    let status = Command::new("stty")
        .args(args)
        .stdin(Stdio::from(tty.try_clone().map_err(local_error)?))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(local_error)?;
    if status.success() {
        Ok(())
    } else {
        Err(RuntimeError::new(
            CliErrorCode::InteractiveRequired,
            "failed to configure hidden terminal input",
        ))
    }
}

fn confirm(prompt: &str) -> Result<bool, RuntimeError> {
    print!("{prompt}");
    io::stdout().flush().map_err(local_error)?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).map_err(local_error)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn require_interactive(json: bool, message: &str) -> Result<(), RuntimeError> {
    if json {
        Err(RuntimeError::new(
            CliErrorCode::InteractiveRequired,
            message,
        ))
    } else {
        Ok(())
    }
}

fn new_operation_id() -> Result<String, RuntimeError> {
    OperationId::from_uuid(Uuid::now_v7())
        .map(|id| id.to_string())
        .map_err(internal_error)
}

fn device_name() -> String {
    std::env::var("DENJU_DEVICE_NAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "device".to_owned())
}

fn force_file_credentials() -> bool {
    std::env::var_os("DENJU_TEST_FILE_CREDENTIALS").is_some()
}

fn client_error(error: ClientError) -> RuntimeError {
    match &error {
        ClientError::Registry(api) if api.code == ApiErrorCode::Unauthorized => {
            RuntimeError::new(CliErrorCode::CredentialUnavailable, api.message.clone())
        }
        ClientError::Registry(api) if api.code == ApiErrorCode::NotFound => {
            RuntimeError::new(CliErrorCode::NotFound, api.message.clone())
        }
        ClientError::Registry(api) if api.code == ApiErrorCode::OperationConflict => {
            RuntimeError::new(CliErrorCode::InvalidArguments, api.message.clone())
        }
        _ => RuntimeError::new(CliErrorCode::RegistryUnavailable, error.to_string())
            .recovery("denju doctor"),
    }
}

fn credential_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::CredentialUnavailable, error.to_string())
}

fn local_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::LocalState, error.to_string()).recovery("denju doctor")
}

fn internal_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::Internal, error.to_string())
}

fn now_unix_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

struct OneTimeSecret([u8; 32]);

impl OneTimeSecret {
    fn generate() -> Self {
        Self(rand::random())
    }

    fn value(&self) -> String {
        hex::encode(self.0)
    }

    fn sha256_hex(&self) -> String {
        hex::encode(Sha256::digest(self.0))
    }
}
