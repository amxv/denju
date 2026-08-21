use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use denju_core::{
    BlobId, OperationId, OwnedSkillEntry, ResourceId, RevisionId, SkillManifest, SnapshotError,
    build_skill_manifest, validate_skill_snapshot,
};
use thiserror::Error;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    JournalState, LocalDatabase, LocalDbError, LocalPaths, MaterializationJournalPayload,
    create_native_directory_link,
};

#[derive(Debug, Clone)]
pub struct DesiredSkillMaterialization {
    pub resource_id: ResourceId,
    pub owner: String,
    pub skill_name: String,
    pub revision_id: RevisionId,
    pub manifest: SkillManifest,
}

pub async fn materialize_skill_snapshot(
    paths: &LocalPaths,
    db: &LocalDatabase,
    desired: &DesiredSkillMaterialization,
    snapshot: &[u8],
) -> Result<PathBuf, MaterializationError> {
    recover_materializations(paths, db).await?;
    let entries = validate_skill_snapshot(&desired.skill_name, &desired.manifest, snapshot)?;
    let operation_id = OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| MaterializationError::Corrupt(error.to_string()))?;
    let holder = operation_id.to_string();
    let resource_key = format!("skill:{}", desired.resource_id);
    if !db
        .claim_lease(resource_key.clone(), holder.clone(), now_unix_ms(), 60_000)
        .await?
    {
        return Err(MaterializationError::Busy(desired.resource_id.to_string()));
    }

    let result = materialize_with_lease(paths, db, desired, &entries, operation_id).await;
    let _ = db.release_lease(resource_key, holder).await;
    result
}

/// Prepare and verify an immutable generation without exposing it through the canonical path.
///
/// Lifecycle rename uses this after registry authority commits but before it records the local
/// rename journal. If the process dies before the journal write, the only residue is an
/// unreferenced generation directory; the old canonical/projection paths are still untouched.
pub(crate) fn stage_skill_generation(
    paths: &LocalPaths,
    desired: &DesiredSkillMaterialization,
    snapshot: &[u8],
    operation_id: OperationId,
) -> Result<PathBuf, MaterializationError> {
    let entries = validate_skill_snapshot(&desired.skill_name, &desired.manifest, snapshot)?;
    let resource_root = paths.generations.join(desired.resource_id.to_string());
    fs::create_dir_all(&resource_root)?;
    let generation_dir = resource_root.join(desired.revision_id.to_string());
    if generation_dir.is_dir() {
        verify_generation(&generation_dir, &desired.skill_name, &desired.manifest)?;
        return Ok(generation_dir);
    }

    let stage_dir = resource_root.join(format!(".rename-stage-{operation_id}"));
    let _ = fs::remove_dir_all(&stage_dir);
    write_generation(paths, &stage_dir, &entries)?;
    verify_generation(&stage_dir, &desired.skill_name, &desired.manifest)?;
    match fs::rename(&stage_dir, &generation_dir) {
        Ok(()) => sync_parent(&resource_root)?,
        Err(error) if generation_dir.is_dir() => {
            let _ = fs::remove_dir_all(&stage_dir);
            verify_generation(&generation_dir, &desired.skill_name, &desired.manifest)?;
            let _ = error;
        }
        Err(error) => return Err(MaterializationError::Io(error)),
    }
    Ok(generation_dir)
}

pub fn export_skill_snapshot(
    skill_name: &str,
    manifest: &SkillManifest,
    snapshot: &[u8],
    destination: &Path,
) -> Result<(), MaterializationError> {
    if destination.exists() {
        return Err(MaterializationError::Corrupt(format!(
            "export destination already exists: {}",
            destination.display()
        )));
    }
    let entries = validate_skill_snapshot(skill_name, manifest, snapshot)?;
    fs::create_dir_all(destination)?;
    let result = write_unmanaged_entries(destination, &entries);
    if result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    result
}

