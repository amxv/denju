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
const KEYRING_USER: &str = "installation";
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
            match store_native(credential) {
                Ok(()) => return Ok(CredentialBackend::OsNative),
                Err(error) if file_fallback_allowed() => {
                    let _ = error;
                }
                Err(error) => return Err(error),
            }
        }
        store_file(paths, credential)?;
        Ok(CredentialBackend::File)
    }

    pub fn load(
        paths: &LocalPaths,
        backend: CredentialBackend,
    ) -> Result<InstallCredential, CredentialError> {
        let _guard = credential_lock()?;
        match backend {
            CredentialBackend::OsNative => load_native(),
            CredentialBackend::File => load_file(paths),
        }
    }

    pub fn verify_file_permissions(paths: &LocalPaths) -> Result<(), CredentialError> {
        let path = credential_file(paths);
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
}

fn credential_lock() -> Result<std::sync::MutexGuard<'static, ()>, CredentialError> {
    CREDENTIAL_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| CredentialError::LockPoisoned)
}

fn store_native(credential: &InstallCredential) -> Result<(), CredentialError> {
    configure_native_store()?;
    Entry::new(KEYRING_SERVICE, KEYRING_USER)?.set_secret(credential.as_bytes())?;
    Ok(())
}

fn load_native() -> Result<InstallCredential, CredentialError> {
    configure_native_store()?;
    let bytes = Entry::new(KEYRING_SERVICE, KEYRING_USER)?.get_secret()?;
    Ok(InstallCredential(
        bytes
            .try_into()
            .map_err(|_| CredentialError::InvalidCredentialLength)?,
    ))
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

fn store_file(paths: &LocalPaths, credential: &InstallCredential) -> Result<(), CredentialError> {
    fs::create_dir_all(&paths.credentials)?;
    let path = credential_file(paths);
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
    file.write_all(credential.to_hex().as_bytes())?;
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

fn credential_file(paths: &LocalPaths) -> PathBuf {
    paths.credentials.join("install-token")
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
}
