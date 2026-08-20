use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimIdentityRequest {
    pub operation_id: String,
    pub username: String,
    pub password: String,
    pub session_token_hash: String,
    pub recovery_secret_hash: String,
    pub device_name: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub operation_id: String,
    pub username: String,
    pub password: String,
    pub session_token_hash: String,
    pub device_name: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryResetRequest {
    pub operation_id: String,
    pub username: String,
    pub recovery_secret: String,
    pub new_password: String,
    pub session_token_hash: String,
    pub replacement_recovery_secret_hash: String,
    pub device_name: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityBackupRequest {
    pub operation_id: String,
    pub password: String,
    pub replacement_recovery_secret_hash: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentitySessionResponse {
    pub user_id: String,
    pub namespace_id: String,
    pub author_principal_id: String,
    pub username: String,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityInfo {
    pub user_id: String,
    pub namespace_id: String,
    pub author_principal_id: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceList {
    pub devices: Vec<DeviceInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub session_id: String,
    pub installation_id: String,
    pub device_name: String,
    pub created_at_unix_ms: i64,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRevokeRequest {
    pub operation_id: String,
    pub session_id: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRevokeResponse {
    pub session_id: String,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationTokenCreateRequest {
    pub operation_id: String,
    pub token_hash: String,
    pub scopes: Vec<String>,
    pub expires_in_seconds: u64,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationTokenCreateResponse {
    pub token_id: String,
    pub scopes: Vec<String>,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationTokenList {
    pub tokens: Vec<AutomationTokenInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationTokenInfo {
    pub token_id: String,
    pub scopes: Vec<String>,
    pub created_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationTokenRevokeRequest {
    pub operation_id: String,
    pub token_id: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationTokenRevokeResponse {
    pub token_id: String,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDeleteRequest {
    pub operation_id: String,
    pub password: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDeleteResponse {
    pub deleted: bool,
    pub username: String,
}
