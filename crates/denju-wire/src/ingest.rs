use serde::{Deserialize, Serialize};

use crate::{PrivateWorkspaceConflict, PublicSkillManifest, SnapshotDownload};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateSkillImportRequest {
    pub operation_id: String,
    pub expected_generation: u64,
    pub name: String,
    pub manifest: PublicSkillManifest,
    pub snapshot_sha256: String,
    pub snapshot_size_bytes: u64,
    pub request_hash: String,
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
    pub generation: u64,
    pub revision_id: String,
    pub manifest: PublicSkillManifest,
    pub snapshot: SnapshotDownload,
    pub conflicts: Vec<PrivateWorkspaceConflict>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateSkillCatalog {
    pub skills: Vec<PrivateSkill>,
}
