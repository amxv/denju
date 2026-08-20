use std::{fmt, str::FromStr};

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const CREATE_INSTALLATION_DOMAIN: &[u8] = b"denju:http:v1:create-installation\0";
const CLAIM_IDENTITY_DOMAIN: &[u8] = b"denju:http:v1:claim-identity\0";
const LOGIN_DOMAIN: &[u8] = b"denju:http:v1:login\0";
const RECOVERY_RESET_DOMAIN: &[u8] = b"denju:http:v1:recovery-reset\0";
const IDENTITY_BACKUP_DOMAIN: &[u8] = b"denju:http:v1:identity-backup\0";
const DEVICE_REVOKE_DOMAIN: &[u8] = b"denju:http:v1:device-revoke\0";
const TOKEN_CREATE_DOMAIN: &[u8] = b"denju:http:v1:automation-token-create\0";
const TOKEN_REVOKE_DOMAIN: &[u8] = b"denju:http:v1:automation-token-revoke\0";
const ACCOUNT_DELETE_DOMAIN: &[u8] = b"denju:http:v1:account-delete\0";
const SUBSCRIBE_DOMAIN: &[u8] = b"denju:http:v1:subscribe\0";
const UNSUBSCRIBE_DOMAIN: &[u8] = b"denju:http:v1:unsubscribe\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestHash([u8; 32]);

impl RequestHash {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for RequestHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl FromStr for RequestHash {
    type Err = RequestHashError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(value).map_err(RequestHashError::InvalidHex)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| RequestHashError::InvalidLength)?;
        Ok(Self(bytes))
    }
}

#[derive(Debug, Error)]
pub enum RequestHashError {
    #[error("request hash must be 32 bytes")]
    InvalidLength,
    #[error("invalid request hash hex: {0}")]
    InvalidHex(hex::FromHexError),
    #[error("failed to canonicalize request payload: {0}")]
    Canonicalization(String),
}

#[derive(Serialize)]
struct CreateInstallationHashInput<'a> {
    operation_id: &'a str,
    credential_hash: &'a str,
}

pub fn create_installation_request_hash(
    operation_id: &str,
    credential_hash: &str,
) -> Result<RequestHash, RequestHashError> {
    let payload = CreateInstallationHashInput {
        operation_id,
        credential_hash,
    };
    let canonical = serde_json_canonicalizer::to_vec(&payload)
        .map_err(|error| RequestHashError::Canonicalization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(CREATE_INSTALLATION_DOMAIN);
    hasher.update(canonical);
    Ok(RequestHash::from_bytes(hasher.finalize().into()))
}

pub fn identity_mutation_request_hash<T: Serialize>(
    operation_id: &str,
    domain: IdentityMutationDomain,
    safe_payload: &T,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct HashInput<'a, T> {
        operation_id: &'a str,
        payload: &'a T,
    }
    let canonical = serde_json_canonicalizer::to_vec(&HashInput {
        operation_id,
        payload: safe_payload,
    })
    .map_err(|error| RequestHashError::Canonicalization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(domain.bytes());
    hasher.update(canonical);
    Ok(RequestHash::from_bytes(hasher.finalize().into()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityMutationDomain {
    Claim,
    Login,
    RecoveryReset,
    Backup,
    DeviceRevoke,
    TokenCreate,
    TokenRevoke,
    AccountDelete,
}

impl IdentityMutationDomain {
    const fn bytes(self) -> &'static [u8] {
        match self {
            Self::Claim => CLAIM_IDENTITY_DOMAIN,
            Self::Login => LOGIN_DOMAIN,
            Self::RecoveryReset => RECOVERY_RESET_DOMAIN,
            Self::Backup => IDENTITY_BACKUP_DOMAIN,
            Self::DeviceRevoke => DEVICE_REVOKE_DOMAIN,
            Self::TokenCreate => TOKEN_CREATE_DOMAIN,
            Self::TokenRevoke => TOKEN_REVOKE_DOMAIN,
            Self::AccountDelete => ACCOUNT_DELETE_DOMAIN,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionMutationKind {
    Subscribe,
    Unsubscribe,
}

#[derive(Serialize)]
struct SubscriptionHashInput<'a> {
    operation_id: &'a str,
    resource_id: &'a str,
    expected_generation: u64,
}

pub fn subscription_request_hash(
    kind: SubscriptionMutationKind,
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
) -> Result<RequestHash, RequestHashError> {
    let payload = SubscriptionHashInput {
        operation_id,
        resource_id,
        expected_generation,
    };
    let canonical = serde_json_canonicalizer::to_vec(&payload)
        .map_err(|error| RequestHashError::Canonicalization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(match kind {
        SubscriptionMutationKind::Subscribe => SUBSCRIBE_DOMAIN,
        SubscriptionMutationKind::Unsubscribe => UNSUBSCRIBE_DOMAIN,
    });
    hasher.update(canonical);
    Ok(RequestHash::from_bytes(hasher.finalize().into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_request_hash_is_stable() {
        let hash = create_installation_request_hash(
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        )
        .expect("request hash");
        assert_eq!(hash.to_string().len(), 64);
        assert_eq!(hash.to_string().parse::<RequestHash>().unwrap(), hash);
    }

    #[test]
    fn subscription_actions_have_distinct_hash_domains() {
        let subscribe = subscription_request_hash(
            SubscriptionMutationKind::Subscribe,
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            7,
        )
        .unwrap();
        let unsubscribe = subscription_request_hash(
            SubscriptionMutationKind::Unsubscribe,
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            7,
        )
        .unwrap();
        assert_ne!(subscribe, unsubscribe);
    }

    #[test]
    fn identity_actions_have_distinct_hash_domains() {
        let operation = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1";
        let payload = ("safe", 7_u64);
        let domains = [
            IdentityMutationDomain::Claim,
            IdentityMutationDomain::Login,
            IdentityMutationDomain::RecoveryReset,
            IdentityMutationDomain::Backup,
            IdentityMutationDomain::DeviceRevoke,
            IdentityMutationDomain::TokenCreate,
            IdentityMutationDomain::TokenRevoke,
            IdentityMutationDomain::AccountDelete,
        ];
        let hashes = domains
            .into_iter()
            .map(|domain| identity_mutation_request_hash(operation, domain, &payload).unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(hashes.len(), domains.len());
    }
}
