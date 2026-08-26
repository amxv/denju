use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use denju_core::{
    OperationId, OwnedSkillEntry, ResourceId, SkillManifest, build_skill_manifest,
    parse_skill_document, rewrite_skill_document_name,
};
use denju_sync::{ManagedSkillName, allocate_projection_names};
use thiserror::Error;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    DerivedProjectionStateRecord, JournalState, LocalDatabase, LocalDbError, LocalPaths,
    ManagedSkillRecord, OwnedSkillRecord, ResolvedHarnessRoots, SubscriptionRecord,
    WorkspaceWritebackJournalPayload, create_native_directory_link, detect_unmanaged_skills,
    harness::{is_managed_skill_target, managed_skill_storage_roots, unique_harness_roots},
    materialize::{
        MaterializationError, atomic_switch_directory_link, remove_owned_link, write_generation,
    },
    read_skill_source,
};

/// Recover collision-derived writeback independently of watcher delivery. Planned/staged
/// work can be retried from the still-visible derived view; verified/switched work resumes
/// the atomic canonical switch and durable baseline update idempotently.
pub async fn recover_workspace_writebacks(
    paths: &LocalPaths,
    db: &LocalDatabase,
) -> Result<(), ProjectionError> {
    for mut journal in db.workspace_writeback_journals().await? {
        let payload = &journal.payload;
        let stage = PathBuf::from(&payload.stage_dir);
        let generation = PathBuf::from(&payload.generation_dir);
        let canonical = PathBuf::from(&payload.canonical_path);
        if !stage.starts_with(&paths.generations)
            || !generation.starts_with(&paths.generations)
            || !canonical.starts_with(&paths.skills)
        {
            return Err(ProjectionError::Corrupt(
                "workspace writeback journal points outside Denju managed paths".to_owned(),
            ));
        }

        if journal.state == JournalState::Planned {
            let _ = fs::remove_dir_all(&stage);
            db.discard_workspace_writeback_journal(journal.operation_id)
                .await?;
            continue;
        }

        if journal.state == JournalState::Staged {
            if generation.is_dir() {
                verify_writeback_generation(
                    &generation,
                    &payload.skill_name,
                    &payload.target_root_tree_id,
                )?;
            } else if stage.is_dir() {
                verify_writeback_generation(
                    &stage,
                    &payload.skill_name,
                    &payload.target_root_tree_id,
                )?;
                fs::rename(&stage, &generation)?;
            } else {
                // The derived projection remains the user's source of truth, so an
                // interrupted pre-verification stage can be safely retried from scratch.
                db.discard_workspace_writeback_journal(journal.operation_id)
                    .await?;
                continue;
            }
            db.update_workspace_writeback_journal(
                journal.operation_id,
                JournalState::Staged,
                JournalState::Verified,
                now_unix_ms(),
            )
            .await?;
            journal.state = JournalState::Verified;
        }

        if journal.state == JournalState::Verified {
            verify_writeback_generation(
                &generation,
                &payload.skill_name,
                &payload.target_root_tree_id,
            )?;
            atomic_switch_directory_link(&generation, &canonical, journal.operation_id)?;
            db.update_workspace_writeback_journal(
                journal.operation_id,
                JournalState::Verified,
                JournalState::Switched,
                now_unix_ms(),
            )
            .await?;
            journal.state = JournalState::Switched;
        }

        if journal.state == JournalState::Switched {
            let actual = fs::canonicalize(&canonical)?;
            let expected = fs::canonicalize(&generation)?;
            if actual != expected {
                atomic_switch_directory_link(&generation, &canonical, journal.operation_id)?;
            }
            db.set_workspace_working_generation(
                payload.resource_id.clone(),
                generation.display().to_string(),
                now_unix_ms(),
            )
            .await?;
            db.save_derived_projection_state(
                DerivedProjectionStateRecord {
                    resource_id: payload.resource_id.clone(),
                    harness_name: payload.harness_name.clone(),
                    baseline_root_tree_id: payload.target_root_tree_id.clone(),
                },
                now_unix_ms(),
            )
            .await?;
            db.update_workspace_writeback_journal(
                journal.operation_id,
                JournalState::Switched,
                JournalState::Complete,
                now_unix_ms(),
            )
            .await?;
        }
    }
    Ok(())
}

