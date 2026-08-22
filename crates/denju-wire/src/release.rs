use serde::{Deserialize, Serialize};

use crate::{
    PrivateRevisionResponse, PublicSkill, PublicSkillManifest, SnapshotDownload, SubscribedSkill,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishSkillRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRelease {
    pub version: u64,
    pub revision_id: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishSkillResponse {
    pub skill: PublicSkill,
    pub release: SkillRelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRevisionSummary {
    pub revision_id: String,
    pub parent_revision_ids: Vec<String>,
    pub released_versions: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillHistoryResponse {
    pub resource_id: String,
    pub locator: String,
    pub generation: u64,
    pub workspace_revision_id: String,
    pub revisions: Vec<SkillRevisionSummary>,
    pub releases: Vec<SkillRelease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRevisionDetail {
    pub resource_id: String,
    pub locator: String,
    pub revision_id: String,
    pub parent_revision_ids: Vec<String>,
    pub manifest: PublicSkillManifest,
    pub snapshot: SnapshotDownload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreSkillRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub target_revision_id: String,
    pub request_hash: String,
}

pub type RestoreSkillResponse = PrivateRevisionResponse;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncKnownResource {
    pub resource_id: String,
    pub generation: u64,
    pub revision_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReconcileRequest {
    #[serde(default)]
    pub known: Vec<SyncKnownResource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReconcileResponse {
    pub skills: Vec<SubscribedSkill>,
    pub removed_resource_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyResource {
    pub resource_id: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncHint {
    Dirty { resources: Vec<DirtyResource> },
    ResyncAll,
}
