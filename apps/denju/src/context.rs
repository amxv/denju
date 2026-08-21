use std::str::FromStr;

use denju_client::{ClientError, RegistryClient};
use denju_local::{
    CredentialBackend, CredentialManager, IdentityRecord, InstallCredential, InstallationRecord,
    LocalDatabase, LocalPaths, ResolvedHarnessRoots, SessionCredential, prepare_harness_roots,
    resolve_harness_roots,
};
use denju_wire::{ApiErrorCode, CliErrorCode, RegistryLimits};
use url::Url;

use crate::setup::RuntimeError;

pub(crate) struct InstalledContext {
    pub(crate) paths: LocalPaths,
    pub(crate) db: LocalDatabase,
    pub(crate) roots: ResolvedHarnessRoots,
    pub(crate) client: RegistryClient,
    pub(crate) limits: RegistryLimits,
}

pub(crate) async fn installed_context(
    authenticated: bool,
) -> Result<InstalledContext, RuntimeError> {
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
    let origin = Url::parse(&installation.registry_origin)
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
    let client = if authenticated {
        let bearer = load_active_bearer(&paths, &db, &installation).await?;
        RegistryClient::authenticated(origin, bearer).map_err(client_error)?
    } else {
        RegistryClient::new(origin).map_err(client_error)?
    };
    client.ready().await.map_err(client_error)?;
    let capabilities = client.capabilities().await.map_err(client_error)?;
    if capabilities.api_version != "v1" || !capabilities.object_store_required {
        return Err(RuntimeError::new(
            CliErrorCode::RegistryUnavailable,
            "registry does not satisfy the Denju v1 capability contract",
        ));
    }
    if authenticated
        && let Some(identity) = db.identity().await.map_err(local_error)?
        && identity.author_principal_id.is_none()
        && identity.session_backend.is_some()
    {
        let remote = client.identity().await.map_err(client_error)?;
        db.save_identity(
            IdentityRecord {
                user_id: remote.user_id,
                namespace_id: remote.namespace_id,
                username: remote.username,
                session_id: identity.session_id,
                session_backend: identity.session_backend,
                author_principal_id: Some(remote.author_principal_id),
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    }
    let recorded = db.harness_config().await.map_err(local_error)?;
    let roots = resolve_harness_roots(&paths, recorded.as_ref()).map_err(local_error)?;
    prepare_harness_roots(&roots).map_err(local_error)?;
    Ok(InstalledContext {
        paths,
        db,
        roots,
        client,
        limits: capabilities.limits,
    })
}

fn load_credential(
    paths: &LocalPaths,
    installation: &InstallationRecord,
) -> Result<InstallCredential, RuntimeError> {
    let backend =
        CredentialBackend::from_str(&installation.credential_backend).map_err(|error| {
            RuntimeError::new(CliErrorCode::CredentialUnavailable, error.to_string())
        })?;
    CredentialManager::load(paths, backend)
        .map_err(|error| RuntimeError::new(CliErrorCode::CredentialUnavailable, error.to_string()))
}

async fn load_active_bearer(
    paths: &LocalPaths,
    db: &LocalDatabase,
    installation: &InstallationRecord,
) -> Result<String, RuntimeError> {
    if let Some(identity) = db.identity().await.map_err(local_error)? {
        let backend_name = identity.session_backend.as_deref().ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::CredentialUnavailable,
                format!("{} is not logged in on this device", identity.username),
            )
            .recovery(format!("denju login {}", identity.username))
        })?;
        let backend = CredentialBackend::from_str(backend_name).map_err(|error| {
            RuntimeError::new(CliErrorCode::CredentialUnavailable, error.to_string())
        })?;
        let session: SessionCredential =
            CredentialManager::load_session(paths, backend).map_err(|error| {
                RuntimeError::new(CliErrorCode::CredentialUnavailable, error.to_string())
                    .recovery("denju login <@username>")
            })?;
        Ok(session.bearer_token())
    } else {
        Ok(load_credential(paths, installation)?.bearer_token())
    }
}

pub(crate) fn client_error(error: ClientError) -> RuntimeError {
    match &error {
        ClientError::Registry(api) if api.code == ApiErrorCode::NotFound => {
            RuntimeError::new(CliErrorCode::NotFound, api.message.clone())
        }
        ClientError::ContentMismatch(_) => {
            RuntimeError::new(CliErrorCode::ContentVerification, error.to_string())
                .recovery("denju sync")
        }
        ClientError::Registry(api) if api.code == ApiErrorCode::QuotaExceeded => {
            RuntimeError::new(CliErrorCode::QuotaExceeded, api.message.clone())
        }
        ClientError::Registry(api)
            if matches!(
                api.code,
                ApiErrorCode::InvalidRequest
                    | ApiErrorCode::InvalidRequestHash
                    | ApiErrorCode::OperationConflict
                    | ApiErrorCode::GenerationConflict
            ) =>
        {
            RuntimeError::new(CliErrorCode::InvalidArguments, api.message.clone())
        }
        ClientError::Registry(api) if api.code == ApiErrorCode::Unauthorized => {
            RuntimeError::new(CliErrorCode::CredentialUnavailable, api.message.clone())
        }
        _ => RuntimeError::new(CliErrorCode::RegistryUnavailable, error.to_string())
            .recovery("denju doctor"),
    }
}

pub(crate) fn local_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::LocalState, error.to_string()).recovery("denju doctor")
}

pub(crate) fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}
