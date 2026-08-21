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
const PRIVATE_REVISION_DOMAIN: &[u8] = b"denju:http:v1:private-revision\0";
const PRIVATE_SKILL_IMPORT_DOMAIN: &[u8] = b"denju:http:v1:private-skill-import\0";
const PUBLISH_SKILL_DOMAIN: &[u8] = b"denju:http:v1:publish-skill\0";
const RESTORE_SKILL_DOMAIN: &[u8] = b"denju:http:v1:restore-skill\0";
const RENAME_SKILL_DOMAIN: &[u8] = b"denju:http:v1:rename-skill\0";
const UNPUBLISH_SKILL_DOMAIN: &[u8] = b"denju:http:v1:unpublish-skill\0";
const DELETE_SKILL_DOMAIN: &[u8] = b"denju:http:v1:delete-skill\0";
const DEPRECATE_SKILL_DOMAIN: &[u8] = b"denju:http:v1:deprecate-skill\0";
const HISTORY_PRUNE_DOMAIN: &[u8] = b"denju:http:v1:history-prune\0";
const SHARE_SKILL_DOMAIN: &[u8] = b"denju:http:v1:share-skill\0";
const UNSHARE_SKILL_DOMAIN: &[u8] = b"denju:http:v1:unshare-skill\0";

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
    release_version: Option<u64>,
    retain_on_delete: bool,
}

pub fn subscription_request_hash(
    kind: SubscriptionMutationKind,
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
    release_version: Option<u64>,
    retain_on_delete: bool,
) -> Result<RequestHash, RequestHashError> {
    let payload = SubscriptionHashInput {
        operation_id,
        resource_id,
        expected_generation,
        release_version,
        retain_on_delete,
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

pub fn rename_skill_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
    new_name: &str,
    prepared_revision_operation_id: Option<&str>,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        resource_id: &'a str,
        expected_generation: u64,
        new_name: &'a str,
        prepared_revision_operation_id: Option<&'a str>,
    }
    hash_payload(
        RENAME_SKILL_DOMAIN,
        &Input {
            operation_id,
            resource_id,
            expected_generation,
            new_name,
            prepared_revision_operation_id,
        },
    )
}

pub fn unpublish_skill_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
) -> Result<RequestHash, RequestHashError> {
    lifecycle_resource_hash(
        UNPUBLISH_SKILL_DOMAIN,
        operation_id,
        resource_id,
        expected_generation,
    )
}

pub fn delete_skill_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
) -> Result<RequestHash, RequestHashError> {
    lifecycle_resource_hash(
        DELETE_SKILL_DOMAIN,
        operation_id,
        resource_id,
        expected_generation,
    )
}

pub fn history_prune_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
) -> Result<RequestHash, RequestHashError> {
    lifecycle_resource_hash(
        HISTORY_PRUNE_DOMAIN,
        operation_id,
        resource_id,
        expected_generation,
    )
}

pub fn deprecate_skill_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
    deprecated: bool,
    replacement_resource_id: Option<&str>,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        resource_id: &'a str,
        expected_generation: u64,
        deprecated: bool,
        replacement_resource_id: Option<&'a str>,
    }
    hash_payload(
        DEPRECATE_SKILL_DOMAIN,
        &Input {
            operation_id,
            resource_id,
            expected_generation,
            deprecated,
            replacement_resource_id,
        },
    )
}

fn lifecycle_resource_hash(
    domain: &[u8],
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        resource_id: &'a str,
        expected_generation: u64,
    }
    hash_payload(
        domain,
        &Input {
            operation_id,
            resource_id,
            expected_generation,
        },
    )
}

fn hash_payload<T: Serialize>(domain: &[u8], payload: &T) -> Result<RequestHash, RequestHashError> {
    let canonical = serde_json_canonicalizer::to_vec(payload)
        .map_err(|error| RequestHashError::Canonicalization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(canonical);
    Ok(RequestHash::from_bytes(hasher.finalize().into()))
}

pub fn publish_skill_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
    message: Option<&str>,
    tags: &[String],
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct HashInput<'a> {
        operation_id: &'a str,
        resource_id: &'a str,
        expected_generation: u64,
        message: Option<&'a str>,
        tags: &'a [String],
    }
    let canonical = serde_json_canonicalizer::to_vec(&HashInput {
        operation_id,
        resource_id,
        expected_generation,
        message,
        tags,
    })
    .map_err(|error| RequestHashError::Canonicalization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(PUBLISH_SKILL_DOMAIN);
    hasher.update(canonical);
    Ok(RequestHash::from_bytes(hasher.finalize().into()))
}

