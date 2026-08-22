use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    str::FromStr,
};

use denju_core::{OperationId, ResourceId, RevisionId};
use denju_local::{
    DesiredSkillMaterialization, PackApplyJournalPayload, PackApplySkillState,
    PackMaterializedSkillRecord, PackSkillSourceRecord, PackSourceConflictRecord,
    PackSubscriptionRecord, restore_skill_generation, stage_skill_generation,
    switch_staged_skill_generation,
};
use denju_sync::{DesiredSource, DesiredSourceKind, ResolvedDesiredState, resolve_desired_sources};
use denju_wire::{CliErrorCode, PackRequirementKind, SubscribedSkill};
use uuid::Uuid;

use crate::{
    context::{InstalledContext, now_unix_ms},
    public::{client_error, local_error},
    setup::RuntimeError,
};

#[derive(Debug, Clone)]
pub(crate) struct PackCatalogState {
    pub(crate) desired: BTreeMap<String, SubscribedSkill>,
    pub(crate) conflicts: Vec<PackSourceConflictRecord>,
}

#[derive(Debug, Clone)]
struct PackCandidate {
    source: DesiredSource,
    desired: Option<SubscribedSkill>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PackApplyOutcome {
    pub(crate) materialized: usize,
    pub(crate) removed: usize,
}

pub(crate) async fn recover_incomplete_apply(
    context: &InstalledContext,
) -> Result<(), RuntimeError> {
    recover_incomplete_apply_state(&context.paths, &context.db).await
}

async fn recover_incomplete_apply_state(
    paths: &denju_local::LocalPaths,
    db: &denju_local::LocalDatabase,
) -> Result<(), RuntimeError> {
    for journal in db
        .incomplete_pack_apply_journals()
        .await
        .map_err(local_error)?
    {
        for old in &journal.payload.old_skills {
            restore_skill_generation(
                paths,
                &old.resource_id,
                &old.owner,
                &old.skill_name,
                old.revision_id.as_deref(),
                journal.operation_id,
            )
            .map_err(local_error)?;
        }
        db.discard_pack_apply_journal(journal.operation_id)
            .await
            .map_err(local_error)?;
    }
    Ok(())
}

pub(crate) async fn refresh_catalog(
    context: &InstalledContext,
) -> Result<PackCatalogState, RuntimeError> {
    let catalog = context
        .client
        .pack_subscriptions()
        .await
        .map_err(client_error)?;
    let now = now_unix_ms();
    let mut packs = Vec::with_capacity(catalog.packs.len());
    let mut sources = Vec::new();
    let mut candidates: BTreeMap<String, Vec<PackCandidate>> = BTreeMap::new();
    for requirement in catalog.packs {
        let source = requirement.source;
        let pack = requirement.pack;
        let enforced = source.kind == PackRequirementKind::TeamAssignment;
        packs.push(PackSubscriptionRecord {
            source_id: source.source_id.clone(),
            source_kind: match source.kind {
                PackRequirementKind::Direct => "direct",
                PackRequirementKind::TeamAssignment => "team_assignment",
            }
            .to_owned(),
            source_label: source.label.clone(),
            source_team_id: source.team_namespace_id.clone(),
            enforced,
            pack_resource_id: pack.pack.resource_id.clone(),
            locator: pack.pack.locator.clone(),
            resource_generation: i64::try_from(pack.pack.generation)
                .map_err(|_| local("pack generation exceeds local database range"))?,
            pack_version: i64::try_from(pack.pack.version)
                .map_err(|_| local("pack version exceeds local database range"))?,
            degraded: pack.pack.degraded,
        });
        for member in pack.members {
            let desired = member.desired;
            let (owner, skill_name) = locator_parts(&member.locator)?;
            let generation = desired
                .as_ref()
                .map(|desired| desired.generation)
                .unwrap_or(0);
            sources.push(PackSkillSourceRecord {
                source_id: source.source_id.clone(),
                pack_resource_id: pack.pack.resource_id.clone(),
                resource_id: member.resource_id.clone(),
                locator: member.locator.clone(),
                owner,
                skill_name,
                resource_generation: i64::try_from(generation)
                    .map_err(|_| local("pack member generation exceeds local database range"))?,
                desired_revision_id: member.revision_id.clone(),
                unavailable_reason: member
                    .unavailable_reason
                    .map(|reason| reason.as_str().to_owned()),
            });
            candidates
                .entry(member.resource_id.clone())
                .or_default()
                .push(PackCandidate {
                    source: DesiredSource {
                        resource_id: member.resource_id,
                        revision_id: member.revision_id,
                        source_id: source.source_id.clone(),
                        source_label: format!("{} via {}", source.label, pack.pack.locator),
                        kind: if enforced {
                            DesiredSourceKind::TeamAssignment
                        } else {
                            DesiredSourceKind::PersonalPack
                        },
                    },
                    desired,
                });
        }
    }
    context
        .db
        .replace_pack_catalog(packs, sources, now)
        .await
        .map_err(local_error)?;

    let subscriptions = context
        .db
        .subscriptions()
        .await
        .map_err(local_error)?
        .into_iter()
        .map(|record| (record.resource_id.clone(), record))
        .collect::<BTreeMap<_, _>>();
    let owned = context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .map(|record| (record.resource_id.clone(), record))
        .collect::<BTreeMap<_, _>>();
    // Source overlap is expected during a resolver transition (for example, a direct
    // subscription created while an enforced pack is already active). `managed_skills()`
    // intentionally rejects two active owners, so derive the last-good revision from the
    // canonical pointer itself before suppression has been reconciled.
    let mut active = BTreeMap::new();
    for record in subscriptions.values() {
        if let Some(revision) = record.materialized_revision_id.as_deref()
            && canonical_targets_revision(
                context,
                &record.resource_id,
                &record.owner,
                &record.skill_name,
                revision,
            )
        {
            active.insert(record.resource_id.clone(), revision.to_owned());
        }
    }
    for record in owned.values() {
        if let Some(revision) = record.materialized_revision_id.as_deref()
            && canonical_targets_revision(
                context,
                &record.resource_id,
                &record.owner,
                &record.skill_name,
                revision,
            )
        {
            active.insert(record.resource_id.clone(), revision.to_owned());
        }
    }
    for record in context
        .db
        .pack_materialized_skills()
        .await
        .map_err(local_error)?
    {
        if canonical_targets_revision(
            context,
            &record.resource_id,
            &record.owner,
            &record.skill_name,
            &record.materialized_revision_id,
        ) {
            active.insert(record.resource_id, record.materialized_revision_id);
        }
    }

    let mut desired = BTreeMap::new();
    let mut conflicts = Vec::new();
    let mut suppress_subscriptions = Vec::new();
    let mut suppress_owned = Vec::new();
    let mut preserve_suppressions = Vec::new();
    for (resource_id, requirements) in candidates {
        let mut all_sources = requirements
            .iter()
            .map(|candidate| candidate.source.clone())
            .collect::<Vec<_>>();
        if let Some(record) = subscriptions.get(&resource_id) {
            all_sources.push(DesiredSource {
                resource_id: resource_id.clone(),
                revision_id: record.desired_revision_id.clone(),
                source_id: format!("direct-skill:{resource_id}"),
                source_label: format!("direct subscription {}", record.locator),
                kind: DesiredSourceKind::DirectSubscription,
            });
        }
        if let Some(record) = owned.get(&resource_id) {
            all_sources.push(DesiredSource {
                resource_id: resource_id.clone(),
                revision_id: record.desired_revision_id.clone(),
                source_id: format!("workspace:{resource_id}"),
                source_label: format!("workspace {}", record.locator),
                kind: DesiredSourceKind::OwnedWorkspace,
            });
        }
        match resolve_desired_sources(all_sources, active.get(&resource_id).map(String::as_str)) {
            ResolvedDesiredState::Selected { source, .. } => {
                if source.kind == DesiredSourceKind::TeamAssignment {
                    if subscriptions.contains_key(&resource_id) {
                        suppress_subscriptions
                            .push((resource_id.clone(), source.source_id.clone()));
                    }
                    if owned.contains_key(&resource_id) {
                        suppress_owned.push((resource_id.clone(), source.source_id.clone()));
                    }
                }
                if let Some(candidate) = requirements
                    .into_iter()
                    .find(|candidate| candidate.source.source_id == source.source_id)
                    && let Some(value) = candidate.desired
                {
                    desired.insert(resource_id, value);
                }
            }
            ResolvedDesiredState::Conflict { sources, .. } => {
                preserve_suppressions.push(resource_id.clone());
                conflicts.push(PackSourceConflictRecord {
                    resource_id: resource_id.clone(),
                    source_ids: sources.iter().map(|source| source.source_id.clone()).collect(),
                    source_labels: sources
                        .iter()
                        .map(|source| source.source_label.clone())
                        .collect(),
                    revision_ids: sources
                        .iter()
                        .map(|source| source.revision_id.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                    message: format!(
                        "equal-authority desired sources disagree for resource {resource_id}; reconcile the team assignments instead of choosing a winner"
                    ),
                });
            }
            ResolvedDesiredState::Absent => {}
        }
    }
    context
        .db
        .reconcile_source_suppressions(
            suppress_subscriptions,
            suppress_owned,
            preserve_suppressions,
            now,
        )
        .await
        .map_err(local_error)?;
    Ok(PackCatalogState { desired, conflicts })
}

fn canonical_targets_revision(
    context: &InstalledContext,
    resource_id: &str,
    owner: &str,
    skill_name: &str,
    revision_id: &str,
) -> bool {
    let canonical = context.paths.skills.join(owner).join(skill_name);
    let expected = context
        .paths
        .generations
        .join(resource_id)
        .join(revision_id);
    match (
        std::fs::canonicalize(canonical),
        std::fs::canonicalize(expected),
    ) {
        (Ok(canonical), Ok(expected)) => canonical == expected,
        _ => false,
    }
}

pub(crate) async fn apply_pack_only_state(
    context: &InstalledContext,
    state: &PackCatalogState,
) -> Result<PackApplyOutcome, RuntimeError> {
    let direct = context.db.subscriptions().await.map_err(local_error)?;
    let direct_ids = direct
        .iter()
        .map(|record| record.resource_id.clone())
        .collect::<BTreeSet<_>>();
    let suppressed_direct = context
        .db
        .source_suppressions("subscription")
        .await
        .map_err(local_error)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let owned = context.db.owned_skills().await.map_err(local_error)?;
    let owned_ids = owned
        .iter()
        .map(|record| record.resource_id.clone())
        .collect::<BTreeSet<_>>();
    let suppressed_owned = context
        .db
        .source_suppressions("owned")
        .await
        .map_err(local_error)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    // These are active stronger local owners that suppress a personal pack source. Sources
    // suppressed *by* team policy are intentionally excluded so the enforced pack can own the
    // single visible projection without deleting the weaker relationship row.
    let active_direct = direct_ids
        .difference(&suppressed_direct)
        .cloned()
        .collect::<BTreeSet<_>>();
    let active_owned = owned_ids
        .difference(&suppressed_owned)
        .cloned()
        .collect::<BTreeSet<_>>();
    let suppressed = active_direct
        .union(&active_owned)
        .cloned()
        .collect::<BTreeSet<_>>();
    // A pack can relinquish a resource to an already-materialized stronger local source without
    // the registry sending that unchanged source again. Keep the exact local revision/path that
    // should become visible when the pack row disappears. Owned workspace authority is stronger
    // than a direct subscription in the resolver and therefore deterministically overwrites the
    // direct fallback if both somehow coexist during a transition.
    let mut local_fallbacks = BTreeMap::new();
    for record in &direct {
        if suppressed_direct.contains(&record.resource_id) {
            continue;
        }
        if let Some(revision_id) = record.materialized_revision_id.clone() {
            local_fallbacks.insert(
                record.resource_id.clone(),
                PackApplySkillState {
                    resource_id: record.resource_id.clone(),
                    owner: record.owner.clone(),
                    skill_name: record.skill_name.clone(),
                    revision_id: Some(revision_id),
                },
            );
        }
    }
    for record in &owned {
        if suppressed_owned.contains(&record.resource_id) {
            continue;
        }
        if let Some(revision_id) = record.materialized_revision_id.clone() {
            local_fallbacks.insert(
                record.resource_id.clone(),
                PackApplySkillState {
                    resource_id: record.resource_id.clone(),
                    owner: record.owner.clone(),
                    skill_name: record.skill_name.clone(),
                    revision_id: Some(revision_id),
                },
            );
        }
    }
    let conflicted = state
        .conflicts
        .iter()
        .map(|conflict| conflict.resource_id.clone())
        .collect::<BTreeSet<_>>();
    let existing = context
        .db
        .pack_materialized_skills()
        .await
        .map_err(local_error)?;
    let existing_by_id = existing
        .iter()
        .map(|record| (record.resource_id.clone(), record.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut desired_records = Vec::new();
    let mut staged: BTreeMap<String, (DesiredSkillMaterialization, PathBuf)> = BTreeMap::new();
    for (resource_id, desired) in &state.desired {
        if suppressed.contains(resource_id) || conflicted.contains(resource_id) {
            continue;
        }
        let record = pack_record_from_desired(
            desired,
            existing_by_id
                .get(resource_id)
                .and_then(|record| record.harness_name.clone()),
        )?;
        let unchanged = existing_by_id.get(resource_id).is_some_and(|existing| {
            existing.materialized_revision_id == desired.revision_id
                && canonical_targets_revision(
                    context,
                    resource_id,
                    &desired.owner,
                    &desired.name,
                    &desired.revision_id,
                )
        });
        if !unchanged {
            let bytes = context
                .client
                .download_snapshot(&desired.snapshot)
                .await
                .map_err(client_error)?;
            let materialization = materialization_from_desired(desired)?;
            let generation =
                stage_skill_generation(&context.paths, &materialization, &bytes, new_operation()?)
                    .map_err(local_error)?;
            staged.insert(resource_id.clone(), (materialization, generation));
        }
        desired_records.push(record);
    }

    preserve_conflicted_existing(
        &mut desired_records,
        &conflicted,
        &suppressed,
        &existing_by_id,
    );

    let desired_ids = desired_records
        .iter()
        .map(|record| record.resource_id.clone())
        .collect::<BTreeSet<_>>();
    let mut touched = BTreeSet::new();
    touched.extend(staged.keys().cloned());
    for record in &existing {
        if !desired_ids.contains(&record.resource_id) {
            touched.insert(record.resource_id.clone());
        }
    }
    if touched.is_empty() {
        // Even if filesystem state did not change, persist current conflict state and source
        // ownership so status/reconciliation stay truthful.
        let operation_id = new_operation()?;
        context
            .db
            .create_pack_apply_journal(
                operation_id,
                PackApplyJournalPayload {
                    old_skills: Vec::new(),
                    new_skills: Vec::new(),
                },
                now_unix_ms(),
            )
            .await
            .map_err(local_error)?;
        context
            .db
            .commit_pack_apply(
                operation_id,
                desired_records,
                state.conflicts.clone(),
                now_unix_ms(),
            )
            .await
            .map_err(local_error)?;
        return Ok(PackApplyOutcome::default());
    }

    let operation_id = new_operation()?;
    let mut old_states = Vec::new();
    let mut new_states = Vec::new();
    for resource_id in &touched {
        let old = existing_by_id.get(resource_id);
        let new = desired_records
            .iter()
            .find(|record| &record.resource_id == resource_id);
        let fallback = local_fallbacks.get(resource_id);
        let identity = new
            .map(|record| (&record.owner, &record.skill_name))
            .or_else(|| fallback.map(|record| (&record.owner, &record.skill_name)))
            .or_else(|| old.map(|record| (&record.owner, &record.skill_name)))
            .ok_or_else(|| local("pack apply lost resource identity"))?;
        old_states.push(PackApplySkillState {
            resource_id: resource_id.clone(),
            owner: identity.0.clone(),
            skill_name: identity.1.clone(),
            revision_id: old.map(|record| record.materialized_revision_id.clone()),
        });
        new_states.push(PackApplySkillState {
            resource_id: resource_id.clone(),
            owner: identity.0.clone(),
            skill_name: identity.1.clone(),
            revision_id: new
                .map(|record| record.desired_revision_id.clone())
                .or_else(|| fallback.and_then(|record| record.revision_id.clone())),
        });
    }
    context
        .db
        .create_pack_apply_journal(
            operation_id,
            PackApplyJournalPayload {
                old_skills: old_states.clone(),
                new_skills: new_states,
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;

    let switch_result = (|| -> Result<(), RuntimeError> {
        for resource_id in &touched {
            if let Some((desired, generation)) = staged.get(resource_id) {
                switch_staged_skill_generation(&context.paths, desired, generation, operation_id)
                    .map_err(local_error)?;
            } else if !desired_ids.contains(resource_id) {
                let old = existing_by_id
                    .get(resource_id)
                    .ok_or_else(|| local("pack removal lost materialized state"))?;
                if let Some(fallback) = local_fallbacks.get(resource_id) {
                    restore_skill_generation(
                        &context.paths,
                        &fallback.resource_id,
                        &fallback.owner,
                        &fallback.skill_name,
                        fallback.revision_id.as_deref(),
                        operation_id,
                    )
                    .map_err(local_error)?;
                } else {
                    restore_skill_generation(
                        &context.paths,
                        &old.resource_id,
                        &old.owner,
                        &old.skill_name,
                        None,
                        operation_id,
                    )
                    .map_err(local_error)?;
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = switch_result {
        for old in &old_states {
            restore_skill_generation(
                &context.paths,
                &old.resource_id,
                &old.owner,
                &old.skill_name,
                old.revision_id.as_deref(),
                operation_id,
            )
            .map_err(local_error)?;
        }
        context
            .db
            .discard_pack_apply_journal(operation_id)
            .await
            .map_err(local_error)?;
        return Err(error);
    }
    context
        .db
        .commit_pack_apply(
            operation_id,
            desired_records,
            state.conflicts.clone(),
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    let materialized = staged.len();
    let removed = touched
        .iter()
        .filter(|resource_id| {
            !desired_ids.contains(*resource_id) && !local_fallbacks.contains_key(*resource_id)
        })
        .count();
    Ok(PackApplyOutcome {
        materialized,
        removed,
    })
}

fn pack_record_from_desired(
    desired: &SubscribedSkill,
    harness_name: Option<String>,
) -> Result<PackMaterializedSkillRecord, RuntimeError> {
    Ok(PackMaterializedSkillRecord {
        resource_id: desired.resource_id.clone(),
        locator: desired.locator.clone(),
        owner: desired.owner.clone(),
        skill_name: desired.name.clone(),
        resource_generation: i64::try_from(desired.generation)
            .map_err(|_| local("pack skill generation exceeds local database range"))?,
        desired_revision_id: desired.revision_id.clone(),
        desired_root_tree_id: desired.manifest.root_tree_id.clone(),
        harness_name,
        materialized_revision_id: desired.revision_id.clone(),
    })
}

fn materialization_from_desired(
    desired: &SubscribedSkill,
) -> Result<DesiredSkillMaterialization, RuntimeError> {
    Ok(DesiredSkillMaterialization {
        resource_id: ResourceId::from_str(&desired.resource_id)
            .map_err(|error| local(format!("invalid pack resource ID from registry: {error}")))?,
        revision_id: RevisionId::from_str(&desired.revision_id)
            .map_err(|error| local(format!("invalid pack revision ID from registry: {error}")))?,
        owner: desired.owner.clone(),
        skill_name: desired.name.clone(),
        manifest: desired.manifest.to_core().map_err(local)?,
    })
}

fn locator_parts(locator: &str) -> Result<(String, String), RuntimeError> {
    let parsed = locator
        .parse::<denju_core::ResourceLocator>()
        .map_err(|error| {
            local(format!(
                "invalid pack member locator from registry: {error}"
            ))
        })?;
    Ok((parsed.owner().to_owned(), parsed.name().to_owned()))
}

fn local(message: impl Into<String>) -> RuntimeError {
    RuntimeError::new(CliErrorCode::LocalState, message).recovery("denju doctor")
}

fn new_operation() -> Result<OperationId, RuntimeError> {
    OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))
}

fn preserve_conflicted_existing(
    desired_records: &mut Vec<PackMaterializedSkillRecord>,
    conflicted: &BTreeSet<String>,
    suppressed: &BTreeSet<String>,
    existing_by_id: &BTreeMap<String, PackMaterializedSkillRecord>,
) {
    // An equal-authority pack conflict must not silently choose a winner or tear down the
    // last known-good projection. Preserve the previously active exact revision while the
    // conflict remains recorded; a first-install conflict intentionally exposes nothing.
    for resource_id in conflicted {
        if suppressed.contains(resource_id) {
            continue;
        }
        if let Some(existing) = existing_by_id.get(resource_id) {
            desired_records.push(existing.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use denju_local::{
        LocalDatabase, LocalPaths, PackApplyJournalPayload, PackApplySkillState,
        create_native_directory_link, ensure_local_layout,
    };
    use tempfile::tempdir;

    use super::*;

    fn materialized_record(resource_id: &str, revision_id: &str) -> PackMaterializedSkillRecord {
        PackMaterializedSkillRecord {
            resource_id: resource_id.to_owned(),
            locator: "@alice/review".to_owned(),
            owner: "alice".to_owned(),
            skill_name: "review".to_owned(),
            resource_generation: 1,
            desired_revision_id: revision_id.to_owned(),
            desired_root_tree_id: "root-tree".to_owned(),
            harness_name: Some("review".to_owned()),
            materialized_revision_id: revision_id.to_owned(),
        }
    }

    #[test]
    fn pack_conflict_exposes_nothing_fresh_and_preserves_last_valid_projection() {
        let resource_id = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1".to_owned();
        let conflicted = BTreeSet::from([resource_id.clone()]);
        let suppressed = BTreeSet::new();
        let mut desired = Vec::new();
        preserve_conflicted_existing(&mut desired, &conflicted, &suppressed, &BTreeMap::new());
        assert!(desired.is_empty());

        let existing = materialized_record(&resource_id, &"11".repeat(32));
        let existing_by_id = BTreeMap::from([(resource_id, existing.clone())]);
        preserve_conflicted_existing(&mut desired, &conflicted, &suppressed, &existing_by_id);
        assert_eq!(desired, vec![existing]);
    }

    #[test]
    fn conflicted_resource_does_not_pause_unrelated_pack_member() {
        let conflicted_id = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1".to_owned();
        let unrelated_id = "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".to_owned();
        let old = materialized_record(&conflicted_id, &"11".repeat(32));
        let unrelated = PackMaterializedSkillRecord {
            locator: "@alice/write".to_owned(),
            skill_name: "write".to_owned(),
            harness_name: Some("write".to_owned()),
            ..materialized_record(&unrelated_id, &"33".repeat(32))
        };
        let mut desired = vec![unrelated.clone()];
        preserve_conflicted_existing(
            &mut desired,
            &BTreeSet::from([conflicted_id.clone()]),
            &BTreeSet::new(),
            &BTreeMap::from([(conflicted_id, old.clone())]),
        );
        desired.sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
        let mut expected = vec![old, unrelated];
        expected.sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
        assert_eq!(desired, expected);
    }

    #[tokio::test]
    async fn interrupted_parent_pack_switch_restores_every_old_pointer() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        ensure_local_layout(&paths).unwrap();
        let db = LocalDatabase::open(&paths.state_db).await.unwrap();
        let resource_id = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1";
        let old_revision = "11".repeat(32);
        let new_revision = "22".repeat(32);
        let resource_root = paths.generations.join(resource_id);
        let old_generation = resource_root.join(&old_revision);
        let new_generation = resource_root.join(&new_revision);
        fs::create_dir_all(&old_generation).unwrap();
        fs::create_dir_all(&new_generation).unwrap();
        fs::write(old_generation.join("marker"), b"old").unwrap();
        fs::write(new_generation.join("marker"), b"new").unwrap();
        let canonical = paths.skills.join("alice/review");
        fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        create_native_directory_link(&new_generation, &canonical).unwrap();

        let operation_id = new_operation().unwrap();
        db.create_pack_apply_journal(
            operation_id,
            PackApplyJournalPayload {
                old_skills: vec![PackApplySkillState {
                    resource_id: resource_id.to_owned(),
                    owner: "alice".to_owned(),
                    skill_name: "review".to_owned(),
                    revision_id: Some(old_revision.clone()),
                }],
                new_skills: vec![PackApplySkillState {
                    resource_id: resource_id.to_owned(),
                    owner: "alice".to_owned(),
                    skill_name: "review".to_owned(),
                    revision_id: Some(new_revision),
                }],
            },
            1,
        )
        .await
        .unwrap();

        recover_incomplete_apply_state(&paths, &db).await.unwrap();
        assert_eq!(
            fs::canonicalize(&canonical).unwrap(),
            fs::canonicalize(&old_generation).unwrap()
        );
        assert!(
            db.incomplete_pack_apply_journals()
                .await
                .unwrap()
                .is_empty()
        );
    }
}
