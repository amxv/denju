use std::str::FromStr;

use denju_wire::{
    IdentityMutationDomain, RequestHash, create_installation_request_hash,
    identity_mutation_request_hash,
};
use proptest::prelude::*;
use serde_json::{Map, Value};

fn property_config() -> ProptestConfig {
    let cases = std::env::var("DENJU_PROPTEST_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(128);
    ProptestConfig::with_cases(cases)
}

proptest! {
    #![proptest_config(property_config())]

    #[test]
    fn request_hash_round_trips(bytes in any::<[u8; 32]>()) {
        let hash = RequestHash::from_bytes(bytes);
        let parsed = RequestHash::from_str(&hash.to_string()).expect("request hash round trip");
        prop_assert_eq!(parsed, hash);
    }

    #[test]
    fn canonical_json_hash_ignores_object_insertion_order(
        entries in prop::collection::btree_map(
            "[a-z]{1,10}",
            -1_000_000_i64..1_000_000_i64,
            0..24,
        )
    ) {
        let mut forward = Map::new();
        for (key, value) in &entries {
            forward.insert(key.clone(), Value::from(*value));
        }
        let mut reverse = Map::new();
        for (key, value) in entries.iter().rev() {
            reverse.insert(key.clone(), Value::from(*value));
        }
        let first = identity_mutation_request_hash(
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            IdentityMutationDomain::Backup,
            &Value::Object(forward),
        )
        .expect("hash");
        let second = identity_mutation_request_hash(
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            IdentityMutationDomain::Backup,
            &Value::Object(reverse),
        )
        .expect("hash");
        prop_assert_eq!(first, second);
    }

    #[test]
    fn request_hash_domains_and_payloads_are_bound(
        credential in "[a-f0-9]{64}",
        other in "[a-f0-9]{64}",
    ) {
        let operation = "01890f47-6a1d-7ad0-8f43-9a4d8c29f002";
        let first = create_installation_request_hash(operation, &credential).expect("hash");
        let second = create_installation_request_hash(operation, &other).expect("hash");
        if credential != other {
            prop_assert_ne!(first, second);
        }
        let identity = identity_mutation_request_hash(
            operation,
            IdentityMutationDomain::Claim,
            &credential,
        )
        .expect("identity hash");
        prop_assert_ne!(first, identity);
    }
}
