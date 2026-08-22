use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{RequestHash, RequestHashError, hash_payload};

const TEAM_CREATE_DOMAIN: &[u8] = b"denju:http:v1:team-create\0";
const TEAM_INVITE_DOMAIN: &[u8] = b"denju:http:v1:team-invite\0";
const TEAM_INVITE_REVOKE_DOMAIN: &[u8] = b"denju:http:v1:team-invite-revoke\0";
const TEAM_JOIN_DOMAIN: &[u8] = b"denju:http:v1:team-join\0";
const TEAM_MEMBER_ROLE_DOMAIN: &[u8] = b"denju:http:v1:team-member-role\0";
const TEAM_MEMBER_REMOVE_DOMAIN: &[u8] = b"denju:http:v1:team-member-remove\0";
const TEAM_SETTINGS_DOMAIN: &[u8] = b"denju:http:v1:team-settings\0";
const RESOURCE_TRANSFER_DOMAIN: &[u8] = b"denju:http:v1:resource-transfer\0";

pub fn invite_code_hash(code: &str) -> RequestHash {
    RequestHash::from_bytes(Sha256::digest(code.as_bytes()).into())
}

pub fn team_create_request_hash(
    operation_id: &str,
    name: &str,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        name: &'a str,
    }
    hash_payload(TEAM_CREATE_DOMAIN, &Input { operation_id, name })
}

pub fn team_invite_request_hash(
    operation_id: &str,
    team: &str,
    role: crate::TeamRole,
    invite_code_hash: &str,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        team: &'a str,
        role: crate::TeamRole,
        invite_code_hash: &'a str,
    }
    hash_payload(
        TEAM_INVITE_DOMAIN,
        &Input {
            operation_id,
            team,
            role,
            invite_code_hash,
        },
    )
}

pub fn team_invite_revoke_request_hash(
    operation_id: &str,
    team: &str,
    invite_id: &str,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        team: &'a str,
        invite_id: &'a str,
    }
    hash_payload(
        TEAM_INVITE_REVOKE_DOMAIN,
        &Input {
            operation_id,
            team,
            invite_id,
        },
    )
}

pub fn team_join_request_hash(
    operation_id: &str,
    code: &str,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        code_hash: String,
    }
    hash_payload(
        TEAM_JOIN_DOMAIN,
        &Input {
            operation_id,
            code_hash: invite_code_hash(code).to_string(),
        },
    )
}

pub fn team_member_role_request_hash(
    operation_id: &str,
    team: &str,
    member: &str,
    role: crate::TeamRole,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        team: &'a str,
        member: &'a str,
        role: crate::TeamRole,
    }
    hash_payload(
        TEAM_MEMBER_ROLE_DOMAIN,
        &Input {
            operation_id,
            team,
            member,
            role,
        },
    )
}

pub fn team_member_remove_request_hash(
    operation_id: &str,
    team: &str,
    member: &str,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        team: &'a str,
        member: &'a str,
    }
    hash_payload(
        TEAM_MEMBER_REMOVE_DOMAIN,
        &Input {
            operation_id,
            team,
            member,
        },
    )
}

pub fn team_settings_request_hash(
    operation_id: &str,
    team: &str,
    members_can_publish: bool,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        team: &'a str,
        members_can_publish: bool,
    }
    hash_payload(
        TEAM_SETTINGS_DOMAIN,
        &Input {
            operation_id,
            team,
            members_can_publish,
        },
    )
}

pub fn resource_transfer_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
    destination_team: &str,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        resource_id: &'a str,
        expected_generation: u64,
        destination_team: &'a str,
    }
    hash_payload(
        RESOURCE_TRANSFER_DOMAIN,
        &Input {
            operation_id,
            resource_id,
            expected_generation,
            destination_team,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_invite_role_and_transfer_destination_are_hash_bound() {
        let operation = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1";
        let code_hash = "11".repeat(32);
        let member =
            team_invite_request_hash(operation, "@acme", crate::TeamRole::Member, &code_hash)
                .unwrap();
        let maintainer =
            team_invite_request_hash(operation, "@acme", crate::TeamRole::Maintainer, &code_hash)
                .unwrap();
        assert_ne!(member, maintainer);

        let resource = "01890f47-6a1d-7ad0-8f43-9a4d8c29f002";
        let acme = resource_transfer_request_hash(operation, resource, 4, "@acme").unwrap();
        let other = resource_transfer_request_hash(operation, resource, 4, "@other").unwrap();
        assert_ne!(acme, other);
    }
}
