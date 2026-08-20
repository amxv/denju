//! Local SQLite, filesystem, watcher, projection, and service boundaries.

mod credentials;
mod db;
mod harness;
mod paths;
mod service;

pub use credentials::{CredentialBackend, CredentialError, CredentialManager, InstallCredential};
pub use db::{
    BootstrapJournal, BootstrapJournalPayload, HarnessConfig, InstallationRecord, JournalState,
    LocalDatabase, LocalDbError, ServiceRecord,
};
pub use harness::{
    HarnessEnvironment, HarnessError, ResolvedHarnessRoots, detect_unmanaged_skills,
    prepare_harness_roots, remove_old_codex_projection, resolve_harness_roots,
    resolve_harness_roots_for,
};
pub use paths::{LocalPathError, LocalPaths, ensure_local_layout, verify_native_directory_links};
pub use service::{ServiceError, ServiceInstallMode, ServiceKind, ServiceManager, ServiceStatus};
