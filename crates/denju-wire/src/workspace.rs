use serde::{Deserialize, Serialize};

use crate::{PublicSkillManifest, StagedBlobUpload};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateRevisionRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub expected_head_revision_id: String,
    pub parent_revision_ids: Vec<String>,
    pub manifest: PublicSkillManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_author_principal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_sync: Option<ForkSyncIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub historical_skill_name: Option<String>,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkSyncIntent {
    pub expected_sync_base_revision_id: String,
    pub upstream_revision_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateRevisionOperationState {
    Prepared,
    Advanced,
    Diverged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateRevisionPrepareResponse {
    pub resource_id: String,
    pub revision_id: String,
    pub expected_generation: u64,
    pub state: PrivateRevisionOperationState,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateWorkspaceConflict {
    pub conflict_id: String,
    pub resource_id: String,
    pub base_revision_id: String,
    pub head_revision_ids: Vec<String>,
    pub active_revision_id: String,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_revision_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PrivateRevisionCommitResponse {
    Advanced {
        revision: PrivateRevisionResponse,
    },
    Diverged {
        resource_id: String,
        revision_id: String,
        conflict: PrivateWorkspaceConflict,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divergent_commit_wire_shape_is_explicitly_tagged() {
        let response = PrivateRevisionCommitResponse::Diverged {
            resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".into(),
            revision_id: "11".repeat(32),
            conflict: PrivateWorkspaceConflict {
                conflict_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f003".into(),
                resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".into(),
                base_revision_id: "22".repeat(32),
                head_revision_ids: vec!["33".repeat(32), "44".repeat(32)],
                active_revision_id: "44".repeat(32),
                generation: 7,
                resolution_revision_id: None,
            },
        };
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["state"], "diverged");
        assert_eq!(value["conflict"]["generation"], 7);
        assert_eq!(
            value["conflict"]["head_revision_ids"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}
