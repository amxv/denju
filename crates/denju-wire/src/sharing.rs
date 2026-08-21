use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareMutationKind {
    Share,
    Unshare,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareSkillRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub recipient: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareSkillResponse {
    pub resource_id: String,
    pub locator: String,
    pub recipient: String,
    pub shared: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribe_command: Option<String>,
}