fn write_unmanaged_entries(
    root: &Path,
    entries: &[OwnedSkillEntry],
) -> Result<(), MaterializationError> {
    let mut directories = entries
        .iter()
        .filter_map(|entry| match entry {
            OwnedSkillEntry::Directory { path } => Some(path.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    directories.sort_by_key(|path| path.split('/').count());
    for path in directories {
        fs::create_dir_all(root.join(path))?;
    }
    for entry in entries {
        match entry {
            OwnedSkillEntry::File {
                path,
                bytes,
                executable,
            } => {
                let destination = root.join(path);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&destination, bytes)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(
                        &destination,
                        fs::Permissions::from_mode(if *executable { 0o755 } else { 0o644 }),
                    )?;
                }
            }
            OwnedSkillEntry::Directory { .. } => {}
            OwnedSkillEntry::Symlink { path, target } => {
                let destination = root.join(path);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                create_relative_symlink(target, &destination)?;
            }
        }
    }
    sync_parent(root)?;
    Ok(())
}

async fn materialize_with_lease(
    paths: &LocalPaths,
    db: &LocalDatabase,
    desired: &DesiredSkillMaterialization,
    entries: &[OwnedSkillEntry],
    operation_id: OperationId,
) -> Result<PathBuf, MaterializationError> {
    let resource_root = paths.generations.join(desired.resource_id.to_string());
    fs::create_dir_all(&resource_root)?;
    let stage_dir = resource_root.join(format!(".stage-{operation_id}"));
    let generation_dir = resource_root.join(desired.revision_id.to_string());
    let canonical_path = paths.skills.join(&desired.owner).join(&desired.skill_name);
    let payload = MaterializationJournalPayload {
        resource_id: desired.resource_id.to_string(),
        revision_id: desired.revision_id.to_string(),
        stage_dir: stage_dir.display().to_string(),
        generation_dir: generation_dir.display().to_string(),
        canonical_path: canonical_path.display().to_string(),
    };
    db.create_materialization_journal(operation_id, payload, now_unix_ms())
        .await?;

    if generation_dir.is_dir() {
        verify_generation(&generation_dir, &desired.skill_name, &desired.manifest)?;
    } else {
        let _ = fs::remove_dir_all(&stage_dir);
        write_generation(paths, &stage_dir, entries)?;
    }
    db.update_materialization_journal(
        operation_id,
        JournalState::Planned,
        JournalState::Staged,
        now_unix_ms(),
    )
    .await?;

    if !generation_dir.is_dir() {
        verify_generation(&stage_dir, &desired.skill_name, &desired.manifest)?;
        fs::rename(&stage_dir, &generation_dir)?;
        sync_parent(&resource_root)?;
    }
    db.update_materialization_journal(
        operation_id,
        JournalState::Staged,
        JournalState::Verified,
        now_unix_ms(),
    )
    .await?;

    atomic_switch_directory_link(&generation_dir, &canonical_path, operation_id)?;
    db.update_materialization_journal(
        operation_id,
        JournalState::Verified,
        JournalState::Switched,
        now_unix_ms(),
    )
    .await?;
    db.mark_skill_materialized(
        desired.resource_id.to_string(),
        desired.revision_id.to_string(),
        now_unix_ms(),
    )
    .await?;
    db.update_materialization_journal(
        operation_id,
        JournalState::Switched,
        JournalState::Complete,
        now_unix_ms(),
    )
    .await?;
    Ok(generation_dir)
}

pub async fn recover_materializations(
    _paths: &LocalPaths,
    db: &LocalDatabase,
) -> Result<(), MaterializationError> {
    for journal in db.materialization_journals().await? {
        let stage = PathBuf::from(&journal.payload.stage_dir);
        let generation = PathBuf::from(&journal.payload.generation_dir);
        let canonical = PathBuf::from(&journal.payload.canonical_path);
        match journal.state {
            JournalState::Planned | JournalState::Staged => {
                let _ = fs::remove_dir_all(stage);
                db.discard_materialization_journal(journal.operation_id)
                    .await?;
            }
            JournalState::Verified => {
                if !generation.is_dir() {
                    return Err(MaterializationError::Corrupt(format!(
                        "verified generation is missing for {}",
                        journal.payload.resource_id
                    )));
                }
                atomic_switch_directory_link(&generation, &canonical, journal.operation_id)?;
                db.update_materialization_journal(
                    journal.operation_id,
                    JournalState::Verified,
                    JournalState::Switched,
                    now_unix_ms(),
                )
                .await?;
                db.mark_skill_materialized(
                    journal.payload.resource_id.clone(),
                    journal.payload.revision_id.clone(),
                    now_unix_ms(),
                )
                .await?;
                db.update_materialization_journal(
                    journal.operation_id,
                    JournalState::Switched,
                    JournalState::Complete,
                    now_unix_ms(),
                )
                .await?;
            }
            JournalState::Switched => {
                let target = fs::canonicalize(&canonical)?;
                let expected = fs::canonicalize(&generation)?;
                if target != expected {
                    return Err(MaterializationError::Corrupt(format!(
                        "switched canonical path does not target verified generation for {}",
                        journal.payload.resource_id
                    )));
                }
                db.mark_skill_materialized(
                    journal.payload.resource_id.clone(),
                    journal.payload.revision_id.clone(),
                    now_unix_ms(),
                )
                .await?;
                db.update_materialization_journal(
                    journal.operation_id,
                    JournalState::Switched,
                    JournalState::Complete,
                    now_unix_ms(),
                )
                .await?;
            }
            JournalState::Complete => {}
        }
    }
    Ok(())
}

pub fn remove_canonical_skill(
    paths: &LocalPaths,
    owner: &str,
    skill_name: &str,
) -> Result<(), MaterializationError> {
    let canonical = paths.skills.join(owner).join(skill_name);
    remove_owned_link(&canonical)?;
    if let Some(parent) = canonical.parent() {
        let _ = fs::remove_dir(parent);
    }
    Ok(())
}

/// Remove canonical links that are no longer represented by local desired state.
///
/// The SQLite desired tables are authoritative. This makes locator changes self-healing even
/// if an older binary updated the row before deleting its previous canonical link, or if a
/// process died between those two idempotent steps. Only Denju's internal canonical tree is
/// inspected; harness roots and user source directories are never scanned here.
pub async fn reconcile_canonical_links(
    paths: &LocalPaths,
    db: &LocalDatabase,
) -> Result<usize, MaterializationError> {
    let desired = db
        .managed_skills()
        .await?
        .into_iter()
        .map(|record| (record.owner, record.skill_name))
        .collect::<BTreeSet<_>>();
    let mut removed = 0;
    for owner_entry in fs::read_dir(&paths.skills)? {
        let owner_entry = owner_entry?;
        let owner = owner_entry.file_name().into_string().map_err(|_| {
            MaterializationError::Corrupt("canonical owner directory is not UTF-8".to_owned())
        })?;
        let owner_path = owner_entry.path();
        if !owner_path.is_dir() {
            return Err(MaterializationError::Corrupt(format!(
                "unexpected canonical owner entry {}",
                owner_path.display()
            )));
        }
        for skill_entry in fs::read_dir(&owner_path)? {
            let skill_entry = skill_entry?;
            let skill_name = skill_entry.file_name().into_string().map_err(|_| {
                MaterializationError::Corrupt("canonical skill name is not UTF-8".to_owned())
            })?;
            if desired.contains(&(owner.clone(), skill_name)) {
                continue;
            }
            remove_owned_link(&skill_entry.path())?;
            removed += 1;
        }
        let _ = fs::remove_dir(&owner_path);
    }
    Ok(removed)
}

pub(crate) fn write_generation(
    paths: &LocalPaths,
    root: &Path,
    entries: &[OwnedSkillEntry],
) -> Result<(), MaterializationError> {
    fs::create_dir_all(root)?;
    let mut directories = entries
        .iter()
        .filter_map(|entry| match entry {
            OwnedSkillEntry::Directory { path } => Some(path.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    directories.sort_by_key(|path| path.split('/').count());
    for path in directories {
        fs::create_dir_all(root.join(path))?;
    }

    for entry in entries {
        match entry {
            OwnedSkillEntry::File {
                path,
                bytes,
                executable,
            } => {
                let cas = store_cas_blob(paths, bytes)?;
                let destination = root.join(path);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(cas, &destination)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(
                        &destination,
                        fs::Permissions::from_mode(if *executable { 0o755 } else { 0o644 }),
                    )?;
                }
            }
            OwnedSkillEntry::Directory { .. } => {}
            OwnedSkillEntry::Symlink { path, target } => {
                let destination = root.join(path);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                create_relative_symlink(target, &destination)?;
            }
        }
    }
    sync_parent(root)?;
    Ok(())
}

fn store_cas_blob(paths: &LocalPaths, bytes: &[u8]) -> Result<PathBuf, MaterializationError> {
    let blob = BlobId::hash(bytes);
    let id = blob.to_string();
    let directory = paths.objects.join(&id[..2]);
    let destination = directory.join(&id);
    fs::create_dir_all(&directory)?;
    if destination.is_file() {
        if BlobId::hash(&fs::read(&destination)?) != blob {
            return Err(MaterializationError::Corrupt(format!(
                "local CAS object {blob} does not match its content"
            )));
        }
        return Ok(destination);
    }
    let temporary = directory.join(format!(".{id}.tmp-{}", Uuid::now_v7()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, &destination)?;
    sync_parent(&directory)?;
    Ok(destination)
}

fn verify_generation(
    root: &Path,
    skill_name: &str,
    expected: &SkillManifest,
) -> Result<(), MaterializationError> {
    let entries = read_generation(root)?;
    let actual = build_skill_manifest(skill_name, &entries)?;
    if &actual != expected {
        return Err(MaterializationError::ManifestMismatch);
    }
    Ok(())
}

fn read_generation(root: &Path) -> Result<Vec<OwnedSkillEntry>, MaterializationError> {
    let mut entries = Vec::new();
    for item in WalkDir::new(root).follow_links(false).min_depth(1) {
        let item = item.map_err(MaterializationError::Walk)?;
        let relative = item.path().strip_prefix(root).map_err(|error| {
            MaterializationError::Corrupt(format!("generation path escaped root: {error}"))
        })?;
        let path = relative
            .to_str()
            .ok_or_else(|| {
                MaterializationError::Corrupt("generation path is not UTF-8".to_owned())
            })?
            .replace('\\', "/");
        if item.file_type().is_symlink() {
            let target = fs::read_link(item.path())?;
            let target = target
                .to_str()
                .ok_or_else(|| {
                    MaterializationError::Corrupt("symlink target is not UTF-8".to_owned())
                })?
                .replace('\\', "/");
            entries.push(OwnedSkillEntry::Symlink { path, target });
        } else if item.file_type().is_dir() {
            entries.push(OwnedSkillEntry::Directory { path });
        } else if item.file_type().is_file() {
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                item.metadata()
                    .map_err(MaterializationError::Walk)?
                    .permissions()
                    .mode()
                    & 0o111
                    != 0
            };
            #[cfg(not(unix))]
            let executable = false;
            entries.push(OwnedSkillEntry::File {
                path,
                bytes: fs::read(item.path())?,
                executable,
            });
        } else {
            return Err(MaterializationError::Corrupt(format!(
                "unsupported generation entry {}",
                item.path().display()
            )));
        }
    }
    Ok(entries)
}

pub(crate) fn atomic_switch_directory_link(
    target: &Path,
    link: &Path,
    operation_id: OperationId,
) -> Result<(), MaterializationError> {
    let parent = link
        .parent()
        .ok_or_else(|| MaterializationError::Corrupt("canonical link has no parent".to_owned()))?;
    fs::create_dir_all(parent)?;
    if link.exists() || fs::symlink_metadata(link).is_ok() {
        let metadata = fs::symlink_metadata(link)?;
        #[cfg(unix)]
        if !metadata.file_type().is_symlink() {
            return Err(MaterializationError::RefuseOverwrite(link.to_owned()));
        }
        #[cfg(windows)]
        if !metadata.file_type().is_symlink() && !metadata.is_dir() {
            return Err(MaterializationError::RefuseOverwrite(link.to_owned()));
        }
    }
    let temporary = parent.join(format!(".denju-link-{operation_id}"));
    let _ = fs::remove_file(&temporary);
    let _ = fs::remove_dir(&temporary);
    create_native_directory_link(target, &temporary)?;
    if let Err(error) = fs::rename(&temporary, link) {
        let _ = fs::remove_file(&temporary);
        let _ = fs::remove_dir(&temporary);
        return Err(MaterializationError::Io(error));
    }
    sync_parent(parent)?;
    Ok(())
}

pub(crate) fn remove_owned_link(path: &Path) -> Result<(), MaterializationError> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    #[cfg(unix)]
    if !metadata.file_type().is_symlink() {
        return Err(MaterializationError::RefuseOverwrite(path.to_owned()));
    }
    #[cfg(windows)]
    if !metadata.file_type().is_symlink() && !metadata.is_dir() {
        return Err(MaterializationError::RefuseOverwrite(path.to_owned()));
    }
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn create_relative_symlink(target: &str, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_relative_symlink(target: &str, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

fn sync_parent(path: &Path) -> Result<(), MaterializationError> {
    #[cfg(unix)]
    {
        let directory = File::open(path)?;
        directory.sync_all()?;
    }
    Ok(())
}

fn now_unix_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

#[derive(Debug, Error)]
pub enum MaterializationError {
    #[error("snapshot verification failed: {0}")]
    Snapshot(#[from] SnapshotError),
    #[error("local database error: {0}")]
    Database(#[from] LocalDbError),
    #[error("local filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("failed to walk materialized generation: {0}")]
    Walk(walkdir::Error),
    #[error("materialized generation does not match the authoritative manifest")]
    ManifestMismatch,
    #[error("local materialization state is corrupt: {0}")]
    Corrupt(String),
    #[error("resource {0} is already being materialized")]
    Busy(String),
    #[error("refusing to overwrite unmanaged path {path}", path = .0.display())]
    RefuseOverwrite(PathBuf),
}

impl From<denju_core::TreeError> for MaterializationError {
    fn from(error: denju_core::TreeError) -> Self {
        Self::Corrupt(error.to_string())
    }
}

impl From<denju_core::SkillValidationError> for MaterializationError {
    fn from(error: denju_core::SkillValidationError) -> Self {
        Self::Corrupt(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use denju_core::{RevisionId, build_deterministic_skill_snapshot};
    use tempfile::tempdir;

    use super::*;
    use crate::{SubscriptionRecord, ensure_local_layout};

    fn entries() -> Vec<OwnedSkillEntry> {
        vec![OwnedSkillEntry::File {
            path: "SKILL.md".to_owned(),
            bytes: b"---\nname: review\ndescription: Reviews code.\n---\n# Review\n".to_vec(),
            executable: false,
        }]
    }

    async fn fixture() -> (
        tempfile::TempDir,
        LocalPaths,
        LocalDatabase,
        DesiredSkillMaterialization,
        Vec<u8>,
    ) {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        ensure_local_layout(&paths).unwrap();
        let db = LocalDatabase::open(&paths.state_db).await.unwrap();
        let snapshot = build_deterministic_skill_snapshot("review", &entries()).unwrap();
        let resource_id = ResourceId::from_str("01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1").unwrap();
        let revision_id = RevisionId::from_bytes([7; 32]);
        db.upsert_subscription_desired(
            SubscriptionRecord {
                resource_id: resource_id.to_string(),
                locator: "@alice/review".to_owned(),
                owner: "alice".to_owned(),
                skill_name: "review".to_owned(),
                resource_generation: 1,
                release_version: 1,
                desired_revision_id: revision_id.to_string(),
                harness_name: None,
                materialized_revision_id: None,
                retain_on_delete: false,
                retained_after_delete: false,
            },
            1,
        )
        .await
        .unwrap();
        let desired = DesiredSkillMaterialization {
            resource_id,
            owner: "alice".to_owned(),
            skill_name: "review".to_owned(),
            revision_id,
            manifest: snapshot.manifest().clone(),
        };
        let bytes = snapshot.bytes().to_vec();
        (home, paths, db, desired, bytes)
    }

    #[tokio::test]
    async fn verified_snapshot_switches_one_canonical_generation() {
        let (_home, paths, db, desired, bytes) = fixture().await;
        let generation = materialize_skill_snapshot(&paths, &db, &desired, &bytes)
            .await
            .unwrap();
        let canonical = paths.skills.join("alice/review");
        assert_eq!(
            fs::canonicalize(canonical).unwrap(),
            fs::canonicalize(generation).unwrap()
        );
        let record = db
            .subscription(desired.resource_id.to_string())
            .await
            .unwrap()
            .unwrap();
        let revision = desired.revision_id.to_string();
        assert_eq!(
            record.materialized_revision_id.as_deref(),
            Some(revision.as_str())
        );
    }

    #[tokio::test]
    async fn corrupt_snapshot_never_changes_visibility() {
        let (_home, paths, db, desired, mut bytes) = fixture().await;
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x01;
        assert!(
            materialize_skill_snapshot(&paths, &db, &desired, &bytes)
                .await
                .is_err()
        );
        assert!(!paths.skills.join("alice/review").exists());
    }

    #[tokio::test]
    async fn canonical_reconcile_removes_a_stale_locator_after_desired_identity_moves() {
        let (_home, paths, db, desired, bytes) = fixture().await;
        materialize_skill_snapshot(&paths, &db, &desired, &bytes)
            .await
            .unwrap();
        assert!(paths.skills.join("alice/review").exists());
        db.upsert_subscription_desired(
            SubscriptionRecord {
                resource_id: desired.resource_id.to_string(),
                locator: "@alice/code-review".to_owned(),
                owner: "alice".to_owned(),
                skill_name: "code-review".to_owned(),
                resource_generation: 2,
                release_version: 2,
                desired_revision_id: desired.revision_id.to_string(),
                harness_name: None,
                materialized_revision_id: None,
                retain_on_delete: false,
                retained_after_delete: false,
            },
            2,
        )
        .await
        .unwrap();

        assert_eq!(reconcile_canonical_links(&paths, &db).await.unwrap(), 1);
        assert!(!paths.skills.join("alice/review").exists());
    }

    #[tokio::test]
    async fn interrupted_materialization_rolls_back_unverified_and_finishes_verified_state() {
        let (_home, paths, db, desired, _bytes) = fixture().await;
        let resource_root = paths.generations.join(desired.resource_id.to_string());
        fs::create_dir_all(&resource_root).unwrap();

        let staged_operation = OperationId::from_uuid(Uuid::now_v7()).unwrap();
        let staged_dir = resource_root.join(".interrupted-stage");
        fs::create_dir_all(&staged_dir).unwrap();
        fs::write(staged_dir.join("partial"), b"not verified").unwrap();
        let generation_dir = resource_root.join(desired.revision_id.to_string());
        let canonical = paths.skills.join("alice/review");
        db.create_materialization_journal(
            staged_operation,
            MaterializationJournalPayload {
                resource_id: desired.resource_id.to_string(),
                revision_id: desired.revision_id.to_string(),
                stage_dir: staged_dir.display().to_string(),
                generation_dir: generation_dir.display().to_string(),
                canonical_path: canonical.display().to_string(),
            },
            10,
        )
        .await
        .unwrap();
        db.update_materialization_journal(
            staged_operation,
            JournalState::Planned,
            JournalState::Staged,
            11,
        )
        .await
        .unwrap();
        recover_materializations(&paths, &db).await.unwrap();
        assert!(!staged_dir.exists());
        assert!(!canonical.exists());

        write_generation(&paths, &generation_dir, &entries()).unwrap();
        let verified_operation = OperationId::from_uuid(Uuid::now_v7()).unwrap();
        db.create_materialization_journal(
            verified_operation,
            MaterializationJournalPayload {
                resource_id: desired.resource_id.to_string(),
                revision_id: desired.revision_id.to_string(),
                stage_dir: resource_root.join("unused-stage").display().to_string(),
                generation_dir: generation_dir.display().to_string(),
                canonical_path: canonical.display().to_string(),
            },
            20,
        )
        .await
        .unwrap();
        db.update_materialization_journal(
            verified_operation,
            JournalState::Planned,
            JournalState::Staged,
            21,
        )
        .await
        .unwrap();
        db.update_materialization_journal(
            verified_operation,
            JournalState::Staged,
            JournalState::Verified,
            22,
        )
        .await
        .unwrap();
        recover_materializations(&paths, &db).await.unwrap();
        assert_eq!(
            fs::canonicalize(&canonical).unwrap(),
            fs::canonicalize(&generation_dir).unwrap()
        );
        let record = db
            .subscription(desired.resource_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            record.materialized_revision_id,
            Some(desired.revision_id.to_string())
        );
    }
}
