use denju_core::OperationId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub struct InstallationRecord {
    pub registry_origin: String,
    pub installation_id: String,
    pub author_principal_id: String,
    pub credential_backend: String,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityRecord {
    pub user_id: String,
    pub namespace_id: String,
    pub username: String,
    pub session_id: Option<String>,
    pub session_backend: Option<String>,
    pub author_principal_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessConfig {
    pub codex_root: String,
    pub claude_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRecord {
    pub kind: String,
    pub persistent: bool,
    pub running: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionRecord {
    pub resource_id: String,
    pub locator: String,
    pub owner: String,
    pub skill_name: String,
    pub resource_generation: i64,
    pub release_version: i64,
    pub desired_revision_id: String,
    pub harness_name: Option<String>,
    pub materialized_revision_id: Option<String>,
    pub retain_on_delete: bool,
    pub retained_after_delete: bool,
    pub live_private: bool,
    pub desired_root_tree_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalForkRecord {
    pub resource_id: String,
    pub upstream_resource_id: String,
    pub upstream_locator: String,
    pub created_from_revision_id: String,
    pub sync_base_revision_id: String,
    pub desired_name: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedSkillRecord {
    pub resource_id: String,
    pub locator: String,
    pub owner: String,
    pub skill_name: String,
    pub resource_generation: i64,
    pub desired_revision_id: String,
    pub harness_name: Option<String>,
    pub materialized_revision_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSkillRecord {
    pub resource_id: String,
    pub locator: String,
    pub owner: String,
    pub skill_name: String,
    pub harness_name: Option<String>,
    pub materialized_revision_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportJournal {
    pub operation_id: OperationId,
    pub state: JournalState,
    pub payload: ImportJournalPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportJournalPayload {
    pub source_path: String,
    pub skill_name: String,
    pub request_hash: String,
    pub manifest_json: String,
    pub snapshot_sha256: String,
    pub snapshot_size_bytes: u64,
    pub snapshot_path: String,
    pub resource_id: Option<String>,
    pub locator: Option<String>,
    pub revision_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializationJournal {
    pub operation_id: OperationId,
    pub state: JournalState,
    pub payload: MaterializationJournalPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterializationJournalPayload {
    pub resource_id: String,
    pub revision_id: String,
    pub stage_dir: String,
    pub generation_dir: String,
    pub canonical_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapJournal {
    pub operation_id: OperationId,
    pub state: JournalState,
    pub payload: BootstrapJournalPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapJournalPayload {
    pub registry_origin: String,
    pub credential_hash: String,
    pub credential_backend: Option<String>,
    pub installation_id: Option<String>,
    pub author_principal_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountDeleteJournal {
    pub operation_id: OperationId,
    pub state: JournalState,
    pub payload: AccountDeleteJournalPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDeleteJournalPayload {
    pub username: String,
    pub session_backend: String,
    pub installation_backend: String,
    pub removed_local_skills: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalState {
    Planned,
    Staged,
    Verified,
    Switched,
    Complete,
}

impl JournalState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Staged => "staged",
            Self::Verified => "verified",
            Self::Switched => "switched",
            Self::Complete => "complete",
        }
    }

    pub const fn next(self) -> Option<Self> {
        match self {
            Self::Planned => Some(Self::Staged),
            Self::Staged => Some(Self::Verified),
            Self::Verified => Some(Self::Switched),
            Self::Switched => Some(Self::Complete),
            Self::Complete => None,
        }
    }
}

impl std::str::FromStr for JournalState {
    type Err = LocalDbError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "planned" => Ok(Self::Planned),
            "staged" => Ok(Self::Staged),
            "verified" => Ok(Self::Verified),
            "switched" => Ok(Self::Switched),
            "complete" => Ok(Self::Complete),
            other => Err(LocalDbError::Corrupt(format!(
                "unknown journal state {other}"
            ))),
        }
    }
}

#[derive(Debug, Error)]
pub enum LocalDbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("local state serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("failed to start SQLite worker: {0}")]
    WorkerStart(std::io::Error),
    #[error("SQLite worker stopped unexpectedly")]
    WorkerStopped,
    #[error("local database schema {0} is newer than this Denju binary")]
    UnsupportedSchema(i64),
    #[error("corrupt local state: {0}")]
    Corrupt(String),
    #[error("invalid journal transition {expected:?} -> {next:?}")]
    InvalidJournalTransition {
        expected: JournalState,
        next: JournalState,
    },
    #[error("lease TTL must be positive, got {0}ms")]
    InvalidLeaseTtl(i64),
}
