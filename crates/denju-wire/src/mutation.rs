use std::{fmt, str::FromStr};

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const CREATE_INSTALLATION_DOMAIN: &[u8] = b"denju:http:v1:create-installation\0";

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
}
