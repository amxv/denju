use denju_sync::{DesiredSource, DesiredSourceKind, ResolvedDesiredState, resolve_desired_sources};
use proptest::prelude::*;

fn property_config() -> ProptestConfig {
    let cases = std::env::var("DENJU_PROPTEST_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(128);
    ProptestConfig::with_cases(cases)
}

fn kind(value: u8) -> DesiredSourceKind {
    match value % 4 {
        0 => DesiredSourceKind::PersonalPack,
        1 => DesiredSourceKind::DirectSubscription,
        2 => DesiredSourceKind::OwnedWorkspace,
        _ => DesiredSourceKind::TeamAssignment,
    }
}

proptest! {
    #![proptest_config(property_config())]

    #[test]
    fn desired_state_resolution_is_input_order_independent(
        source_map in prop::collection::btree_map(
            "source-[a-z0-9]{1,12}",
            (0_u8..4, "rev-[a-z0-9]{1,12}"),
            0..32,
        ),
        last_valid in prop::option::of("rev-[a-z0-9]{1,12}"),
    ) {
        let sources = source_map
            .into_iter()
            .map(|(source_id, (priority, revision_id))| DesiredSource {
                resource_id: "resource-1".to_owned(),
                revision_id,
                source_label: source_id.clone(),
                source_id,
                kind: kind(priority),
            })
            .collect::<Vec<_>>();
        let mut reversed = sources.clone();
        reversed.reverse();
        prop_assert_eq!(
            resolve_desired_sources(sources, last_valid.as_deref()),
            resolve_desired_sources(reversed, last_valid.as_deref())
        );
    }

    #[test]
    fn weaker_sources_cannot_override_an_enforced_team_revision(
        enforced_revision in "rev-[a-z0-9]{1,12}",
        weaker_revision in "rev-[a-z0-9]{1,12}",
    ) {
        let enforced = DesiredSource {
            resource_id: "resource-1".to_owned(),
            revision_id: enforced_revision.clone(),
            source_id: "team:enforced".to_owned(),
            source_label: "team:enforced".to_owned(),
            kind: DesiredSourceKind::TeamAssignment,
        };
        let weaker = DesiredSource {
            resource_id: "resource-1".to_owned(),
            revision_id: weaker_revision,
            source_id: "direct".to_owned(),
            source_label: "direct".to_owned(),
            kind: DesiredSourceKind::DirectSubscription,
        };
        match resolve_desired_sources(vec![weaker, enforced], None) {
            ResolvedDesiredState::Selected { source, .. } => {
                prop_assert_eq!(source.revision_id, enforced_revision);
            }
            other => prop_assert!(false, "unexpected desired state: {other:?}"),
        }
    }
}
