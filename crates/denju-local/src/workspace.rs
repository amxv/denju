use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, OnceLock},
    time::UNIX_EPOCH,
};

use denju_core::{
    BlobId, SkillManifest, SkillManifestEntry, build_skill_manifest_from_hashed_entries,
    skill_document_declared_name,
};
use thiserror::Error;
use tokio::sync::Semaphore;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{LocalDatabase, LocalDbError, LocalPaths, OwnedSkillRecord, WorkspaceFileRecord};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkspaceScanStats {
    pub hashed_files: usize,
    pub reused_files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceScan {
    pub manifest: SkillManifest,
    pub stats: WorkspaceScanStats,
    pub working_generation_path: PathBuf,
}

pub async fn scan_owned_workspace(
    paths: &LocalPaths,
    db: &LocalDatabase,
    record: &OwnedSkillRecord,
    force_full_hash: bool,
) -> Result<WorkspaceScan, WorkspaceScanError> {
    let previous = db
        .workspace_file_index(record.resource_id.clone())
        .await?
        .into_iter()
        .map(|row| (row.path.clone(), row))
        .collect::<BTreeMap<_, _>>();
    let permit = scan_semaphore()
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| WorkspaceScanError::Corrupt("workspace scan pool closed".to_owned()))?;
    let paths = paths.clone();
    let record = record.clone();
    let resource_id = record.resource_id.clone();
    let scan = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        scan_filesystem(&paths, &record, &previous, force_full_hash)
    })
    .await
    .map_err(|error| {
        WorkspaceScanError::Corrupt(format!("workspace scan task failed: {error}"))
    })??;
    db.replace_workspace_file_index(resource_id, scan.index)
        .await?;
    Ok(WorkspaceScan {
        manifest: scan.manifest,
        stats: scan.stats,
        working_generation_path: scan.working_generation_path,
    })
}

struct FilesystemScan {
    manifest: SkillManifest,
    stats: WorkspaceScanStats,
    working_generation_path: PathBuf,
    index: Vec<WorkspaceFileRecord>,
}