/// Rebuild both harness views from local desired state. Old managed invocation links are
/// removed before new aliases are exposed, so a collision transition can temporarily hide
/// a skill but can never expose two resources under the same invocation name.
pub async fn reconcile_harness_projections(
    paths: &LocalPaths,
    db: &LocalDatabase,
    roots: &ResolvedHarnessRoots,
) -> Result<Vec<(String, String)>, ProjectionError> {
    let managed_records = db.managed_skills().await?;
    let owned_ids = db
        .owned_skills()
        .await?
        .into_iter()
        .map(|record| record.resource_id)
        .collect::<BTreeSet<_>>();
    let reserved = unmanaged_names(paths, roots)?;
    let mut managed = Vec::new();
    for record in &managed_records {
        if record.materialized_revision_id.is_none() {
            continue;
        }
        managed.push(ManagedSkillName {
            resource_id: ResourceId::from_str(&record.resource_id)
                .map_err(|error| ProjectionError::Corrupt(error.to_string()))?,
            owner: record.owner.clone(),
            skill_name: record.skill_name.clone(),
            previous_harness_name: record.harness_name.clone(),
        });
    }
    let assignments = allocate_projection_names(&managed, &reserved);

    // Remove only invocation paths whose desired name changed. Collision transitions are
    // fail-closed: every old canonical invocation is removed before any new aliases appear,
    // while unchanged links remain continuously visible across background sync cycles.
    for record in &managed_records {
        let desired = assignments
            .iter()
            .find(|assignment| assignment.resource_id.to_string() == record.resource_id)
            .map(|assignment| assignment.harness_name.as_str());
        if record
            .harness_name
            .as_deref()
            .is_some_and(|current| Some(current) != desired)
        {
            remove_stale_managed_projection(paths, roots, record.harness_name.as_deref())?;
        }
    }

    let mut projected = Vec::with_capacity(assignments.len());

    for assignment in assignments {
        let record = managed_records
            .iter()
            .find(|record| record.resource_id == assignment.resource_id.to_string())
            .ok_or_else(|| {
                ProjectionError::Corrupt("projection assignment lost its resource".to_owned())
            })?;
        let canonical = paths.skills.join(&record.owner).join(&record.skill_name);
        if !canonical.exists() {
            return Err(ProjectionError::Corrupt(format!(
                "canonical skill is missing for {}",
                record.locator
            )));
        }
        let (target, derived_created) = if assignment.derived {
            let (target, created) =
                derived_view(paths, record, &canonical, &assignment.harness_name)?;
            (target, created)
        } else {
            (canonical, false)
        };

        for root in unique_harness_roots(roots) {
            create_projection_link(paths, &target, &root.join(&assignment.harness_name))?;
        }
        if record.harness_name.as_deref() != Some(assignment.harness_name.as_str()) {
            db.set_managed_harness_name(
                record.resource_id.clone(),
                assignment.harness_name.clone(),
                now_unix_ms(),
            )
            .await?;
        }
        if assignment.derived && owned_ids.contains(&record.resource_id) {
            let existing = db
                .derived_projection_state(record.resource_id.clone())
                .await?;
            if derived_created
                || existing
                    .as_ref()
                    .is_none_or(|state| state.harness_name != assignment.harness_name)
            {
                let canonical_root =
                    fs::canonicalize(paths.skills.join(&record.owner).join(&record.skill_name))?;
                let canonical_manifest = skill_manifest(&canonical_root, &record.skill_name)?;
                let derived_manifest = canonicalized_derived_manifest(
                    &target,
                    &assignment.harness_name,
                    &record.skill_name,
                )?;
                if canonical_manifest == derived_manifest {
                    db.save_derived_projection_state(
                        DerivedProjectionStateRecord {
                            resource_id: record.resource_id.clone(),
                            harness_name: assignment.harness_name.clone(),
                            baseline_root_tree_id: canonical_manifest.root_tree().to_string(),
                        },
                        now_unix_ms(),
                    )
                    .await?;
                }
            }
        }
        projected.push((record.locator.clone(), assignment.harness_name));
    }
    Ok(projected)
}

pub fn remove_subscription_projection(
    paths: &LocalPaths,
    roots: &ResolvedHarnessRoots,
    record: &SubscriptionRecord,
) -> Result<(), ProjectionError> {
    remove_managed_projection(paths, roots, &record.owner, record.harness_name.as_deref())
}

pub fn remove_managed_skill_projection(
    paths: &LocalPaths,
    roots: &ResolvedHarnessRoots,
    record: &ManagedSkillRecord,
) -> Result<(), ProjectionError> {
    remove_managed_projection(paths, roots, &record.owner, record.harness_name.as_deref())
}

