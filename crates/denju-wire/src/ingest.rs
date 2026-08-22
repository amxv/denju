use serde::{Deserialize, Serialize};

use crate::{PrivateWorkspaceConflict, PublicSkillManifest, SnapshotDownload};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateSkillImportRequest {
    pub operation_id: String,
    pub expected_generation: u64,
    pub owner: String,
    pub name: String,
    pub manifest: PublicSkillManifest,
    pub snapshot_sha256: String,
    pub snapshot_size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_author_principal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<ForkImportIntent>,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkImportIntent {
    pub upstream_resource_id: String,
    pub upstream_revision_id: String,
    #[serde(default)]
    pub replace_subscription: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion_head_revision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub historical_skill_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillForkProvenance {
    pub upstream_resource_id: String,
    pub upstream_locator: String,
    pub created_from_revision_id: String,
    pub sync_base_revision_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateSkillImportCommitRequest {
    pub operation_id: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedBlobUpload {
    pub blob_id: String,
    pub size_bytes: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateSkillImportPrepareResponse {
    pub resource_id: String,
    pub locator: String,
    pub revision_id: String,
    pub generation: u64,
    pub committed: bool,
    pub uploads: Vec<StagedBlobUpload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateSkill {
    pub resource_id: String,
    pub locator: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    /// Stable resource generation for lifecycle/subscriber-visible changes.
    pub generation: u64,
    /// Private CAS generation for this authenticated user's workspace ref.
    pub workspace_generation: u64,
    pub revision_id: String,
    pub manifest: PublicSkillManifest,
    pub snapshot: SnapshotDownload,
    pub conflicts: Vec<PrivateWorkspaceConflict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<SkillForkProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateSkillImportResponse {
    pub resource_id: String,
    pub locator: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub generation: u64,
    pub revision_id: String,
    pub manifest: PublicSkillManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<SkillForkProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateSkillCatalog {
    pub skills: Vec<PrivateSkill>,
}
