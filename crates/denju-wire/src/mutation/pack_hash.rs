use serde::Serialize;

use super::{
    PACK_ADD_DOMAIN, PACK_CREATE_DOMAIN, PACK_DELETE_DOMAIN, PACK_PUBLISH_DOMAIN,
    PACK_REMOVE_DOMAIN, PACK_RENAME_DOMAIN, PACK_SUBSCRIBE_DOMAIN, PACK_UNPUBLISH_DOMAIN,
    PACK_UNSUBSCRIBE_DOMAIN, PackMutationKind, PackSubscriptionMutationKind, RequestHash,
    RequestHashError, hash_payload, lifecycle_resource_hash,
};

pub fn pack_create_request_hash(
    operation_id: &str,
    owner: &str,
    name: &str,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        owner: &'a str,
        name: &'a str,
    }
    hash_payload(
        PACK_CREATE_DOMAIN,
        &Input {
            operation_id,
            owner,
            name,
        },
    )
}

pub fn pack_mutation_request_hash(
    kind: PackMutationKind,
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
    members: &[crate::PackMemberTarget],
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        resource_id: &'a str,
        expected_generation: u64,
        members: &'a [crate::PackMemberTarget],
    }
    hash_payload(
        match kind {
            PackMutationKind::Add => PACK_ADD_DOMAIN,
            PackMutationKind::Remove => PACK_REMOVE_DOMAIN,
        },
        &Input {
            operation_id,
            resource_id,
            expected_generation,
            members,
        },
    )
}

pub fn pack_publish_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
) -> Result<RequestHash, RequestHashError> {
    lifecycle_resource_hash(
        PACK_PUBLISH_DOMAIN,
        operation_id,
        resource_id,
        expected_generation,
    )
}

pub fn pack_rename_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
    new_name: &str,
) -> Result<RequestHash, RequestHashError> {
    #[derive(Serialize)]
    struct Input<'a> {
        operation_id: &'a str,
        resource_id: &'a str,
        expected_generation: u64,
        new_name: &'a str,
    }
    hash_payload(
        PACK_RENAME_DOMAIN,
        &Input {
            operation_id,
            resource_id,
            expected_generation,
            new_name,
        },
    )
}

pub fn pack_unpublish_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
) -> Result<RequestHash, RequestHashError> {
    lifecycle_resource_hash(
        PACK_UNPUBLISH_DOMAIN,
        operation_id,
        resource_id,
        expected_generation,
    )
}

pub fn pack_delete_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
) -> Result<RequestHash, RequestHashError> {
    lifecycle_resource_hash(
        PACK_DELETE_DOMAIN,
        operation_id,
        resource_id,
        expected_generation,
    )
}

pub fn pack_subscription_request_hash(
    kind: PackSubscriptionMutationKind,
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
) -> Result<RequestHash, RequestHashError> {
    lifecycle_resource_hash(
        match kind {
            PackSubscriptionMutationKind::Subscribe => PACK_SUBSCRIBE_DOMAIN,
            PackSubscriptionMutationKind::Unsubscribe => PACK_UNSUBSCRIBE_DOMAIN,
        },
        operation_id,
        resource_id,
        expected_generation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_add_remove_and_pin_changes_have_distinct_request_hashes() {
        let operation = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1";
        let pack = "01890f47-6a1d-7ad0-8f43-9a4d8c29f002";
        let member = "01890f47-6a1e-72ce-88bf-ef23fc661004";
        let following = vec![crate::PackMemberTarget {
            resource_id: member.to_owned(),
            release_version: None,
        }];
        let pinned = vec![crate::PackMemberTarget {
            resource_id: member.to_owned(),
            release_version: Some(3),
        }];
        let add = pack_mutation_request_hash(PackMutationKind::Add, operation, pack, 4, &following)
            .unwrap();
        let remove =
            pack_mutation_request_hash(PackMutationKind::Remove, operation, pack, 4, &following)
                .unwrap();
        let pinned_add =
            pack_mutation_request_hash(PackMutationKind::Add, operation, pack, 4, &pinned).unwrap();
        assert_ne!(add, remove);
        assert_ne!(add, pinned_add);
    }

    #[test]
    fn pack_subscribe_and_unsubscribe_have_distinct_request_hashes() {
        let operation = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1";
        let pack = "01890f47-6a1d-7ad0-8f43-9a4d8c29f002";
        let subscribe = pack_subscription_request_hash(
            PackSubscriptionMutationKind::Subscribe,
            operation,
            pack,
            8,
        )
        .unwrap();
        let unsubscribe = pack_subscription_request_hash(
            PackSubscriptionMutationKind::Unsubscribe,
            operation,
            pack,
            8,
        )
        .unwrap();
        assert_ne!(subscribe, unsubscribe);
    }
}
