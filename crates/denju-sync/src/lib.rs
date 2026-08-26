//! Deterministic desired-state and reconciliation logic.

use std::collections::{BTreeMap, BTreeSet};

use denju_core::ResourceId;

/// One independently durable reason a skill should exist on this device. The priority is
/// deliberately part of the pure sync model so local SQLite/network code cannot accidentally
/// invent its own precedence rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DesiredSourceKind {
    PersonalPack,
    DirectSubscription,
    OwnedWorkspace,
    TeamAssignment,
}

impl DesiredSourceKind {
    pub const fn is_enforced(self) -> bool {
        matches!(self, Self::TeamAssignment)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredSource {
    pub resource_id: String,
    pub revision_id: String,
    /// Stable source identity, for example `direct:<resource-id>` or
    /// `team:<team-id>:pack:<pack-id>`.
    pub source_id: String,
    /// Human-readable source locator used by status/recovery guidance.
    pub source_label: String,
    pub kind: DesiredSourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDesiredState {
    Absent,
    Selected {
        source: DesiredSource,
        suppressed_sources: Vec<DesiredSource>,
    },
    Conflict {
        sources: Vec<DesiredSource>,
        /// A conflict never chooses a winner. If the resource was already visible, callers
        /// preserve that exact last-good revision until the conflict is resolved.
        last_valid_revision_id: Option<String>,
    },
}

/// Resolve all current requirements for one immutable skill resource.
///
/// Only the strongest authority level participates in winner/conflict selection. Lower-priority
/// requirements remain returned as suppressed state so removing policy naturally reactivates
/// them rather than deleting user intent. Equal-authority disagreement never last-write-wins.
pub fn resolve_desired_sources(
    mut sources: Vec<DesiredSource>,
    last_valid_revision_id: Option<&str>,
) -> ResolvedDesiredState {
    if sources.is_empty() {
        return ResolvedDesiredState::Absent;
    }
    sources.sort_by(|left, right| {
        right
            .kind
            .cmp(&left.kind)
            .then_with(|| left.source_id.cmp(&right.source_id))
            .then_with(|| left.revision_id.cmp(&right.revision_id))
    });
    let strongest = sources[0].kind;
    let split = sources.partition_point(|source| source.kind == strongest);
    let mut active = sources[..split].to_vec();
    let suppressed = sources[split..].to_vec();
    let revisions = active
        .iter()
        .map(|source| source.revision_id.as_str())
        .collect::<BTreeSet<_>>();
    if revisions.len() > 1 {
        active.sort_by(|left, right| left.source_id.cmp(&right.source_id));
        return ResolvedDesiredState::Conflict {
            sources: active,
            last_valid_revision_id: last_valid_revision_id.map(str::to_owned),
        };
    }
    let source = active.remove(0);
    let mut suppressed_sources = active;
    suppressed_sources.extend(suppressed);
    ResolvedDesiredState::Selected {
        source,
        suppressed_sources,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSkillName {
    pub resource_id: ResourceId,
    pub owner: String,
    pub skill_name: String,
    pub previous_harness_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionAssignment {
    pub resource_id: ResourceId,
    pub harness_name: String,
    pub derived: bool,
}

/// Allocate one invocation name per managed skill across both harnesses. A canonical
/// Agent Skills name is retained only when it is unambiguous and not occupied by an
/// unmanaged skill. Collisions use human-readable owner-qualified aliases and preserve
/// an existing numbered allocation when possible so reconciliation does not renumber
/// stable invocation names just because a lower suffix later becomes available.
pub fn allocate_projection_names(
    skills: &[ManagedSkillName],
    reserved_names: &BTreeSet<String>,
) -> Vec<ProjectionAssignment> {
    let mut by_name = BTreeMap::<&str, Vec<&ManagedSkillName>>::new();
    for skill in skills {
        by_name.entry(&skill.skill_name).or_default().push(skill);
    }

    let mut ordered = skills.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|skill| skill.resource_id);
    let collides = |skill: &ManagedSkillName| {
        by_name
            .get(skill.skill_name.as_str())
            .is_some_and(|group| group.len() > 1)
            || reserved_names.contains(&skill.skill_name)
    };

    // Reserve every canonical name that will remain canonical before assigning aliases.
    // Otherwise a colliding resource processed first could accidentally claim another
    // managed skill's unambiguous canonical invocation as its owner-qualified alias.
    let mut used = reserved_names.clone();
    for skill in &ordered {
        if !collides(skill) {
            used.insert(skill.skill_name.clone());
        }
    }

    // Existing human aliases are sticky. Reserve all valid unique claims up front so a
    // newly colliding resource cannot steal a later resource's persisted allocation merely
    // because it sorts first by ResourceId. Legacy UUID-based aliases intentionally fail this
    // validation and are migrated to the cleaner naming scheme on the next reconcile.
    let mut sticky = BTreeMap::<ResourceId, String>::new();
    for skill in &ordered {
        if !collides(skill) {
            continue;
        }
        let Some(previous) = skill.previous_harness_name.as_deref() else {
            continue;
        };
        if valid_collision_alias(skill, previous) && !used.contains(previous) {
            used.insert(previous.to_owned());
            sticky.insert(skill.resource_id, previous.to_owned());
        }
    }

    let mut result = Vec::with_capacity(ordered.len());

    for skill in ordered {
        let collision = collides(skill);
        let harness_name = if !collision {
            skill.skill_name.clone()
        } else if let Some(previous) = sticky.get(&skill.resource_id) {
            previous.clone()
        } else {
            collision_alias(skill, &used)
        };
        used.insert(harness_name.clone());
        result.push(ProjectionAssignment {
            resource_id: skill.resource_id,
            derived: harness_name != skill.skill_name,
            harness_name,
        });
    }
    result
}

fn collision_alias(skill: &ManagedSkillName, used: &BTreeSet<String>) -> String {
    for index in 1_u64.. {
        let candidate = collision_alias_at(skill, index);
        if !used.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("u64 collision alias space exhausted")
}

fn valid_collision_alias(skill: &ManagedSkillName, candidate: &str) -> bool {
    if candidate == collision_alias_at(skill, 1) {
        return true;
    }
    let Some((_, suffix)) = candidate.rsplit_once('-') else {
        return false;
    };
    let Ok(index) = suffix.parse::<u64>() else {
        return false;
    };
    index >= 2 && candidate == collision_alias_at(skill, index)
}

fn collision_alias_at(skill: &ManagedSkillName, index: u64) -> String {
    let owner = sanitize_component(&skill.owner);
    let raw = format!("{owner}-{}", skill.skill_name);
    let suffix = if index == 1 {
        String::new()
    } else {
        format!("-{index}")
    };
    let room = 64_usize.saturating_sub(suffix.len());
    let mut base = raw;
    base.truncate(room);
    base = base.trim_matches('-').to_owned();
    if base.is_empty() {
        base = "skill".to_owned();
    }
    format!("{base}{suffix}")
}

fn sanitize_component(value: &str) -> String {
    let mut output = String::new();
    let mut pending_hyphen = false;
    for byte in value.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            if pending_hyphen && !output.is_empty() {
                output.push('-');
            }
            pending_hyphen = false;
            output.push(char::from(byte));
        } else {
            pending_hyphen = true;
        }
    }
    if output.is_empty() {
        "owner".to_owned()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use denju_core::validate_skill_name;

    use super::*;

    fn id(value: &str) -> ResourceId {
        ResourceId::from_str(value).unwrap()
    }

    fn skill(
        resource_id: &str,
        owner: &str,
        skill_name: &str,
        previous_harness_name: Option<&str>,
    ) -> ManagedSkillName {
        ManagedSkillName {
            resource_id: id(resource_id),
            owner: owner.to_owned(),
            skill_name: skill_name.to_owned(),
            previous_harness_name: previous_harness_name.map(str::to_owned),
        }
    }

    #[test]
    fn duplicate_skill_names_use_human_owner_qualified_aliases() {
        let skills = vec![
            skill(
                "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
                "alice",
                "review",
                None,
            ),
            skill(
                "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
                "bob",
                "review",
                None,
            ),
        ];
        let first = allocate_projection_names(&skills, &BTreeSet::new());
        let second = allocate_projection_names(&skills, &BTreeSet::new());
        assert_eq!(first, second);
        assert!(first.iter().all(|item| item.derived));
        assert_eq!(first[0].harness_name, "alice-review");
        assert_eq!(first[1].harness_name, "bob-review");
    }

    #[test]
    fn unmanaged_name_reserves_the_canonical_invocation() {
        let skill = skill(
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "alice",
            "review",
            None,
        );
        let reserved = BTreeSet::from(["review".to_owned()]);
        let assignment = allocate_projection_names(&[skill], &reserved);
        assert!(assignment[0].derived);
        assert_eq!(assignment[0].harness_name, "alice-review");
    }

    #[test]
    fn occupied_owner_alias_uses_lowest_available_numeric_suffix() {
        let skill = skill(
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "alice",
            "review",
            None,
        );
        let reserved = BTreeSet::from(["review".to_owned(), "alice-review".to_owned()]);
        let assignment = allocate_projection_names(&[skill], &reserved);
        assert_eq!(assignment[0].harness_name, "alice-review-2");
    }

    #[test]
    fn persisted_human_alias_is_sticky_when_lower_suffixes_become_free() {
        let skill = skill(
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "alice",
            "review",
            Some("alice-review-3"),
        );
        let reserved = BTreeSet::from(["review".to_owned()]);
        let assignment = allocate_projection_names(&[skill], &reserved);
        assert_eq!(assignment[0].harness_name, "alice-review-3");
    }

    #[test]
    fn legacy_uuid_alias_migrates_to_human_alias() {
        let skill = skill(
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "alice",
            "review",
            Some("denju-alice-review-01890f476a"),
        );
        let reserved = BTreeSet::from(["review".to_owned()]);
        let assignment = allocate_projection_names(&[skill], &reserved);
        assert_eq!(assignment[0].harness_name, "alice-review");
    }

    #[test]
    fn alias_allocator_does_not_steal_another_managed_canonical_name() {
        let skills = vec![
            skill(
                "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
                "alice",
                "review",
                None,
            ),
            skill(
                "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
                "charlie",
                "alice-review",
                None,
            ),
        ];
        let reserved = BTreeSet::from(["review".to_owned()]);
        let assignment = allocate_projection_names(&skills, &reserved);
        assert_eq!(assignment[0].harness_name, "alice-review-2");
        assert_eq!(assignment[1].harness_name, "alice-review");
    }

    #[test]
    fn human_collision_aliases_remain_valid_agent_skill_names_at_max_length() {
        let name = "r".repeat(64);
        let skill = skill(
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "a-very-long-owner-name",
            &name,
            None,
        );
        let reserved = BTreeSet::from([name]);
        let assignment = allocate_projection_names(&[skill], &reserved);
        assert!(validate_skill_name(&assignment[0].harness_name).is_ok());
        assert!(assignment[0].harness_name.len() <= 64);
    }

    fn source(kind: DesiredSourceKind, source_id: &str, revision: &str) -> DesiredSource {
        DesiredSource {
            resource_id: "skill-1".to_owned(),
            revision_id: revision.to_owned(),
            source_id: source_id.to_owned(),
            source_label: source_id.to_owned(),
            kind,
        }
    }

    #[test]
    fn enforced_team_assignment_suppresses_weaker_personal_intent_without_deleting_it() {
        let direct = source(DesiredSourceKind::DirectSubscription, "direct", "personal");
        let pack = source(DesiredSourceKind::PersonalPack, "pack", "pack-revision");
        let enforced = source(DesiredSourceKind::TeamAssignment, "team:acme", "approved");
        assert_eq!(
            resolve_desired_sources(vec![direct.clone(), enforced.clone(), pack.clone()], None),
            ResolvedDesiredState::Selected {
                source: enforced,
                suppressed_sources: vec![direct, pack],
            }
        );
    }

    #[test]
    fn equal_team_assignments_conflict_and_preserve_only_the_last_good_visibility() {
        let alpha = source(DesiredSourceKind::TeamAssignment, "team:alpha", "rev-a");
        let beta = source(DesiredSourceKind::TeamAssignment, "team:beta", "rev-b");
        assert_eq!(
            resolve_desired_sources(vec![beta.clone(), alpha.clone()], Some("last-good")),
            ResolvedDesiredState::Conflict {
                sources: vec![alpha, beta],
                last_valid_revision_id: Some("last-good".to_owned()),
            }
        );
    }

    #[test]
    fn matching_equal_authority_sources_are_one_requirement_not_a_conflict() {
        let alpha = source(DesiredSourceKind::TeamAssignment, "team:alpha", "same");
        let beta = source(DesiredSourceKind::TeamAssignment, "team:beta", "same");
        assert_eq!(
            resolve_desired_sources(vec![beta.clone(), alpha.clone()], None),
            ResolvedDesiredState::Selected {
                source: alpha,
                suppressed_sources: vec![beta],
            }
        );
    }
}
