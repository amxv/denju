use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackUnavailableReason {
    Deleted,
    Unpublished,
    AccessRevoked,
    Quarantined,
}

impl PackUnavailableReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::Unpublished => "unpublished",
            Self::AccessRevoked => "access_revoked",
            Self::Quarantined => "quarantined",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackMemberTarget {
    pub resource_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_version: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackCreateRequest {
    pub operation_id: String,
    pub owner: String,
    pub name: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackMutationRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub members: Vec<PackMemberTarget>,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackPublishRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    #[serde(default)]
    pub public: bool,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackLifecycleRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackRenameRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub new_name: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSubscriptionRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSummary {
    pub resource_id: String,
    pub locator: String,
    pub generation: u64,
    pub version: u64,
    pub visibility: String,
    pub member_count: u64,
    pub degraded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackCreateResponse {
    #[serde(flatten)]
    pub pack: PackSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackMutationResponse {
    #[serde(flatten)]
    pub pack: PackSummary,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackLifecycleResponse {
    #[serde(flatten)]
    pub pack: PackSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_locator: Option<String>,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackMember {
    pub resource_id: String,
    pub locator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_release_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_release_version: Option<u64>,
    pub revision_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<PackUnavailableReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desired: Option<crate::SubscribedSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackDetail {
    #[serde(flatten)]
    pub pack: PackSummary,
    pub members: Vec<PackMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSubscriptionResponse {
    pub resource_id: String,
    pub locator: String,
    pub subscribed: bool,
    pub generation: u64,
    pub version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackRequirementKind {
    Direct,
    TeamAssignment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackRequirementSource {
    pub source_id: String,
    pub kind: PackRequirementKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_namespace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackRequirement {
    pub source: PackRequirementSource,
    pub pack: PackDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSubscriptionCatalog {
    pub packs: Vec<PackRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackDrainResponse {
    pub processed_pack_revisions: u64,
    pub completed_release_events: u64,
    pub pending_release_event_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackDrainRequest {
    #[serde(default = "default_drain_limit")]
    pub limit: u32,
}

fn default_drain_limit() -> u32 {
    64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_reason_has_stable_wire_vocabulary() {
        assert_eq!(
            serde_json::to_string(&PackUnavailableReason::AccessRevoked).unwrap(),
            "\"access_revoked\""
        );
        assert_eq!(
            serde_json::from_str::<PackUnavailableReason>("\"quarantined\"").unwrap(),
            PackUnavailableReason::Quarantined
        );
    }
}