fn scan_filesystem(
    paths: &LocalPaths,
    record: &OwnedSkillRecord,
    previous: &BTreeMap<String, WorkspaceFileRecord>,
    force_full_hash: bool,
) -> Result<FilesystemScan, WorkspaceScanError> {
    let canonical = paths.skills.join(&record.owner).join(&record.skill_name);
    let root = fs::canonicalize(&canonical).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            WorkspaceScanError::MissingCanonical {
                path: canonical.clone(),
            }
        } else {
            WorkspaceScanError::Io(error)
        }
    })?;
    let generations = fs::canonicalize(&paths.generations)?;
    if !root.starts_with(&generations) {
        return Err(WorkspaceScanError::UnmanagedCanonical { path: root });
    }
    let mut manifest_entries = Vec::new();
    let mut index = Vec::new();
    let mut skill_md = None;
    let mut stats = WorkspaceScanStats::default();

    for item in WalkDir::new(&root).follow_links(false).min_depth(1) {
        let item = item.map_err(WorkspaceScanError::Walk)?;
        let relative = item.path().strip_prefix(&root).map_err(|error| {
            WorkspaceScanError::Corrupt(format!("workspace entry escaped root: {error}"))
        })?;
        let path = relative
            .to_str()
            .ok_or_else(|| WorkspaceScanError::Corrupt("workspace path is not UTF-8".into()))?
            .replace('\\', "/");
        if item.file_type().is_symlink() {
            let target = fs::read_link(item.path())?;
            let target = target
                .to_str()
                .ok_or_else(|| {
                    WorkspaceScanError::Corrupt("workspace symlink target is not UTF-8".into())
                })?
                .replace('\\', "/");
            manifest_entries.push(SkillManifestEntry::Symlink {
                path: path.clone(),
                target: target.clone(),
            });
            index.push(WorkspaceFileRecord {
                resource_id: record.resource_id.clone(),
                path,
                kind: "symlink".to_owned(),
                size_bytes: None,
                mtime_ns: None,
                executable: None,
                blob_id: None,
                symlink_target: Some(target),
            });
        } else if item.file_type().is_dir() {
            manifest_entries.push(SkillManifestEntry::Directory { path: path.clone() });
            index.push(WorkspaceFileRecord {
                resource_id: record.resource_id.clone(),
                path,
                kind: "directory".to_owned(),
                size_bytes: None,
                mtime_ns: None,
                executable: None,
                blob_id: None,
                symlink_target: None,
            });
        } else if item.file_type().is_file() {
            let metadata = item.metadata().map_err(WorkspaceScanError::Walk)?;
            let size = i64::try_from(metadata.len())
                .map_err(|_| WorkspaceScanError::Corrupt("file is too large".into()))?;
            let mtime_ns = metadata_mtime_ns(&metadata)?;
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let executable = false;
            let reusable = !force_full_hash
                && previous.get(&path).is_some_and(|old| {
                    old.kind == "file"
                        && old.size_bytes == Some(size)
                        && old.mtime_ns == Some(mtime_ns)
                        && old.executable == Some(executable)
                        && old.blob_id.is_some()
                });
            let (blob, bytes) = if reusable {
                stats.reused_files += 1;
                let blob = BlobId::from_str(
                    previous[&path]
                        .blob_id
                        .as_deref()
                        .expect("reusable file has blob"),
                )
                .map_err(|error| WorkspaceScanError::Corrupt(error.to_string()))?;
                let bytes = if path == "SKILL.md" {
                    Some(fs::read(item.path())?)
                } else {
                    None
                };
                (blob, bytes)
            } else {
                let bytes = fs::read(item.path())?;
                let blob = BlobId::hash(&bytes);
                store_workspace_blob(paths, blob, &bytes)?;
                stats.hashed_files += 1;
                (blob, Some(bytes))
            };
            if path == "SKILL.md" {
                skill_md = bytes.or_else(|| fs::read(item.path()).ok());
            }
            manifest_entries.push(SkillManifestEntry::File {
                path: path.clone(),
                blob,
                size: u64::try_from(size)
                    .map_err(|_| WorkspaceScanError::Corrupt("negative file size".into()))?,
                executable,
            });
            index.push(WorkspaceFileRecord {
                resource_id: record.resource_id.clone(),
                path,
                kind: "file".to_owned(),
                size_bytes: Some(size),
                mtime_ns: Some(mtime_ns),
                executable: Some(executable),
                blob_id: Some(blob.to_string()),
                symlink_target: None,
            });
        } else {
            return Err(WorkspaceScanError::UnsupportedEntry {
                path: item.path().to_owned(),
            });
        }
    }

    let skill_md = skill_md.ok_or_else(|| {
        WorkspaceScanError::Validation("skill directory must contain SKILL.md".to_owned())
    })?;
    let declared = skill_document_declared_name(&skill_md)
        .map_err(|error| WorkspaceScanError::Validation(error.to_string()))?;
    if declared != record.skill_name {
        return Err(WorkspaceScanError::PendingRename {
            requested: declared,
        });
    }
    let manifest =
        build_skill_manifest_from_hashed_entries(&record.skill_name, &skill_md, manifest_entries)
            .map_err(|error| WorkspaceScanError::Validation(error.to_string()))?;
    Ok(FilesystemScan {
        manifest,
        stats,
        working_generation_path: root,
        index,
    })
}

fn scan_semaphore() -> &'static Arc<Semaphore> {
    static SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEMAPHORE.get_or_init(|| {
        let cpus = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(2);
        Arc::new(Semaphore::new((cpus / 2).clamp(1, 4)))
    })
}

pub fn workspace_blob_path(paths: &LocalPaths, blob: BlobId) -> PathBuf {
    let id = blob.to_string();
    paths.objects.join(&id[..2]).join(id)
}

fn store_workspace_blob(
    paths: &LocalPaths,
    blob: BlobId,
    bytes: &[u8],
) -> Result<(), WorkspaceScanError> {
    let destination = workspace_blob_path(paths, blob);
    if destination.is_file() {
        if BlobId::hash(&fs::read(&destination)?) != blob {
            return Err(WorkspaceScanError::Corrupt(format!(
                "local CAS object {blob} does not match its content"
            )));
        }
        return Ok(());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| WorkspaceScanError::Corrupt("CAS path has no parent".into()))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".{}.tmp-{}", blob, Uuid::now_v7()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, &destination)?;
    Ok(())
}

