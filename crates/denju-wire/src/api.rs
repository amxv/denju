use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryCapabilities {
    pub api_version: String,
    pub registry_origin: String,
    pub object_store_required: bool,
    pub limits: RegistryLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryLimits {
    pub max_object_bytes: u64,
    pub max_release_bytes: u64,
    pub namespace_storage_bytes: u64,
    pub max_transfer_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInstallationRequest {
    pub operation_id: String,
    pub credential_hash: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInstallationResponse {
    pub installation_id: String,
    pub author_principal_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ApiErrorCode,
    pub message: String,
}

impl ApiError {
    pub fn new(code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorCode {
    InvalidRequest,
    InvalidRequestHash,
    OperationConflict,
    GenerationConflict,
    Unauthorized,
    NotFound,
    Internal,
    Unavailable,
}