fn remove_managed_projection(
    paths: &LocalPaths,
    roots: &ResolvedHarnessRoots,
    _owner: &str,
    harness_name: Option<&str>,
) -> Result<(), ProjectionError> {
    let Some(harness_name) = harness_name else {
        return Ok(());
    };
    for root in unique_harness_roots(roots) {
        remove_managed_projection_link(paths, &root.join(harness_name))?;
    }
    Ok(())
}

fn remove_stale_managed_projection(
    paths: &LocalPaths,
    roots: &ResolvedHarnessRoots,
    harness_name: Option<&str>,
) -> Result<(), ProjectionError> {
    let Some(harness_name) = harness_name else {
        return Ok(());
    };
    for root in unique_harness_roots(roots) {
        let link = root.join(harness_name);
        match remove_managed_projection_link(paths, &link) {
            Ok(()) | Err(ProjectionError::RefuseOverwrite(_)) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn unmanaged_names(
    paths: &LocalPaths,
    roots: &ResolvedHarnessRoots,
) -> Result<BTreeSet<String>, ProjectionError> {
    let managed_roots = managed_skill_storage_roots(paths);
    let mut names = BTreeSet::new();
    for skill_dir in detect_unmanaged_skills(paths, roots)? {
        if let Some(name) = skill_dir.file_name().and_then(|name| name.to_str()) {
            names.insert(name.to_owned());
        }
    }
    for root in unique_harness_roots(roots) {
        if !root.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let path = entry.path();
            if is_managed_skill_target(&managed_roots, &path) {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                names.insert(name.to_owned());
            }
        }
    }
    Ok(names)
}

fn derived_view(
    paths: &LocalPaths,
    record: &ManagedSkillRecord,
    canonical: &Path,
    harness_name: &str,
) -> Result<(PathBuf, bool), ProjectionError> {
    let revision = record.materialized_revision_id.as_deref().ok_or_else(|| {
        ProjectionError::Corrupt("derived projection has no materialized revision".to_owned())
    })?;
    let root = paths
        .derived
        .join(&record.resource_id)
        .join(format!("{revision}-{harness_name}"));
    if root.is_dir() {
        let skill_md = fs::read(root.join("SKILL.md"))?;
        parse_skill_document(harness_name, &skill_md)
            .map_err(|error| ProjectionError::Corrupt(error.to_string()))?;
        return Ok((root, false));
    }

    let source = fs::canonicalize(canonical)?;
    let parent = root
        .parent()
        .ok_or_else(|| ProjectionError::Corrupt("derived root has no parent".to_owned()))?;
    fs::create_dir_all(parent)?;
    let stage = parent.join(format!(".stage-{}", Uuid::now_v7()));
    copy_derived_tree(&source, &stage, &record.skill_name, harness_name)?;
    fs::rename(&stage, &root)?;
    Ok((root, true))
}

/// Reconcile one owned collision-derived view before scanning the canonical workspace.
/// The persisted semantic root records which side last agreed, so polling never guesses which
/// copy won when both the canonical and the derived view are editable.
pub async fn reconcile_owned_derived_projection(
    paths: &LocalPaths,
    db: &LocalDatabase,
    record: &OwnedSkillRecord,
) -> Result<bool, ProjectionError> {
    let Some(harness_name) = record.harness_name.as_deref() else {
        return Ok(false);
    };
    if harness_name == record.skill_name {
        return Ok(false);
    }
    let Some(materialized_revision) = record.materialized_revision_id.as_deref() else {
        return Ok(false);
    };
    let derived = paths
        .derived
        .join(&record.resource_id)
        .join(format!("{materialized_revision}-{harness_name}"));
    if !derived.is_dir() {
        return Ok(false);
    }
    let canonical = paths.skills.join(&record.owner).join(&record.skill_name);
    let canonical_root = fs::canonicalize(&canonical)?;
    let canonical_manifest = skill_manifest(&canonical_root, &record.skill_name)?;
    let (derived_manifest, derived_entries) =
        canonicalized_derived(&derived, harness_name, &record.skill_name)?;
    let canonical_root_id = canonical_manifest.root_tree().to_string();
    let derived_root_id = derived_manifest.root_tree().to_string();
    let baseline = db
        .derived_projection_state(record.resource_id.clone())
        .await?;

    if canonical_root_id == derived_root_id {
        db.save_derived_projection_state(
            DerivedProjectionStateRecord {
                resource_id: record.resource_id.clone(),
                harness_name: harness_name.to_owned(),
                baseline_root_tree_id: canonical_root_id,
            },
            now_unix_ms(),
        )
        .await?;
        return Ok(false);
    }

    let Some(baseline) = baseline.filter(|state| state.harness_name == harness_name) else {
        return Err(ProjectionError::DivergedDerivedEdit(record.locator.clone()));
    };
    let canonical_changed = canonical_root_id != baseline.baseline_root_tree_id;
    let derived_changed = derived_root_id != baseline.baseline_root_tree_id;
    match (canonical_changed, derived_changed) {
        (false, true) => {
            let operation = OperationId::from_uuid(Uuid::now_v7())
                .map_err(|error| ProjectionError::Corrupt(error.to_string()))?;
            let resource_root = paths.generations.join(&record.resource_id);
            fs::create_dir_all(&resource_root)?;
            let stage = resource_root.join(format!(".writeback-{operation}"));
            let generation = resource_root.join(format!("workspace-{operation}"));
            let payload = WorkspaceWritebackJournalPayload {
                resource_id: record.resource_id.clone(),
                skill_name: record.skill_name.clone(),
                harness_name: harness_name.to_owned(),
                target_root_tree_id: derived_root_id.clone(),
                stage_dir: stage.display().to_string(),
                generation_dir: generation.display().to_string(),
                canonical_path: canonical.display().to_string(),
            };
            db.create_workspace_writeback_journal(operation, payload, now_unix_ms())
                .await?;
            write_generation(paths, &stage, &derived_entries)?;
            db.update_workspace_writeback_journal(
                operation,
                JournalState::Planned,
                JournalState::Staged,
                now_unix_ms(),
            )
            .await?;
            verify_writeback_generation(&stage, &record.skill_name, &derived_root_id)?;
            fs::rename(&stage, &generation)?;
            db.update_workspace_writeback_journal(
                operation,
                JournalState::Staged,
                JournalState::Verified,
                now_unix_ms(),
            )
            .await?;
            atomic_switch_directory_link(&generation, &canonical, operation)?;
            db.update_workspace_writeback_journal(
                operation,
                JournalState::Verified,
                JournalState::Switched,
                now_unix_ms(),
            )
            .await?;
            db.set_workspace_working_generation(
                record.resource_id.clone(),
                generation.display().to_string(),
                now_unix_ms(),
            )
            .await?;
            db.save_derived_projection_state(
                DerivedProjectionStateRecord {
                    resource_id: record.resource_id.clone(),
                    harness_name: harness_name.to_owned(),
                    baseline_root_tree_id: derived_root_id,
                },
                now_unix_ms(),
            )
            .await?;
            db.update_workspace_writeback_journal(
                operation,
                JournalState::Switched,
                JournalState::Complete,
                now_unix_ms(),
            )
            .await?;
            Ok(true)
        }
        (true, false) => {
            fs::remove_dir_all(&derived)?;
            copy_derived_tree(&canonical_root, &derived, &record.skill_name, harness_name)?;
            db.save_derived_projection_state(
                DerivedProjectionStateRecord {
                    resource_id: record.resource_id.clone(),
                    harness_name: harness_name.to_owned(),
                    baseline_root_tree_id: canonical_root_id,
                },
                now_unix_ms(),
            )
            .await?;
            Ok(false)
        }
        (true, true) => Err(ProjectionError::DivergedDerivedEdit(record.locator.clone())),
        (false, false) => Ok(false),
    }
}

fn skill_manifest(root: &Path, skill_name: &str) -> Result<SkillManifest, ProjectionError> {
    let entries = read_skill_source(root)?;
    build_skill_manifest(skill_name, &entries)
        .map_err(|error| ProjectionError::Corrupt(error.to_string()))
}

fn verify_writeback_generation(
    root: &Path,
    skill_name: &str,
    expected_root_tree_id: &str,
) -> Result<(), ProjectionError> {
    let manifest = skill_manifest(root, skill_name)?;
    if manifest.root_tree().to_string() != expected_root_tree_id {
        return Err(ProjectionError::Corrupt(
            "workspace writeback generation failed semantic verification".to_owned(),
        ));
    }
    Ok(())
}

fn canonicalized_derived_manifest(
    root: &Path,
    harness_name: &str,
    skill_name: &str,
) -> Result<SkillManifest, ProjectionError> {
    canonicalized_derived(root, harness_name, skill_name).map(|value| value.0)
}

fn canonicalized_derived(
    root: &Path,
    harness_name: &str,
    skill_name: &str,
) -> Result<(SkillManifest, Vec<OwnedSkillEntry>), ProjectionError> {
    let mut entries = read_skill_source(root)?;
    let mut found_skill_md = false;
    for entry in &mut entries {
        if let OwnedSkillEntry::File { path, bytes, .. } = entry
            && path == "SKILL.md"
        {
            *bytes = rewrite_skill_document_name(harness_name, bytes, skill_name)
                .map_err(|error| ProjectionError::Corrupt(error.to_string()))?;
            found_skill_md = true;
        }
    }
    if !found_skill_md {
        return Err(ProjectionError::Corrupt(
            "derived projection is missing SKILL.md".to_owned(),
        ));
    }
    let manifest = build_skill_manifest(skill_name, &entries)
        .map_err(|error| ProjectionError::Corrupt(error.to_string()))?;
    Ok((manifest, entries))
}

fn copy_derived_tree(
    source: &Path,
    destination: &Path,
    canonical_name: &str,
    harness_name: &str,
) -> Result<(), ProjectionError> {
    fs::create_dir_all(destination)?;
    for item in WalkDir::new(source).follow_links(false).min_depth(1) {
        let item = item.map_err(ProjectionError::Walk)?;
        let relative = item
            .path()
            .strip_prefix(source)
            .map_err(|error| ProjectionError::Corrupt(error.to_string()))?;
        let target = destination.join(relative);
        if item.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if item.file_type().is_symlink() {
            let link_target = fs::read_link(item.path())?;
            create_symlink(&link_target, &target)?;
        } else if item.file_type().is_file() {
            if relative == Path::new("SKILL.md") {
                let canonical = fs::read(item.path())?;
                let rewritten =
                    rewrite_skill_document_name(canonical_name, &canonical, harness_name)
                        .map_err(|error| ProjectionError::Corrupt(error.to_string()))?;
                fs::write(&target, rewritten)?;
            } else {
                // Collision-derived views must remain independently writable until Denju
                // validates and journals writeback. A hard link would let an edit through
                // the derived view mutate the canonical working generation before Denju can
                // determine which side changed.
                fs::copy(item.path(), &target)?;
            }
        } else {
            return Err(ProjectionError::Corrupt(format!(
                "unsupported derived entry {}",
                item.path().display()
            )));
        }
    }
    let skill_md = fs::read(destination.join("SKILL.md"))?;
    parse_skill_document(harness_name, &skill_md)
        .map_err(|error| ProjectionError::Corrupt(error.to_string()))?;
    Ok(())
}

fn create_projection_link(
    paths: &LocalPaths,
    target: &Path,
    link: &Path,
) -> Result<(), ProjectionError> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::symlink_metadata(link).is_ok() {
        let actual = fs::canonicalize(link).ok();
        let expected = fs::canonicalize(target)?;
        if actual.as_ref() == Some(&expected) {
            return Ok(());
        }
        remove_managed_projection_link(paths, link)?;
    }
    create_native_directory_link(target, link)?;
    Ok(())
}

fn remove_managed_projection_link(paths: &LocalPaths, link: &Path) -> Result<(), ProjectionError> {
    let Ok(metadata) = fs::symlink_metadata(link) else {
        return Ok(());
    };
    #[cfg(unix)]
    if !metadata.file_type().is_symlink() {
        return Err(ProjectionError::RefuseOverwrite(link.to_owned()));
    }
    #[cfg(windows)]
    if !metadata.file_type().is_symlink() && !metadata.is_dir() {
        return Err(ProjectionError::RefuseOverwrite(link.to_owned()));
    }
    let target =
        fs::canonicalize(link).map_err(|_| ProjectionError::RefuseOverwrite(link.to_owned()))?;
    let managed_root = fs::canonicalize(&paths.root).unwrap_or_else(|_| paths.root.clone());
    if !target.starts_with(&managed_root) {
        return Err(ProjectionError::RefuseOverwrite(link.to_owned()));
    }
    remove_owned_link(link)?;
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

fn now_unix_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error("projection filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("projection database error: {0}")]
    Database(#[from] LocalDbError),
    #[error("projection materialization error: {0}")]
    Materialization(#[from] MaterializationError),
    #[error("failed to scan harness skills: {0}")]
    Harness(#[from] crate::HarnessError),
    #[error("failed to read managed skill content: {0}")]
    Source(#[from] crate::SourceError),
    #[error("failed to copy derived view: {0}")]
    Walk(walkdir::Error),
    #[error("projection state is corrupt: {0}")]
    Corrupt(String),
    #[error("refusing to overwrite unmanaged projection path {path}", path = .0.display())]
    RefuseOverwrite(PathBuf),
    #[error("canonical and collision-derived views both changed for {0}")]
    DivergedDerivedEdit(String),
}

#[cfg(test)]
mod tests;
