use serde::{Deserialize, Serialize};

use crate::{PublicSkillManifest, SnapshotDownload};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillProposalState {
    Open,
    NeedsSync,
    Accepted,
    Rejected,
    Withdrawn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillProposal {
    pub proposal_id: String,
    pub generation: u64,
    pub state: SkillProposalState,
    pub proposer: String,
    pub source_resource_id: String,
    pub source_locator: String,
    pub source_generation: u64,
    pub target_resource_id: String,
    pub target_locator: String,
    pub target_generation: u64,
    pub proposed_revision_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillProposalList {
    pub proposals: Vec<SkillProposal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillProposalDetail {
    #[serde(flatten)]
    pub proposal: SkillProposal,
    pub manifest: PublicSkillManifest,
    pub snapshot: SnapshotDownload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalCreateRequest {
    pub operation_id: String,
    pub source_resource_id: String,
    pub expected_source_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalCloseRequest {
    pub operation_id: String,
    pub proposal_id: String,
    pub expected_generation: u64,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalAcceptRequest {
    pub operation_id: String,
    pub proposal_id: String,
    pub expected_generation: u64,
    pub expected_proposed_revision_id: String,
    pub expected_source_generation: u64,
    pub expected_target_generation: u64,
    pub request_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalCloseKind {
    Reject,
    Withdraw,
}
