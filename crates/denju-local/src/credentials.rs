use std::{
    fs,
    fs::OpenOptions,
    io::{self, Write},
    path::PathBuf,
    str::FromStr,
    sync::{Mutex, OnceLock},
};

use keyring_core::Entry;
use rand::random;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::LocalPaths;

const KEYRING_SERVICE: &str = "denju";
const KEYRING_INSTALL_USER: &str = "installation";
const KEYRING_SESSION_USER: &str = "session";
static CREDENTIAL_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, PartialEq, Eq)]
pub struct InstallCredential([u8; 32]);

impl InstallCredential {
    pub fn generate() -> Self {
        Self(random())
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn sha256_hex(&self) -> String {
        hex::encode(Sha256::digest(self.0))
    }

    /// Opaque bearer representation used only at the HTTPS client edge. The credential
    /// type intentionally has no Debug implementation so this value never appears through
    /// routine structured logging.
    pub fn bearer_token(&self) -> String {
        hex::encode(self.0)
    }

    fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SessionCredential([u8; 32]);

impl SessionCredential {
    pub fn generate() -> Self {
        Self(random())
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn sha256_hex(&self) -> String {
        hex::encode(Sha256::digest(self.0))
    }

    pub fn bearer_token(&self) -> String {
        hex::encode(self.0)
    }

    fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl FromStr for SessionCredential {
    type Err = CredentialError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(value.trim()).map_err(CredentialError::InvalidCredential)?;
        Ok(Self(
            bytes
                .try_into()
                .map_err(|_| CredentialError::InvalidCredentialLength)?,
        ))
    }
}

impl FromStr for InstallCredential {
    type Err = CredentialError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(value.trim()).map_err(CredentialError::InvalidCredential)?;
        Ok(Self(
            bytes
                .try_into()
                .map_err(|_| CredentialError::InvalidCredentialLength)?,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialBackend {
    OsNative,
    File,
}

impl CredentialBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OsNative => "os_native",
            Self::File => "file",
        }
    }
}

impl FromStr for CredentialBackend {
    type Err = CredentialError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "os_native" => Ok(Self::OsNative),
            "file" => Ok(Self::File),
            other => Err(CredentialError::UnknownBackend(other.to_owned())),
        }
    }
}

pub struct CredentialManager;

impl CredentialManager {
    pub fn store(
        paths: &LocalPaths,
        credential: &InstallCredential,
        force_file: bool,
    ) -> Result<CredentialBackend, CredentialError> {
        let _guard = credential_lock()?;
        if !force_file {
            match store_native(KEYRING_INSTALL_USER, credential.as_bytes()) {
                Ok(()) => return Ok(CredentialBackend::OsNative),
                Err(error) if file_fallback_allowed() => {
                    let _ = error;
                }
                Err(error) => return Err(error),
            }
        }
        store_secret_file(credential_file(paths), &credential.to_hex())?;
        Ok(CredentialBackend::File)
    }

    pub fn load(
        paths: &LocalPaths,
        backend: CredentialBackend,
    ) -> Result<InstallCredential, CredentialError> {
        let _guard = credential_lock()?;
        match backend {
            CredentialBackend::OsNative => {
                Ok(InstallCredential(load_native(KEYRING_INSTALL_USER)?))
            }
            CredentialBackend::File => load_file(paths),
        }
    }

    pub fn store_session(
        paths: &LocalPaths,
        credential: &SessionCredential,
        force_file: bool,
    ) -> Result<CredentialBackend, CredentialError> {
        let _guard = credential_lock()?;
        if !force_file {
            match store_native(KEYRING_SESSION_USER, credential.as_bytes()) {
                Ok(()) => return Ok(CredentialBackend::OsNative),
                Err(error) if file_fallback_allowed() => {
                    let _ = error;
                }
                Err(error) => return Err(error),
            }
        }
        store_secret_file(session_credential_file(paths), &credential.to_hex())?;
        Ok(CredentialBackend::File)
    }

    pub fn load_session(
        paths: &LocalPaths,
        backend: CredentialBackend,
    ) -> Result<SessionCredential, CredentialError> {
        let _guard = credential_lock()?;
        match backend {
            CredentialBackend::OsNative => {
                Ok(SessionCredential(load_native(KEYRING_SESSION_USER)?))
            }
            CredentialBackend::File => {
                verify_owner_only_file(&session_credential_file(paths))?;
                fs::read_to_string(session_credential_file(paths))?.parse()
            }
        }
    }

    pub fn delete_session(
        paths: &LocalPaths,
        backend: CredentialBackend,
    ) -> Result<(), CredentialError> {
        let _guard = credential_lock()?;
        match backend {
            CredentialBackend::OsNative => {
                configure_native_store()?;
                match Entry::new(KEYRING_SERVICE, KEYRING_SESSION_USER)?.delete_credential() {
                    Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                    Err(error) => Err(error.into()),
                }
            }
            CredentialBackend::File => match fs::remove_file(session_credential_file(paths)) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            },
        }
    }