pub fn restore_skill_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
    target_revision_id: &str,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct HashInput<'a> {
        operation_id: &'a str,
        resource_id: &'a str,
        expected_generation: u64,
        target_revision_id: &'a str,
    }
    let canonical = serde_json_canonicalizer::to_vec(&HashInput {
        operation_id,
        resource_id,
        expected_generation,
        target_revision_id,
    })
    .map_err(|error| RequestHashError::Canonicalization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(RESTORE_SKILL_DOMAIN);
    hasher.update(canonical);
    Ok(RequestHash::from_bytes(hasher.finalize().into()))
}

#[derive(Serialize)]
pub struct PrivateRevisionRequestHashInput<'a> {
    pub operation_id: &'a str,
    pub resource_id: &'a str,
    pub expected_generation: u64,
    pub expected_head_revision_id: &'a str,
    pub parent_revision_ids: &'a [String],
    pub manifest: &'a crate::PublicSkillManifest,
    pub revision_author_principal_id: Option<&'a str>,
    pub fork_sync: Option<&'a crate::ForkSyncIntent>,
    pub historical_skill_name: Option<&'a str>,
}

pub fn private_revision_request_hash(
    input: &PrivateRevisionRequestHashInput<'_>,
) -> Result<RequestHash, RequestHashError> {
    let canonical = serde_json_canonicalizer::to_vec(input)
        .map_err(|error| RequestHashError::Canonicalization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(PRIVATE_REVISION_DOMAIN);
    hasher.update(canonical);
    Ok(RequestHash::from_bytes(hasher.finalize().into()))
}

#[derive(Serialize)]
pub struct PrivateSkillImportRequestHashInput<'a, T> {
    pub operation_id: &'a str,
    pub expected_generation: u64,
    pub name: &'a str,
    pub manifest: &'a T,
    pub snapshot_sha256: &'a str,
    pub snapshot_size_bytes: u64,
    pub revision_author_principal_id: Option<&'a str>,
    pub fork: Option<&'a crate::ForkImportIntent>,
}

