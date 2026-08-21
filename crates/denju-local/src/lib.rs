//! Local SQLite, filesystem, watcher, projection, and service boundaries.

mod credentials;
mod db;
mod harness;
mod materialize;
mod paths;
mod projection;
mod service;
mod source;
mod workspace;
mod workspace_db;
mod workspace_watch;

pub use credentials::{
    CredentialBackend, CredentialError, CredentialManager, InstallCredential, SessionCredential,
};
pub use db::{
    BootstrapJournal, BootstrapJournalPayload, HarnessConfig, IdentityRecord, ImportJournal,
    ImportJournalPayload, InstallationRecord, JournalState, LocalDatabase, LocalDbError,
    ManagedSkillRecord, MaterializationJournal, MaterializationJournalPayload, OwnedSkillRecord,
    ServiceRecord, SubscriptionRecord,
};
pub use harness::{
    HarnessEnvironment, HarnessError, ResolvedHarnessRoots, detect_unmanaged_skills,
    prepare_harness_roots, remove_old_codex_projection, resolve_harness_roots,
    resolve_harness_roots_for,
};
pub use materialize::{
    DesiredSkillMaterialization, MaterializationError, export_skill_snapshot,
    materialize_skill_snapshot, recover_materializations, remove_canonical_skill,
};
pub use paths::{
    LocalPathError, LocalPaths, create_native_directory_link, ensure_local_layout,
    verify_native_directory_links,
};
pub use projection::{
    ProjectionError, reconcile_harness_projections, reconcile_owned_derived_projection,
    recover_workspace_writebacks, remove_managed_skill_projection, remove_subscription_projection,
};
pub use service::{ServiceError, ServiceInstallMode, ServiceKind, ServiceManager, ServiceStatus};
pub use source::{SourceError, read_skill_source};
pub use workspace::{
    WorkspaceScan, WorkspaceScanError, WorkspaceScanStats, scan_owned_workspace,
    workspace_blob_path,
};
pub use workspace_db::{
    DerivedProjectionStateRecord, LocalRevisionRecord, WorkspaceFileRecord, WorkspaceStateRecord,
    WorkspaceStatus, WorkspaceWritebackJournal, WorkspaceWritebackJournalPayload,
};
pub use workspace_watch::{WorkspaceWatchError, WorkspaceWatcher};