    pub fn delete_installation(
        paths: &LocalPaths,
        backend: CredentialBackend,
    ) -> Result<(), CredentialError> {
        let _guard = credential_lock()?;
        match backend {
            CredentialBackend::OsNative => {
                configure_native_store()?;
                match Entry::new(KEYRING_SERVICE, KEYRING_INSTALL_USER)?.delete_credential() {
                    Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                    Err(error) => Err(error.into()),
                }
            }
            CredentialBackend::File => match fs::remove_file(credential_file(paths)) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            },
        }
    }

    pub fn verify_file_permissions(paths: &LocalPaths) -> Result<(), CredentialError> {
        verify_owner_only_file(&credential_file(paths))
    }

    pub fn verify_session_file_permissions(paths: &LocalPaths) -> Result<(), CredentialError> {
        verify_owner_only_file(&session_credential_file(paths))
    }
}

fn credential_lock() -> Result<std::sync::MutexGuard<'static, ()>, CredentialError> {
    CREDENTIAL_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| CredentialError::LockPoisoned)
}

fn store_native(user: &str, secret: &[u8]) -> Result<(), CredentialError> {
    configure_native_store()?;
    Entry::new(KEYRING_SERVICE, user)?.set_secret(secret)?;
    Ok(())
}

fn load_native(user: &str) -> Result<[u8; 32], CredentialError> {
    configure_native_store()?;
    let bytes = Entry::new(KEYRING_SERVICE, user)?.get_secret()?;
    bytes
        .try_into()
        .map_err(|_| CredentialError::InvalidCredentialLength)
}

fn configure_native_store() -> Result<(), CredentialError> {
    #[cfg(target_os = "macos")]
    let store = apple_native_keyring_store::keychain::Store::new()?;
    #[cfg(target_os = "windows")]
    let store = windows_native_keyring_store::Store::new()?;
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    let store = zbus_secret_service_keyring_store::Store::new()?;
    #[cfg(not(any(unix, windows)))]
    return Err(CredentialError::NativeUnavailable);
    #[cfg(any(unix, windows))]
    keyring_core::set_default_store(store);
    Ok(())
}

fn store_secret_file(path: PathBuf, value: &str) -> Result<(), CredentialError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)?
    };
    #[cfg(not(unix))]
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)?;
    file.write_all(value.as_bytes())?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn load_file(paths: &LocalPaths) -> Result<InstallCredential, CredentialError> {
    CredentialManager::verify_file_permissions(paths)?;
    fs::read_to_string(credential_file(paths))?.parse()
}

fn verify_owner_only_file(path: &PathBuf) -> Result<(), CredentialError> {
    if !path.exists() {
        return Err(CredentialError::MissingCredential);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(CredentialError::InsecurePermissions(mode & 0o777));
        }
    }
    Ok(())
}

fn credential_file(paths: &LocalPaths) -> PathBuf {
    paths.credentials.join("install-token")
}

fn session_credential_file(paths: &LocalPaths) -> PathBuf {
    paths.credentials.join("session-token")
}

const fn file_fallback_allowed() -> bool {
    cfg!(all(
        unix,
        not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "android"
        ))
    ))
}

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("credential store error: {0}")]
    Keyring(#[from] keyring_core::Error),
    #[error("credential file error: {0}")]
    Io(#[from] io::Error),
    #[error("install credential has invalid hex: {0}")]
    InvalidCredential(hex::FromHexError),
    #[error("install credential must contain 32 bytes")]
    InvalidCredentialLength,
    #[error("installation credential is missing")]
    MissingCredential,
    #[error("credential file permissions are too broad: {0:o}")]
    InsecurePermissions(u32),
    #[error("unknown credential backend {0}")]
    UnknownBackend(String),
    #[error("native credential storage is unavailable on this platform")]
    NativeUnavailable,
    #[error("credential access lock was poisoned")]
    LockPoisoned,
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn file_backend_round_trips_without_sqlite() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        let credential = InstallCredential::generate();
        let backend = CredentialManager::store(&paths, &credential, true).unwrap();
        assert_eq!(backend, CredentialBackend::File);
        assert!(CredentialManager::load(&paths, backend).unwrap() == credential);
        CredentialManager::verify_file_permissions(&paths).unwrap();
    }

    #[test]
    fn session_file_is_separate_owner_only_and_revocable() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        let installation = InstallCredential::generate();
        let session = SessionCredential::generate();
        CredentialManager::store(&paths, &installation, true).unwrap();
        let backend = CredentialManager::store_session(&paths, &session, true).unwrap();
        assert_eq!(backend, CredentialBackend::File);
        assert!(CredentialManager::load_session(&paths, backend).unwrap() == session);
        CredentialManager::verify_session_file_permissions(&paths).unwrap();
        assert!(credential_file(&paths).is_file());
        assert!(session_credential_file(&paths).is_file());
        CredentialManager::delete_session(&paths, backend).unwrap();
        assert!(!session_credential_file(&paths).exists());
        assert!(credential_file(&paths).is_file());
    }
}
