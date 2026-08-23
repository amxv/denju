use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminOperatorCredential {
    pub operator_id: String,
    pub name: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminOperatorRevokeResponse {
    pub operator_id: String,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminResourceTarget {
    pub resource_id: String,
    pub locator: String,
    pub kind: String,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_release_version: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminReport {
    pub report_id: String,
    pub resource_id: String,
    pub locator: String,
    pub resource_generation: u64,
    pub reason: String,
    pub created_at_unix_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminReportList {
    pub reports: Vec<AdminReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminQuarantineRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_version: Option<u64>,
    #[serde(default)]
    pub reason: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminQuarantineResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine_id: Option<String>,
    pub resource_id: String,
    pub locator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_version: Option<u64>,
    pub quarantined: bool,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantinedResource {
    pub resource_id: String,
    pub locator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
    pub reason: String,
}
