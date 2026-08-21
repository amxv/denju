//! Local SQLite, filesystem, watcher, projection, and service boundaries.

mod credentials;
mod db;
mod desired_db;
mod fork_db;
mod harness;
mod lifecycle;
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
    AccountDeleteJournal, AccountDeleteJournalPayload, BootstrapJournal, BootstrapJournalPayload,
    HarnessConfig, IdentityRecord, ImportJournal, ImportJournalPayload, InstallationRecord,
    JournalState, LocalDatabase, LocalDbError, LocalForkRecord, ManagedSkillRecord,
    MaterializationJournal, MaterializationJournalPayload, OwnedSkillRecord, ServiceRecord,
    SubscriptionRecord,
};
pub use harness::{
    HarnessEnvironment, HarnessError, ResolvedHarnessRoots, detect_unmanaged_skills,
    prepare_harness_roots, remove_old_codex_projection, resolve_harness_roots,
    resolve_harness_roots_for,
};
pub use lifecycle::{
    LocalLifecycleError, ManagedDesiredKind, RegistryRenameState, apply_registry_rename,
    journaled_remove_managed_skill, recover_local_lifecycle,
};
pub use materialize::{
    DesiredSkillMaterialization, MaterializationError, export_skill_snapshot,
    materialize_skill_snapshot, reconcile_canonical_links, recover_materializations,
    remove_canonical_skill,
};
pub use paths::{
    LocalPathError, LocalPaths, TEST_HOME_ENV, TEST_HOME_MARKER, create_native_directory_link,
    ensure_local_layout, verify_native_directory_links,
};
pub use projection::{
    ProjectionError, reconcile_harness_projections, reconcile_owned_derived_projection,
    recover_workspace_writebacks, remove_managed_skill_projection, remove_subscription_projection,
};
pub use service::{ServiceError, ServiceInstallMode, ServiceKind, ServiceManager, ServiceStatus};
pub use source::{SourceError, read_skill_source};
pub use workspace::{
    WorkspaceScan, WorkspaceScanError, WorkspaceScanStats, scan_owned_workspace,
    store_workspace_entries, workspace_blob_path, workspace_entries_from_manifest,
};
pub use workspace_db::{
    DerivedProjectionStateRecord, LocalRevisionRecord, WorkspaceContentConflictRecord,
    WorkspaceFileRecord, WorkspaceStateRecord, WorkspaceStatus, WorkspaceWritebackJournal,
    WorkspaceWritebackJournalPayload,
};
pub use workspace_watch::{WorkspaceWatchError, WorkspaceWatcher};
