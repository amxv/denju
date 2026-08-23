use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{RequestHash, RequestHashError};

const QUARANTINE_DOMAIN: &[u8] = b"denju:http:v1:admin-quarantine\0";
const UNQUARANTINE_DOMAIN: &[u8] = b"denju:http:v1:admin-unquarantine\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminQuarantineMutationKind {
    Quarantine,
    Unquarantine,
}

#[derive(Serialize)]
struct AdminQuarantineHashInput<'a> {
    operation_id: &'a str,
    resource_id: &'a str,
    expected_generation: u64,
    release_version: Option<u64>,
    reason: &'a str,
}

pub fn admin_quarantine_request_hash(
    kind: AdminQuarantineMutationKind,
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
    release_version: Option<u64>,
    reason: &str,
) -> Result<RequestHash, RequestHashError> {
    let payload = AdminQuarantineHashInput {
        operation_id,
        resource_id,
        expected_generation,
        release_version,
        reason,
    };
    let canonical = serde_json_canonicalizer::to_vec(&payload)
        .map_err(|error| RequestHashError::Canonicalization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(match kind {
        AdminQuarantineMutationKind::Quarantine => QUARANTINE_DOMAIN,
        AdminQuarantineMutationKind::Unquarantine => UNQUARANTINE_DOMAIN,
    });
    hasher.update(canonical);
    Ok(RequestHash::from_bytes(hasher.finalize().into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarantine_action_scope_and_generation_are_hash_bound() {
        let op = "01890f47-6a1d-7ad0-8f43-9a4d8c29f001";
        let resource = "01890f47-6a1d-7ad0-8f43-9a4d8c29f002";
        assert_ne!(
            admin_quarantine_request_hash(
                AdminQuarantineMutationKind::Quarantine,
                op,
                resource,
                3,
                Some(2),
                "malicious",
            )
            .unwrap(),
            admin_quarantine_request_hash(
                AdminQuarantineMutationKind::Unquarantine,
                op,
                resource,
                3,
                Some(2),
                "malicious",
            )
            .unwrap()
        );
        assert_ne!(
            admin_quarantine_request_hash(
                AdminQuarantineMutationKind::Quarantine,
                op,
                resource,
                3,
                Some(2),
                "malicious",
            )
            .unwrap(),
            admin_quarantine_request_hash(
                AdminQuarantineMutationKind::Quarantine,
                op,
                resource,
                4,
                Some(2),
                "malicious",
            )
            .unwrap()
        );
    }
}
