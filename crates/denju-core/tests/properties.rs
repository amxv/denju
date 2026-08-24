use std::str::FromStr;

use denju_core::{
    BlobId, OwnedSkillEntry, PortablePath, RevisionId, SkillMergeResult, TreeEntry, TreeEntryKind,
    TreeId, build_deterministic_skill_snapshot, merge_skill_entries, validate_skill_snapshot,
};
use proptest::prelude::*;

fn property_config() -> ProptestConfig {
    let cases = std::env::var("DENJU_PROPTEST_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(96);
    ProptestConfig::with_cases(cases)
}

fn skill_md(name: &str) -> Vec<u8> {
    format!("---\nname: {name}\ndescription: Deterministic property-test skill.\n---\n# Property\n")
        .into_bytes()
}

proptest! {
    #![proptest_config(property_config())]

    #[test]
    fn portable_path_parser_round_trips_every_accepted_input(input in ".{0,192}") {
        if let Ok(path) = PortablePath::parse(&input) {
            prop_assert_eq!(path.as_str(), input.as_str());
            prop_assert!(path.component_count() >= 1);
        }
    }

    #[test]
    fn object_ids_round_trip(bytes in any::<[u8; 32]>()) {
        let id = RevisionId::from_bytes(bytes);
        let parsed = RevisionId::from_str(&id.to_string()).expect("hex object ID round trips");
        prop_assert_eq!(parsed, id);
    }

    #[test]
    fn tree_identity_is_independent_of_input_order(
        files in prop::collection::btree_map(
            "file-[a-z0-9]{1,8}\\.txt",
            (any::<[u8; 32]>(), any::<bool>()),
            0..32,
        )
    ) {
        let entries = files
            .into_iter()
            .map(|(name, (bytes, executable))| {
                TreeEntry::new(
                    name,
                    TreeEntryKind::File {
                        blob: BlobId::hash(&bytes),
                        executable,
                    },
                )
                .expect("generated portable direct child")
            })
            .collect::<Vec<_>>();
        let mut reversed = entries.clone();
        reversed.reverse();
        prop_assert_eq!(
            TreeId::from_entries(&entries).expect("tree"),
            TreeId::from_entries(&reversed).expect("reversed tree")
        );
    }

    #[test]
    fn deterministic_snapshot_is_independent_of_entry_order(
        files in prop::collection::btree_map(
            "data-[a-z0-9]{1,8}\\.txt",
            prop::collection::vec(any::<u8>(), 0..256),
            0..12,
        )
    ) {
        let mut entries = vec![OwnedSkillEntry::File {
            path: "SKILL.md".to_owned(),
            bytes: skill_md("property-skill"),
            executable: false,
        }];
        entries.extend(files.into_iter().map(|(path, bytes)| OwnedSkillEntry::File {
            path,
            bytes,
            executable: false,
        }));
        let mut reversed = entries.clone();
        reversed.reverse();
        let first = build_deterministic_skill_snapshot("property-skill", &entries)
            .expect("snapshot");
        let second = build_deterministic_skill_snapshot("property-skill", &reversed)
            .expect("snapshot");
        prop_assert_eq!(first.manifest(), second.manifest());
        prop_assert_eq!(first.bytes(), second.bytes());
    }

    #[test]
    fn malformed_snapshot_bytes_never_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..4096)
    ) {
        let entries = vec![OwnedSkillEntry::File {
            path: "SKILL.md".to_owned(),
            bytes: skill_md("property-skill"),
            executable: false,
        }];
        let valid = build_deterministic_skill_snapshot("property-skill", &entries)
            .expect("fixture");
        let _ = validate_skill_snapshot("property-skill", valid.manifest(), &bytes);
    }

    #[test]
    fn disjoint_file_edits_merge_symmetrically(
        base_a in prop::collection::vec(any::<u8>(), 0..128),
        base_b in prop::collection::vec(any::<u8>(), 0..128),
        edit_a in prop::collection::vec(any::<u8>(), 0..128),
        edit_b in prop::collection::vec(any::<u8>(), 0..128),
    ) {
        let file = |path: &str, bytes: Vec<u8>| OwnedSkillEntry::File {
            path: path.to_owned(),
            bytes,
            executable: false,
        };
        let base = vec![file("a.txt", base_a.clone()), file("b.txt", base_b.clone())];
        let left = vec![file("a.txt", edit_a), file("b.txt", base_b)];
        let right = vec![file("a.txt", base_a), file("b.txt", edit_b)];
        let ab = merge_skill_entries(&base, &left, &right);
        let ba = merge_skill_entries(&base, &right, &left);
        match (ab, ba) {
            (
                SkillMergeResult::Clean { entries: ab },
                SkillMergeResult::Clean { entries: ba },
            ) => prop_assert_eq!(ab, ba),
            (left, right) => prop_assert_eq!(left, right),
        }
    }
}
