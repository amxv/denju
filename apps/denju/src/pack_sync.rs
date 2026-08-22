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
use denju_wire::{CliErrorCode, SubscribedSkill};
use uuid::Uuid;

use crate::{
    context::{InstalledContext, now_unix_ms},
    public::{client_error, local_error},
    setup::RuntimeError,
};

#[derive(Debug, Clone)]
pub(crate) struct PackCatalogState {
    desired: BTreeMap<String, SubscribedSkill>,
    conflicts: Vec<PackSourceConflictRecord>,
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
    let mut candidates: BTreeMap<String, Vec<(String, SubscribedSkill)>> = BTreeMap::new();
    for pack in catalog.packs {
        packs.push(PackSubscriptionRecord {
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
            if let Some(desired) = desired {
                candidates
                    .entry(member.resource_id)
                    .or_default()
                    .push((pack.pack.resource_id.clone(), desired));
            }
        }
    }
    context
        .db
        .replace_pack_catalog(packs, sources, now)
        .await
        .map_err(local_error)?;

    let mut desired = BTreeMap::new();
    let mut conflicts = Vec::new();
    for (resource_id, requirements) in candidates {
        let revisions = requirements
            .iter()
            .map(|(_, desired)| desired.revision_id.clone())
            .collect::<BTreeSet<_>>();
        if revisions.len() == 1 {
            if let Some((_, value)) = requirements.into_iter().next() {
                desired.insert(resource_id, value);
            }
        } else {
            conflicts.push(PackSourceConflictRecord {
                resource_id: resource_id.clone(),
                source_pack_ids: requirements.iter().map(|(pack, _)| pack.clone()).collect(),
                revision_ids: revisions.into_iter().collect(),
                message: format!(
                    "pack requirements disagree for resource {resource_id}; change or pin one pack source"
                ),
            });
        }
    }
    Ok(PackCatalogState { desired, conflicts })
}

pub(crate) async fn apply_pack_only_state(
    context: &InstalledContext,
    state: &PackCatalogState,
) -> Result<PackApplyOutcome, RuntimeError> {
    let direct_ids = context
        .db
        .subscriptions()
        .await
        .map_err(local_error)?
        .into_iter()
        .map(|record| record.resource_id)
        .collect::<BTreeSet<_>>();
    let owned_ids = context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .map(|record| record.resource_id)
        .collect::<BTreeSet<_>>();
    let suppressed = direct_ids
        .union(&owned_ids)
        .cloned()
        .collect::<BTreeSet<_>>();
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
        let unchanged = existing_by_id
            .get(resource_id)
            .is_some_and(|existing| existing.materialized_revision_id == desired.revision_id);
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
        if !desired_ids.contains(&record.resource_id) && !suppressed.contains(&record.resource_id) {
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
        let identity = new
            .map(|record| (&record.owner, &record.skill_name))
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
            revision_id: new.map(|record| record.desired_revision_id.clone()),
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
            } else if !desired_ids.contains(resource_id) && !suppressed.contains(resource_id) {
                let old = existing_by_id
                    .get(resource_id)
                    .ok_or_else(|| local("pack removal lost materialized state"))?;
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
        .filter(|resource_id| !desired_ids.contains(*resource_id))
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
