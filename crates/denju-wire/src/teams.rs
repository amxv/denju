use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    Owner,
    Maintainer,
    Member,
}

impl TeamRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Maintainer => "maintainer",
            Self::Member => "member",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamCreateRequest {
    pub operation_id: String,
    pub name: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamInviteRequest {
    pub operation_id: String,
    pub team: String,
    pub role: TeamRole,
    /// SHA-256 of the client-generated bearer invite code. The registry never receives the
    /// raw invite secret on invite creation, so retries do not require persisting it.
    pub invite_code_hash: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamInviteResponse {
    pub invite_id: String,
    pub team: String,
    pub role: TeamRole,
    pub expires_at_unix_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamInviteRevokeRequest {
    pub operation_id: String,
    pub team: String,
    pub invite_id: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamJoinRequest {
    pub operation_id: String,
    pub code: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMemberRoleRequest {
    pub operation_id: String,
    pub team: String,
    pub member: String,
    pub role: TeamRole,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMemberRemoveRequest {
    pub operation_id: String,
    pub team: String,
    pub member: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSettingsRequest {
    pub operation_id: String,
    pub team: String,
    pub members_can_publish: bool,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMember {
    pub user_id: String,
    pub username: String,
    pub role: TeamRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSummary {
    pub namespace_id: String,
    pub team: String,
    pub role: TeamRole,
    pub members_can_publish: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamDetail {
    #[serde(flatten)]
    pub team: TeamSummary,
    pub members: Vec<TeamMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamList {
    pub teams: Vec<TeamSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMutationResponse {
    #[serde(flatten)]
    pub team: TeamSummary,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceTransferRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub destination_team: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceTransferResponse {
    pub resource_id: String,
    pub old_locator: String,
    pub new_locator: String,
    pub generation: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_roles_have_exact_stable_wire_vocabulary() {
        assert_eq!(
            serde_json::to_string(&TeamRole::Owner).unwrap(),
            "\"owner\""
        );
        assert_eq!(
            serde_json::to_string(&TeamRole::Maintainer).unwrap(),
            "\"maintainer\""
        );
        assert_eq!(
            serde_json::to_string(&TeamRole::Member).unwrap(),
            "\"member\""
        );
    }
}
