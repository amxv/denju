use serde::{Deserialize, Serialize};

use crate::{PublicSkillManifest, StagedBlobUpload};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateRevisionRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub expected_parent_revision_id: String,
    pub manifest: PublicSkillManifest,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateRevisionPrepareResponse {
    pub resource_id: String,
    pub revision_id: String,
    pub generation: u64,
    pub committed: bool,
    pub uploads: Vec<StagedBlobUpload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateRevisionCommitRequest {
    pub operation_id: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateRevisionResponse {
    pub resource_id: String,
    pub generation: u64,
    pub revision_id: String,
    pub description: String,
    pub manifest: PublicSkillManifest,
}