pub fn private_skill_import_request_hash<T: Serialize>(
    input: &PrivateSkillImportRequestHashInput<'_, T>,
) -> Result<RequestHash, RequestHashError> {
    let canonical = serde_json_canonicalizer::to_vec(input)
        .map_err(|error| RequestHashError::Canonicalization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(PRIVATE_SKILL_IMPORT_DOMAIN);
    hasher.update(canonical);
    Ok(RequestHash::from_bytes(hasher.finalize().into()))
}

pub fn share_skill_request_hash(
    kind: crate::ShareMutationKind,
    operation_id: &str,
    resource_id: &str,
    recipient: &str,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct HashInput<'a> {
        operation_id: &'a str,
        resource_id: &'a str,
        recipient: &'a str,
    }
    let canonical = serde_json_canonicalizer::to_vec(&HashInput {
        operation_id,
        resource_id,
        recipient,
    })
    .map_err(|error| RequestHashError::Canonicalization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(match kind {
        crate::ShareMutationKind::Share => SHARE_SKILL_DOMAIN,
        crate::ShareMutationKind::Unshare => UNSHARE_SKILL_DOMAIN,
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
            None,
            false,
        )
        .unwrap();
        let unsubscribe = subscription_request_hash(
            SubscriptionMutationKind::Unsubscribe,
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            7,
            None,
            false,
        )
        .unwrap();
        assert_ne!(subscribe, unsubscribe);
    }

    #[test]
    fn private_share_hash_binds_action_resource_and_recipient() {
        let operation = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1";
        let resource = "01890f47-6a1d-7ad0-8f43-9a4d8c29f002";
        let share =
            share_skill_request_hash(crate::ShareMutationKind::Share, operation, resource, "@bob")
                .unwrap();
        let unshare = share_skill_request_hash(
            crate::ShareMutationKind::Unshare,
            operation,
            resource,
            "@bob",
        )
        .unwrap();
        let other_recipient = share_skill_request_hash(
            crate::ShareMutationKind::Share,
            operation,
            resource,
            "@charlie",
        )
        .unwrap();
        let other_resource = share_skill_request_hash(
            crate::ShareMutationKind::Share,
            operation,
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f003",
            "@bob",
        )
        .unwrap();
        assert_ne!(share, unshare);
        assert_ne!(share, other_recipient);
        assert_ne!(share, other_resource);
    }

    #[test]
    fn subscription_pin_is_part_of_the_request_hash() {
        let following_latest = subscription_request_hash(
            SubscriptionMutationKind::Subscribe,
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            7,
            None,
            false,
        )
        .unwrap();
        let pinned = subscription_request_hash(
            SubscriptionMutationKind::Subscribe,
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            7,
            Some(3),
            false,
        )
        .unwrap();
        assert_ne!(following_latest, pinned);
    }

    #[test]
    fn subscription_delete_retention_is_part_of_the_request_hash() {
        let ordinary = subscription_request_hash(
            SubscriptionMutationKind::Subscribe,
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            7,
            None,
            false,
        )
        .unwrap();
        let retained = subscription_request_hash(
            SubscriptionMutationKind::Subscribe,
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            7,
            None,
            true,
        )
        .unwrap();
        assert_ne!(ordinary, retained);
    }

    #[test]
    fn lifecycle_hashes_bind_action_and_mutable_fields() {
        let operation = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1";
        let resource = "01890f47-6a1d-7ad0-8f43-9a4d8c29f002";
        let unpublish = unpublish_skill_request_hash(operation, resource, 7).unwrap();
        let delete = delete_skill_request_hash(operation, resource, 7).unwrap();
        let prune = history_prune_request_hash(operation, resource, 7).unwrap();
        assert_ne!(unpublish, delete);
        assert_ne!(delete, prune);

        let renamed = rename_skill_request_hash(operation, resource, 7, "renamed", None).unwrap();
        let renamed_again =
            rename_skill_request_hash(operation, resource, 7, "other", None).unwrap();
        let prepared = rename_skill_request_hash(
            operation,
            resource,
            7,
            "renamed",
            Some("01890f47-6a1c-7cc2-98c1-5f6c1ed8a3ff"),
        )
        .unwrap();
        assert_ne!(renamed, renamed_again);
        assert_ne!(renamed, prepared);

        let deprecated = deprecate_skill_request_hash(operation, resource, 7, true, None).unwrap();
        let replacement = deprecate_skill_request_hash(
            operation,
            resource,
            7,
            true,
            Some("01890f47-6a1e-72ce-88bf-ef23fc661004"),
        )
        .unwrap();
        let restored = deprecate_skill_request_hash(operation, resource, 7, false, None).unwrap();
        assert_ne!(deprecated, replacement);
        assert_ne!(deprecated, restored);
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

    #[test]
    fn private_import_hash_binds_manifest_and_snapshot() {
        let operation = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1";
        let manifest = serde_json::json!({"root_tree_id": "11", "entries": []});
        let snapshot = "22".repeat(32);
        let left = private_skill_import_request_hash(&PrivateSkillImportRequestHashInput {
            operation_id: operation,
            expected_generation: 0,
            name: "review",
            manifest: &manifest,
            snapshot_sha256: &snapshot,
            snapshot_size_bytes: 42,
            revision_author_principal_id: None,
            fork: None,
        })
        .unwrap();
        let right = private_skill_import_request_hash(&PrivateSkillImportRequestHashInput {
            operation_id: operation,
            expected_generation: 0,
            name: "review",
            manifest: &manifest,
            snapshot_sha256: &snapshot,
            snapshot_size_bytes: 43,
            revision_author_principal_id: None,
            fork: None,
        })
        .unwrap();
        assert_ne!(left, right);
    }

    #[test]
    fn private_import_hash_binds_replay_author_and_promotion_intent() {
        let operation = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1";
        let manifest = serde_json::json!({"root_tree_id": "11", "entries": []});
        let fork = crate::ForkImportIntent {
            upstream_resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".to_owned(),
            upstream_revision_id: "22".repeat(32),
            replace_subscription: true,
            promotion_head_revision_id: Some("33".repeat(32)),
            historical_skill_name: Some("review".to_owned()),
        };
        let snapshot = "44".repeat(32);
        let base = private_skill_import_request_hash(&PrivateSkillImportRequestHashInput {
            operation_id: operation,
            expected_generation: 0,
            name: "review-renamed",
            manifest: &manifest,
            snapshot_sha256: &snapshot,
            snapshot_size_bytes: 42,
            revision_author_principal_id: Some("01890f47-6a1e-72ce-88bf-ef23fc661004"),
            fork: Some(&fork),
        })
        .unwrap();
        let other_author = private_skill_import_request_hash(&PrivateSkillImportRequestHashInput {
            operation_id: operation,
            expected_generation: 0,
            name: "review-renamed",
            manifest: &manifest,
            snapshot_sha256: &snapshot,
            snapshot_size_bytes: 42,
            revision_author_principal_id: Some("01890f47-6a1f-72ce-88bf-ef23fc661005"),
            fork: Some(&fork),
        })
        .unwrap();
        let mut other_fork = fork.clone();
        other_fork.promotion_head_revision_id = Some("55".repeat(32));
        let other_promotion =
            private_skill_import_request_hash(&PrivateSkillImportRequestHashInput {
                operation_id: operation,
                expected_generation: 0,
                name: "review-renamed",
                manifest: &manifest,
                snapshot_sha256: &snapshot,
                snapshot_size_bytes: 42,
                revision_author_principal_id: Some("01890f47-6a1e-72ce-88bf-ef23fc661004"),
                fork: Some(&other_fork),
            })
            .unwrap();
        assert_ne!(base, other_author);
        assert_ne!(base, other_promotion);
    }

    #[test]
    fn private_revision_hash_binds_parent_generation_and_manifest() {
        use denju_core::{OwnedSkillEntry, build_skill_manifest};

        let manifest = crate::PublicSkillManifest::from_core(
            &build_skill_manifest(
                "review",
                &[OwnedSkillEntry::File {
                    path: "SKILL.md".into(),
                    bytes: b"---\nname: review\ndescription: Review.\n---\n".to_vec(),
                    executable: false,
                }],
            )
            .unwrap(),
        );
        let head = "11".repeat(32);
        let parents = vec![head.clone()];
        let a = private_revision_request_hash(&PrivateRevisionRequestHashInput {
            operation_id: "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            expected_generation: 7,
            expected_head_revision_id: &head,
            parent_revision_ids: &parents,
            manifest: &manifest,
            revision_author_principal_id: None,
            fork_sync: None,
            historical_skill_name: None,
        })
        .unwrap();
        let b = private_revision_request_hash(&PrivateRevisionRequestHashInput {
            operation_id: "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            expected_generation: 8,
            expected_head_revision_id: &head,
            parent_revision_ids: &parents,
            manifest: &manifest,
            revision_author_principal_id: None,
            fork_sync: None,
            historical_skill_name: None,
        })
        .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn private_revision_hash_binds_fork_sync_and_historical_replay() {
        use denju_core::{OwnedSkillEntry, build_skill_manifest};

        let manifest = crate::PublicSkillManifest::from_core(
            &build_skill_manifest(
                "review",
                &[OwnedSkillEntry::File {
                    path: "SKILL.md".into(),
                    bytes: b"---\nname: review\ndescription: Review.\n---\n".to_vec(),
                    executable: false,
                }],
            )
            .unwrap(),
        );
        let sync = crate::ForkSyncIntent {
            expected_sync_base_revision_id: "22".repeat(32),
            upstream_revision_id: "33".repeat(32),
        };
        let head = "11".repeat(32);
        let parents = vec![head.clone(), "33".repeat(32)];
        let base = private_revision_request_hash(&PrivateRevisionRequestHashInput {
            operation_id: "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            expected_generation: 7,
            expected_head_revision_id: &head,
            parent_revision_ids: &parents,
            manifest: &manifest,
            revision_author_principal_id: None,
            fork_sync: Some(&sync),
            historical_skill_name: None,
        })
        .unwrap();
        let historical = private_revision_request_hash(&PrivateRevisionRequestHashInput {
            operation_id: "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            expected_generation: 7,
            expected_head_revision_id: &head,
            parent_revision_ids: &parents,
            manifest: &manifest,
            revision_author_principal_id: None,
            fork_sync: Some(&sync),
            historical_skill_name: Some("old-review"),
        })
        .unwrap();
        let other_sync_intent = crate::ForkSyncIntent {
            expected_sync_base_revision_id: "22".repeat(32),
            upstream_revision_id: "44".repeat(32),
        };
        let other_parents = vec![head.clone(), "44".repeat(32)];
        let other_sync = private_revision_request_hash(&PrivateRevisionRequestHashInput {
            operation_id: "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            expected_generation: 7,
            expected_head_revision_id: &head,
            parent_revision_ids: &other_parents,
            manifest: &manifest,
            revision_author_principal_id: None,
            fork_sync: Some(&other_sync_intent),
            historical_skill_name: None,
        })
        .unwrap();
        assert_ne!(base, historical);
        assert_ne!(base, other_sync);
    }
}