fn metadata_mtime_ns(metadata: &fs::Metadata) -> Result<i64, WorkspaceScanError> {
    let modified = metadata.modified()?;
    let nanos = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|error| WorkspaceScanError::Corrupt(error.to_string()))?
        .as_nanos();
    i64::try_from(nanos).map_err(|_| WorkspaceScanError::Corrupt("mtime is out of range".into()))
}

#[derive(Debug, Error)]
pub enum WorkspaceScanError {
    #[error("workspace filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("workspace database error: {0}")]
    Database(#[from] LocalDbError),
    #[error("failed to walk workspace: {0}")]
    Walk(walkdir::Error),
    #[error("managed canonical skill is missing: {path}", path = path.display())]
    MissingCanonical { path: PathBuf },
    #[error("canonical skill points outside Denju generations: {path}", path = path.display())]
    UnmanagedCanonical { path: PathBuf },
    #[error("pending rename to {requested}")]
    PendingRename { requested: String },
    #[error("invalid stable skill content: {0}")]
    Validation(String),
    #[error("unsupported workspace entry: {path}", path = path.display())]
    UnsupportedEntry { path: PathBuf },
    #[error("workspace state is corrupt: {0}")]
    Corrupt(String),
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use denju_core::{ResourceId, build_deterministic_skill_snapshot};
    use tempfile::tempdir;

    use super::*;
    use crate::{
        DesiredSkillMaterialization, LocalPaths, OwnedSkillRecord, ensure_local_layout,
        materialize_skill_snapshot,
    };

    #[tokio::test]
    async fn incremental_scan_hashes_only_changed_file_after_baseline() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        ensure_local_layout(&paths).unwrap();
        let db = LocalDatabase::open(&paths.state_db).await.unwrap();
        let resource_id = ResourceId::from_str("01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1").unwrap();
        let entries = vec![
            denju_core::OwnedSkillEntry::File {
                path: "SKILL.md".into(),
                bytes: b"---\nname: review\ndescription: Reviews code.\n---\n# Review\n".to_vec(),
                executable: false,
            },
            denju_core::OwnedSkillEntry::File {
                path: "a.txt".into(),
                bytes: b"a\n".to_vec(),
                executable: false,
            },
            denju_core::OwnedSkillEntry::File {
                path: "b.txt".into(),
                bytes: b"b\n".to_vec(),
                executable: false,
            },
        ];
        let snapshot = build_deterministic_skill_snapshot("review", &entries).unwrap();
        let revision_id = denju_core::RevisionId::from_bytes([7; 32]);
        db.upsert_owned_skill_desired(
            OwnedSkillRecord {
                resource_id: resource_id.to_string(),
                locator: "@alice/review".into(),
                owner: "alice".into(),
                skill_name: "review".into(),
                resource_generation: 1,
                desired_revision_id: revision_id.to_string(),
                harness_name: None,
                materialized_revision_id: None,
            },
            1,
        )
        .await
        .unwrap();
        materialize_skill_snapshot(
            &paths,
            &db,
            &DesiredSkillMaterialization {
                resource_id,
                owner: "alice".into(),
                skill_name: "review".into(),
                revision_id,
                manifest: snapshot.manifest().clone(),
            },
            snapshot.bytes(),
        )
        .await
        .unwrap();
        let record = db.owned_skills().await.unwrap().remove(0);
        let first = scan_owned_workspace(&paths, &db, &record, false)
            .await
            .unwrap();
        assert_eq!(first.stats.hashed_files, 3);
        fs::write(paths.skills.join("alice/review/a.txt"), b"changed\n").unwrap();
        let second = scan_owned_workspace(&paths, &db, &record, false)
            .await
            .unwrap();
        assert_eq!(second.stats.hashed_files, 1);
        assert!(second.stats.reused_files >= 2);
        assert_ne!(first.manifest.root_tree(), second.manifest.root_tree());
    }
}
