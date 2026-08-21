use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameSkillRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub new_name: String,
    #[serde(default)]
    pub prepared_revision_operation_id: Option<String>,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameSkillResponse {
    pub resource_id: String,
    pub old_locator: String,
    pub locator: String,
    pub generation: u64,
    pub revision_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_version: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLifecycleRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnpublishSkillResponse {
    pub resource_id: String,
    pub locator: String,
    pub generation: u64,
    pub unpublished: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteSkillResponse {
    pub resource_id: String,
    pub locator: String,
    pub generation: u64,
    pub deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_release_version: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeprecateSkillRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub deprecated: bool,
    #[serde(default)]
    pub replacement_resource_id: Option<String>,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeprecateSkillResponse {
    pub resource_id: String,
    pub locator: String,
    pub generation: u64,
    pub deprecated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageResponse {
    pub storage_limit_bytes: u64,
    pub storage_used_bytes: u64,
    pub storage_available_bytes: u64,
    pub active_resources: u64,
    pub private_revisions: u64,
    pub prunable_private_revisions: u64,
    pub prunable_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryPruneResponse {
    pub resource_id: String,
    pub locator: String,
    pub generation: u64,
    pub pruned_revisions: u64,
    pub reclaimed_bytes: u64,
    pub gc_candidates: u64,
}
