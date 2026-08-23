use std::{fs, path::PathBuf, str::FromStr};

use denju_core::{
    OperationId, PortableEntry, PortableEntryKind, ResourceId, RevisionId, SkillManifest,
    validate_portable_tree,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    DesiredSkillMaterialization, JournalState, LocalDatabase, LocalDbError, LocalPaths,
    ManagedSkillRecord, OwnedSkillRecord, ResolvedHarnessRoots, create_native_directory_link,
    materialize::stage_skill_generation, reconcile_harness_projections, remove_canonical_skill,
    remove_managed_skill_projection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDesiredKind {
    Subscription,
    Owned,
    Pack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum LocalLifecyclePayload {
    Remove {
        resource_id: String,
        owner: String,
        skill_name: String,
        harness_name: Option<String>,
        desired_kind: ManagedDesiredKind,
    },
    Rename {
        resource_id: String,
        old_owner: String,
        old_name: String,
        old_harness_name: Option<String>,
        new_owner: String,
        new_name: String,
        new_locator: String,
        remote_resource_generation: i64,
        remote_workspace_generation: i64,
        remote_revision_id: String,
        remote_root_tree_id: String,
        working_generation_path: String,
        preserve_working: bool,
    },
}

#[derive(Debug, Clone)]
struct LocalLifecycleJournal {
    operation_id: OperationId,
    state: JournalState,
    payload: LocalLifecyclePayload,
}

#[derive(Debug, Clone)]
pub struct RegistryRenameState {
    pub resource_id: String,
    pub owner: String,
    pub name: String,
    pub locator: String,
    pub resource_generation: i64,
    pub workspace_generation: i64,
    pub revision_id: String,
    pub root_tree_id: String,
}

#[derive(Debug, Error)]
pub enum LocalLifecycleError {
    #[error(transparent)]
    Database(#[from] LocalDbError),
    #[error("local lifecycle filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("local lifecycle projection error: {0}")]
    Projection(String),
    #[error("local lifecycle materialization error: {0}")]
    Materialization(String),
    #[error("corrupt local lifecycle state: {0}")]
    Corrupt(String),
}

pub async fn journaled_remove_managed_skill(
    paths: &LocalPaths,
    db: &LocalDatabase,
    roots: &ResolvedHarnessRoots,
    record: &ManagedSkillRecord,
    desired_kind: ManagedDesiredKind,
) -> Result<(), LocalLifecycleError> {
    let operation_id = OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| LocalLifecycleError::Corrupt(error.to_string()))?;
    db.create_local_lifecycle_journal(
        operation_id,
        LocalLifecyclePayload::Remove {
            resource_id: record.resource_id.clone(),
            owner: record.owner.clone(),
            skill_name: record.skill_name.clone(),
            harness_name: record.harness_name.clone(),
            desired_kind,
        },
    )
    .await?;
    recover_local_lifecycle(paths, db, roots).await
}

pub fn preserve_quarantined_managed_skill(
    paths: &LocalPaths,
    record: &ManagedSkillRecord,
) -> Result<PathBuf, LocalLifecycleError> {
    preserve_quarantined_managed_skill_for(paths, record, &record.resource_id)
}

fn preserve_quarantined_managed_skill_for(
    paths: &LocalPaths,
    record: &ManagedSkillRecord,
    quarantine_resource_id: &str,
) -> Result<PathBuf, LocalLifecycleError> {
    ResourceId::from_str(quarantine_resource_id).map_err(|error| {
        LocalLifecycleError::Corrupt(format!("invalid quarantine resource id: {error}"))
    })?;
    let revision_id = record.materialized_revision_id.as_deref().ok_or_else(|| {
        LocalLifecycleError::Corrupt(format!(
            "{} has no materialized revision to quarantine",
            record.locator
        ))
    })?;
    let source = paths
        .generations
        .join(&record.resource_id)
        .join(revision_id);
    let destination = paths
        .quarantine
        .join(quarantine_resource_id)
        .join(revision_id);
    if destination.exists() {
        let destination = fs::canonicalize(&destination)?;
        let quarantine_root = fs::canonicalize(&paths.quarantine)?;
        if !destination.starts_with(&quarantine_root) {
            return Err(LocalLifecycleError::Corrupt(
                "quarantined generation escaped the Denju quarantine root".to_owned(),
            ));
        }
        return Ok(destination);
    }
    let source = fs::canonicalize(&source).map_err(|error| {
        LocalLifecycleError::Corrupt(format!(
            "{} materialized generation is unavailable for quarantine: {error}",
            record.locator
        ))
    })?;
    let generations_root = fs::canonicalize(&paths.generations)?;
    if !source.starts_with(&generations_root) {
        return Err(LocalLifecycleError::Corrupt(
            "materialized generation escaped the Denju generations root".to_owned(),
        ));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(LocalLifecycleError::Corrupt(format!(
            "quarantine destination already exists unexpectedly: {}",
            destination.display()
        )));
    }
    let stage = destination
        .parent()
        .expect("quarantine revision has a resource parent")
        .join(format!(".stage-{}", Uuid::now_v7()));
    copy_quarantine_tree(&source, &stage)?;
    fs::rename(&stage, &destination)?;
    Ok(fs::canonicalize(destination)?)
}

fn copy_quarantine_tree(
    source: &PathBuf,
    destination: &PathBuf,
) -> Result<(), LocalLifecycleError> {
    validate_quarantine_source(source)?;
    fs::create_dir(destination)?;
    let result = (|| {
        for entry in WalkDir::new(source).follow_links(false).min_depth(1) {
            let entry = entry.map_err(|error| LocalLifecycleError::Corrupt(error.to_string()))?;
            let relative = entry.path().strip_prefix(source).map_err(|error| {
                LocalLifecycleError::Corrupt(format!("quarantine copy escaped source: {error}"))
            })?;
            let target = destination.join(relative);
            if entry.file_type().is_dir() {
                fs::create_dir(&target)?;
                continue;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            if entry.file_type().is_file() {
                fs::copy(entry.path(), &target)?;
                fs::set_permissions(&target, fs::metadata(entry.path())?.permissions())?;
                continue;
            }
            if entry.file_type().is_symlink() {
                let link_target = fs::read_link(entry.path())?;
                if link_target.is_absolute() {
                    return Err(LocalLifecycleError::Corrupt(
                        "quarantined generation contains an absolute symlink".to_owned(),
                    ));
                }
                create_quarantine_symlink(&link_target, &target)?;
                continue;
            }
            return Err(LocalLifecycleError::Corrupt(format!(
                "unsupported entry in quarantined generation: {}",
                entry.path().display()
            )));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    result
}

fn validate_quarantine_source(source: &std::path::Path) -> Result<(), LocalLifecycleError> {
    let mut portable = Vec::new();
    for entry in WalkDir::new(source).follow_links(false).min_depth(1) {
        let entry = entry.map_err(|error| LocalLifecycleError::Corrupt(error.to_string()))?;
        let relative = entry.path().strip_prefix(source).map_err(|error| {
            LocalLifecycleError::Corrupt(format!("quarantine validation escaped source: {error}"))
        })?;
        let path = relative
            .components()
            .map(|component| component.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                LocalLifecycleError::Corrupt(
                    "quarantined generation contains a non-UTF-8 path".to_owned(),
                )
            })?
            .join("/");
        let kind = if entry.file_type().is_dir() {
            PortableEntryKind::Directory
        } else if entry.file_type().is_file() {
            PortableEntryKind::File { executable: false }
        } else if entry.file_type().is_symlink() {
            let target = fs::read_link(entry.path())?;
            let target = target.to_str().ok_or_else(|| {
                LocalLifecycleError::Corrupt(
                    "quarantined generation contains a non-UTF-8 symlink target".to_owned(),
                )
            })?;
            PortableEntryKind::Symlink {
                target: target.to_owned(),
            }
        } else {
            return Err(LocalLifecycleError::Corrupt(format!(
                "unsupported entry in quarantined generation: {}",
                entry.path().display()
            )));
        };
        portable.push(PortableEntry::new(&path, kind).map_err(|error| {
            LocalLifecycleError::Corrupt(format!("invalid quarantined path {path}: {error}"))
        })?);
    }
    validate_portable_tree(portable).map_err(|error| {
        LocalLifecycleError::Corrupt(format!("unsafe quarantined generation: {error}"))
    })?;
    Ok(())
}

#[cfg(unix)]
fn create_quarantine_symlink(
    target: &std::path::Path,
    link: &std::path::Path,
) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_quarantine_symlink(
    target: &std::path::Path,
    link: &std::path::Path,
) -> std::io::Result<()> {
    let resolved = link
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(target);
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

pub async fn apply_registry_rename(
    paths: &LocalPaths,
    db: &LocalDatabase,
    roots: &ResolvedHarnessRoots,
    old: &ManagedSkillRecord,
    remote: RegistryRenameState,
    preserve_working: bool,
    authoritative_snapshot: Option<(&SkillManifest, &[u8])>,
) -> Result<(), LocalLifecycleError> {
    let resource_id = ResourceId::from_str(&remote.resource_id)
        .map_err(|error| LocalLifecycleError::Corrupt(error.to_string()))?;
    if resource_id.to_string() != old.resource_id {
        return Err(LocalLifecycleError::Corrupt(
            "rename response changed the immutable resource ID".to_owned(),
        ));
    }
    let operation_id = OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| LocalLifecycleError::Corrupt(error.to_string()))?;
    let old_canonical = paths.skills.join(&old.owner).join(&old.skill_name);
    let working_generation = if preserve_working {
        if authoritative_snapshot.is_some() {
            return Err(LocalLifecycleError::Corrupt(
                "pending rename cannot replace preserved working bytes".to_owned(),
            ));
        }
        fs::canonicalize(&old_canonical)?
    } else {
        let (manifest, snapshot) = authoritative_snapshot.ok_or_else(|| {
            LocalLifecycleError::Corrupt(
                "authoritative rename snapshot is required for a clean workspace".to_owned(),
            )
        })?;
        let revision_id = RevisionId::from_str(&remote.revision_id)
            .map_err(|error| LocalLifecycleError::Corrupt(error.to_string()))?;
        let desired = DesiredSkillMaterialization {
            resource_id,
            owner: remote.owner.clone(),
            skill_name: remote.name.clone(),
            revision_id,
            manifest: manifest.clone(),
        };
        stage_skill_generation(paths, &desired, snapshot, operation_id)
            .map_err(|error| LocalLifecycleError::Materialization(error.to_string()))?
    };
    db.create_local_lifecycle_journal(
        operation_id,
        LocalLifecyclePayload::Rename {
            resource_id: remote.resource_id,
            old_owner: old.owner.clone(),
            old_name: old.skill_name.clone(),
            old_harness_name: old.harness_name.clone(),
            new_owner: remote.owner,
            new_name: remote.name,
            new_locator: remote.locator,
            remote_resource_generation: remote.resource_generation,
            remote_workspace_generation: remote.workspace_generation,
            remote_revision_id: remote.revision_id,
            remote_root_tree_id: remote.root_tree_id,
            working_generation_path: working_generation.display().to_string(),
            preserve_working,
        },
    )
    .await?;
    recover_local_lifecycle(paths, db, roots).await
}

pub async fn recover_local_lifecycle(
    paths: &LocalPaths,
    db: &LocalDatabase,
    roots: &ResolvedHarnessRoots,
) -> Result<(), LocalLifecycleError> {
    for journal in db.local_lifecycle_journals().await? {
        match &journal.payload {
            LocalLifecyclePayload::Remove {
                resource_id,
                owner,
                skill_name,
                harness_name,
                desired_kind,
            } => {
                if journal.state == JournalState::Planned {
                    let record = ManagedSkillRecord {
                        resource_id: resource_id.clone(),
                        locator: format!("@{owner}/{skill_name}"),
                        owner: owner.clone(),
                        skill_name: skill_name.clone(),
                        harness_name: harness_name.clone(),
                        materialized_revision_id: None,
                    };
                    remove_managed_skill_projection(paths, roots, &record)
                        .map_err(|error| LocalLifecycleError::Projection(error.to_string()))?;
                    db.advance_local_lifecycle(journal.operation_id, JournalState::Planned)
                        .await?;
                }
                let state = db.local_lifecycle_state(journal.operation_id).await?;
                if state == JournalState::Staged {
                    remove_canonical_skill(paths, owner, skill_name)
                        .map_err(|error| LocalLifecycleError::Materialization(error.to_string()))?;
                    db.advance_local_lifecycle(journal.operation_id, JournalState::Staged)
                        .await?;
                }
                let state = db.local_lifecycle_state(journal.operation_id).await?;
                if state == JournalState::Verified {
                    match desired_kind {
                        ManagedDesiredKind::Subscription => {
                            db.remove_subscription(resource_id.clone()).await?
                        }
                        ManagedDesiredKind::Owned => {
                            db.remove_owned_skill(resource_id.clone()).await?
                        }
                        ManagedDesiredKind::Pack => {
                            db.remove_pack_materialized_record(resource_id.clone())
                                .await?
                        }
                    }
                    db.advance_local_lifecycle(journal.operation_id, JournalState::Verified)
                        .await?;
                }
                let state = db.local_lifecycle_state(journal.operation_id).await?;
                if state == JournalState::Switched {
                    db.advance_local_lifecycle(journal.operation_id, JournalState::Switched)
                        .await?;
                }
            }
            LocalLifecyclePayload::Rename {
                resource_id,
                old_owner,
                old_name,
                old_harness_name,
                new_owner,
                new_name,
                new_locator,
                remote_resource_generation,
                remote_workspace_generation,
                remote_revision_id,
                remote_root_tree_id,
                working_generation_path,
                preserve_working,
            } => {
                if journal.state == JournalState::Planned {
                    let new_canonical = paths.skills.join(new_owner).join(new_name);
                    let working = PathBuf::from(working_generation_path);
                    ensure_canonical_link(&working, &new_canonical, journal.operation_id)?;
                    db.advance_local_lifecycle(journal.operation_id, JournalState::Planned)
                        .await?;
                }
                let state = db.local_lifecycle_state(journal.operation_id).await?;
                if state == JournalState::Staged {
                    let existing = db
                        .owned_skills()
                        .await?
                        .into_iter()
                        .find(|record| record.resource_id == *resource_id)
                        .ok_or_else(|| {
                            LocalLifecycleError::Corrupt(format!(
                                "rename lost owned resource {resource_id}"
                            ))
                        })?;
                    db.upsert_owned_skill_desired(
                        OwnedSkillRecord {
                            resource_id: resource_id.clone(),
                            locator: new_locator.clone(),
                            owner: new_owner.clone(),
                            skill_name: new_name.clone(),
                            resource_generation: *remote_resource_generation,
                            workspace_generation: *remote_workspace_generation,
                            desired_revision_id: remote_revision_id.clone(),
                            harness_name: existing.harness_name,
                            materialized_revision_id: existing.materialized_revision_id,
                        },
                        now_unix_ms(),
                    )
                    .await?;
                    db.adopt_registry_rename_baseline(
                        resource_id.clone(),
                        *remote_workspace_generation,
                        remote_revision_id.clone(),
                        remote_root_tree_id.clone(),
                        working_generation_path.clone(),
                        now_unix_ms(),
                    )
                    .await?;
                    if !preserve_working {
                        db.mark_skill_materialized(
                            resource_id.clone(),
                            remote_revision_id.clone(),
                            now_unix_ms(),
                        )
                        .await?;
                    }
                    db.advance_local_lifecycle(journal.operation_id, JournalState::Staged)
                        .await?;
                }
                let state = db.local_lifecycle_state(journal.operation_id).await?;
                if state == JournalState::Verified {
                    let old_record = ManagedSkillRecord {
                        resource_id: resource_id.clone(),
                        locator: format!("@{old_owner}/{old_name}"),
                        owner: old_owner.clone(),
                        skill_name: old_name.clone(),
                        harness_name: old_harness_name.clone(),
                        materialized_revision_id: None,
                    };
                    remove_managed_skill_projection(paths, roots, &old_record)
                        .map_err(|error| LocalLifecycleError::Projection(error.to_string()))?;
                    remove_canonical_skill(paths, old_owner, old_name)
                        .map_err(|error| LocalLifecycleError::Materialization(error.to_string()))?;
                    db.advance_local_lifecycle(journal.operation_id, JournalState::Verified)
                        .await?;
                }
                let state = db.local_lifecycle_state(journal.operation_id).await?;
                if state == JournalState::Switched {
                    reconcile_harness_projections(paths, db, roots)
                        .await
                        .map_err(|error| LocalLifecycleError::Projection(error.to_string()))?;
                    db.advance_local_lifecycle(journal.operation_id, JournalState::Switched)
                        .await?;
                }
            }
        }
    }
    Ok(())
}

fn ensure_canonical_link(
    target: &PathBuf,
    link: &PathBuf,
    operation_id: OperationId,
) -> Result<(), LocalLifecycleError> {
    if let Ok(existing) = fs::canonicalize(link) {
        if existing == fs::canonicalize(target)? {
            return Ok(());
        }
        return Err(LocalLifecycleError::Corrupt(format!(
            "rename destination {} already points elsewhere",
            link.display()
        )));
    }
    let parent = link.parent().ok_or_else(|| {
        LocalLifecycleError::Corrupt(format!(
            "rename destination has no parent: {}",
            link.display()
        ))
    })?;
    fs::create_dir_all(parent)?;
    let stage = parent.join(format!(".denju-rename-{operation_id}"));
    let _ = fs::remove_file(&stage);
    let _ = fs::remove_dir(&stage);
    create_native_directory_link(target, &stage)?;
    fs::rename(&stage, link)?;
    Ok(())
}

impl LocalDatabase {
    async fn create_local_lifecycle_journal(
        &self,
        operation_id: OperationId,
        payload: LocalLifecyclePayload,
    ) -> Result<(), LocalDbError> {
        let payload = serde_json::to_string(&payload)?;
        let now = now_unix_ms();
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO operation_journal \
                 (operation_id,kind,state,payload_json,created_at_unix_ms,updated_at_unix_ms) \
                 VALUES (?1,'lifecycle_local','planned',?2,?3,?3)",
                rusqlite::params![operation_id.to_string(), payload, now],
            )?;
            Ok(())
        })
        .await
    }

    async fn local_lifecycle_journals(&self) -> Result<Vec<LocalLifecycleJournal>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT operation_id,state,payload_json FROM operation_journal \
                 WHERE kind='lifecycle_local' AND state<>'complete' ORDER BY created_at_unix_ms",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            let mut result = Vec::new();
            for row in rows {
                let (operation_id, state, payload) = row?;
                result.push(LocalLifecycleJournal {
                    operation_id: operation_id.parse().map_err(|error: denju_core::IdError| {
                        LocalDbError::Corrupt(error.to_string())
                    })?,
                    state: state.parse()?,
                    payload: serde_json::from_str(&payload)?,
                });
            }
            Ok(result)
        })
        .await
    }

    async fn local_lifecycle_state(
        &self,
        operation_id: OperationId,
    ) -> Result<JournalState, LocalDbError> {
        self.call(move |connection| {
            let state: String = connection.query_row(
                "SELECT state FROM operation_journal WHERE operation_id=?1 AND kind='lifecycle_local'",
                rusqlite::params![operation_id.to_string()],
                |row| row.get(0),
            )?;
            state.parse()
        })
        .await
    }

    async fn advance_local_lifecycle(
        &self,
        operation_id: OperationId,
        expected: JournalState,
    ) -> Result<(), LocalDbError> {
        let next = expected
            .next()
            .ok_or_else(|| LocalDbError::InvalidJournalTransition {
                expected,
                next: expected,
            })?;
        let now = now_unix_ms();
        self.call(move |connection| {
            let changed = connection.execute(
                "UPDATE operation_journal SET state=?1,updated_at_unix_ms=?2 \
                 WHERE operation_id=?3 AND kind='lifecycle_local' AND state=?4",
                rusqlite::params![
                    next.as_str(),
                    now,
                    operation_id.to_string(),
                    expected.as_str()
                ],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows.into());
            }
            Ok(())
        })
        .await
    }
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests;
