use std::{
    collections::BTreeMap,
    io::{self, Cursor, Read},
};

use tar::{Builder, EntryType, Header, HeaderMode};
use thiserror::Error;

use crate::{
    BlobId, SkillEntry, SkillValidationError, TreeEntry, TreeEntryKind, TreeError, TreeId,
    validate_skill_directory,
};

/// One complete, owned entry from a skill tree. This is an in-memory domain value;
/// filesystem discovery and persistence remain edge responsibilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedSkillEntry {
    File {
        path: String,
        bytes: Vec<u8>,
        executable: bool,
    },
    Directory {
        path: String,
    },
    Symlink {
        path: String,
        target: String,
    },
}

impl OwnedSkillEntry {
    pub fn path(&self) -> &str {
        match self {
            Self::File { path, .. } | Self::Directory { path } | Self::Symlink { path, .. } => path,
        }
    }

    pub fn as_skill_entry(&self) -> SkillEntry<'_> {
        match self {
            Self::File {
                path,
                bytes,
                executable,
            } => SkillEntry::File {
                path,
                bytes,
                executable: *executable,
            },
            Self::Directory { path } => SkillEntry::Directory { path },
            Self::Symlink { path, target } => SkillEntry::Symlink { path, target },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillManifest {
    root_tree: TreeId,
    entries: Vec<SkillManifestEntry>,
}

impl SkillManifest {
    pub const fn from_declared_parts(root_tree: TreeId, entries: Vec<SkillManifestEntry>) -> Self {
        Self { root_tree, entries }
    }

    pub const fn root_tree(&self) -> TreeId {
        self.root_tree
    }

    pub fn entries(&self) -> &[SkillManifestEntry] {
        &self.entries
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillManifestEntry {
    File {
        path: String,
        blob: BlobId,
        size: u64,
        executable: bool,
    },
    Directory {
        path: String,
    },
    Symlink {
        path: String,
        target: String,
    },
}

impl SkillManifestEntry {
    pub fn path(&self) -> &str {
        match self {
            Self::File { path, .. } | Self::Directory { path } | Self::Symlink { path, .. } => path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeterministicSkillSnapshot {
    manifest: SkillManifest,
    bytes: Vec<u8>,
}

impl DeterministicSkillSnapshot {
    pub const fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

pub fn build_skill_manifest(
    parent_directory_name: &str,
    entries: &[OwnedSkillEntry],
) -> Result<SkillManifest, SnapshotError> {
    let borrowed = entries
        .iter()
        .map(OwnedSkillEntry::as_skill_entry)
        .collect::<Vec<_>>();
    validate_skill_directory(parent_directory_name, &borrowed)?;

    let mut manifest_entries = entries
        .iter()
        .map(|entry| match entry {
            OwnedSkillEntry::File {
                path,
                bytes,
                executable,
            } => Ok(SkillManifestEntry::File {
                path: path.clone(),
                blob: BlobId::hash(bytes),
                size: u64::try_from(bytes.len()).map_err(|_| SnapshotError::EntryTooLarge)?,
                executable: *executable,
            }),
            OwnedSkillEntry::Directory { path } => {
                Ok(SkillManifestEntry::Directory { path: path.clone() })
            }
            OwnedSkillEntry::Symlink { path, target } => Ok(SkillManifestEntry::Symlink {
                path: path.clone(),
                target: target.clone(),
            }),
        })
        .collect::<Result<Vec<_>, SnapshotError>>()?;
    manifest_entries.sort_by(|left, right| left.path().as_bytes().cmp(right.path().as_bytes()));

    let mut tree_ids = BTreeMap::<String, TreeId>::new();
    let mut directories = manifest_entries
        .iter()
        .filter_map(|entry| match entry {
            SkillManifestEntry::Directory { path } => Some(path.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    directories.push(String::new());
    directories.sort_by(|left, right| {
        component_count(right)
            .cmp(&component_count(left))
            .then_with(|| left.as_bytes().cmp(right.as_bytes()))
    });

    for directory in directories {
        let mut children = Vec::new();
        for entry in &manifest_entries {
            let (parent, name) = split_parent(entry.path());
            if parent != directory {
                continue;
            }
            let kind = match entry {
                SkillManifestEntry::File {
                    blob, executable, ..
                } => TreeEntryKind::File {
                    blob: *blob,
                    executable: *executable,
                },
                SkillManifestEntry::Directory { path } => TreeEntryKind::Directory {
                    tree: *tree_ids
                        .get(path)
                        .ok_or_else(|| SnapshotError::MissingChildTree(path.clone()))?,
                },
                SkillManifestEntry::Symlink { target, .. } => TreeEntryKind::Symlink {
                    target: target.clone(),
                },
            };
            children.push(TreeEntry::new(name, kind)?);
        }
        tree_ids.insert(directory, TreeId::from_entries(&children)?);
    }

    let root_tree = *tree_ids
        .get("")
        .ok_or_else(|| SnapshotError::MissingChildTree("<root>".to_owned()))?;
    Ok(SkillManifest {
        root_tree,
        entries: manifest_entries,
    })
}

pub fn build_deterministic_skill_snapshot(
    parent_directory_name: &str,
    entries: &[OwnedSkillEntry],
) -> Result<DeterministicSkillSnapshot, SnapshotError> {
    let manifest = build_skill_manifest(parent_directory_name, entries)?;
    let mut sorted = entries.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.path().as_bytes().cmp(right.path().as_bytes()));

    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 9)?;
    encoder.include_checksum(true)?;
    {
        let mut archive = Builder::new(&mut encoder);
        archive.mode(HeaderMode::Deterministic);
        for entry in sorted {
            append_entry(&mut archive, entry)?;
        }
        archive.finish()?;
    }
    let bytes = encoder.finish()?;
    Ok(DeterministicSkillSnapshot { manifest, bytes })
}

/// Decode an untrusted deterministic snapshot and prove it represents exactly the
/// supplied semantic manifest before returning any materializable entries.
pub fn validate_skill_snapshot(
    parent_directory_name: &str,
    expected: &SkillManifest,
    compressed: &[u8],
) -> Result<Vec<OwnedSkillEntry>, SnapshotError> {
    let decoder = zstd::stream::read::Decoder::new(Cursor::new(compressed))?;
    let mut archive = tar::Archive::new(decoder);
    let mut entries = Vec::new();

    for item in archive.entries()? {
        let mut item = item?;
        let path = item.path()?;
        let path = path
            .to_str()
            .ok_or(SnapshotError::ArchivePathNotUtf8)?
            .trim_end_matches('/')
            .to_owned();
        if path.is_empty() {
            return Err(SnapshotError::UnsupportedArchiveEntry);
        }
        let entry_type = item.header().entry_type();
        if entry_type.is_file() {
            let mut bytes = Vec::new();
            item.read_to_end(&mut bytes)?;
            let mode = item.header().mode()?;
            entries.push(OwnedSkillEntry::File {
                path,
                bytes,
                executable: mode & 0o111 != 0,
            });
        } else if entry_type.is_dir() {
            entries.push(OwnedSkillEntry::Directory { path });
        } else if entry_type.is_symlink() {
            let target = item
                .link_name()?
                .ok_or(SnapshotError::MissingSymlinkTarget)?;
            let target = target
                .to_str()
                .ok_or(SnapshotError::ArchivePathNotUtf8)?
                .to_owned();
            entries.push(OwnedSkillEntry::Symlink { path, target });
        } else {
            return Err(SnapshotError::UnsupportedArchiveEntry);
        }
    }

    let actual = build_skill_manifest(parent_directory_name, &entries)?;
    if &actual != expected {
        return Err(SnapshotError::ManifestMismatch);
    }
    Ok(entries)
}

fn append_entry(
    archive: &mut Builder<&mut zstd::stream::write::Encoder<'_, Vec<u8>>>,
    entry: &OwnedSkillEntry,
) -> Result<(), SnapshotError> {
    let mut header = Header::new_gnu();
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    match entry {
        OwnedSkillEntry::File {
            path,
            bytes,
            executable,
        } => {
            header.set_entry_type(EntryType::Regular);
            header.set_mode(if *executable { 0o755 } else { 0o644 });
            header.set_size(u64::try_from(bytes.len()).map_err(|_| SnapshotError::EntryTooLarge)?);
            header.set_cksum();
            archive.append_data(&mut header, path, Cursor::new(bytes))?;
        }
        OwnedSkillEntry::Directory { path } => {
            header.set_entry_type(EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            header.set_cksum();
            archive.append_data(&mut header, path, io::empty())?;
        }
        OwnedSkillEntry::Symlink { path, target } => {
            header.set_entry_type(EntryType::Symlink);
            header.set_mode(0o777);
            header.set_size(0);
            header.set_link_name(target)?;
            header.set_cksum();
            archive.append_data(&mut header, path, io::empty())?;
        }
    }
    Ok(())
}

fn split_parent(path: &str) -> (String, &str) {
    match path.rsplit_once('/') {
        Some((parent, name)) => (parent.to_owned(), name),
        None => (String::new(), path),
    }
}

fn component_count(path: &str) -> usize {
    if path.is_empty() {
        0
    } else {
        path.split('/').count()
    }
}

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("invalid skill snapshot content: {0}")]
    Skill(#[from] SkillValidationError),
    #[error("invalid semantic tree: {0}")]
    Tree(#[from] TreeError),
    #[error("snapshot I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("snapshot entry is too large for the current platform")]
    EntryTooLarge,
    #[error("snapshot manifest references directory tree before it is available: {0}")]
    MissingChildTree(String),
    #[error("snapshot archive path or link target is not UTF-8")]
    ArchivePathNotUtf8,
    #[error("snapshot archive contains an unsupported entry type")]
    UnsupportedArchiveEntry,
    #[error("snapshot symlink is missing its target")]
    MissingSymlinkTarget,
    #[error("snapshot bytes do not match the authoritative semantic manifest")]
    ManifestMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<OwnedSkillEntry> {
        vec![
            OwnedSkillEntry::File {
                path: "SKILL.md".to_owned(),
                bytes: b"---\nname: review\ndescription: Reviews changes.\nmetadata:\n  owner: fixture\n---\n# Review\n".to_vec(),
                executable: false,
            },
            OwnedSkillEntry::Directory {
                path: "scripts".to_owned(),
            },
            OwnedSkillEntry::File {
                path: "scripts/check.sh".to_owned(),
                bytes: b"#!/bin/sh\necho ok\n".to_vec(),
                executable: true,
            },
            OwnedSkillEntry::Symlink {
                path: "scripts/current".to_owned(),
                target: "check.sh".to_owned(),
            },
        ]
    }

    #[test]
    fn deterministic_snapshot_round_trips_exact_manifest() {
        let entries = fixture();
        let first = build_deterministic_skill_snapshot("review", &entries).unwrap();
        let second = build_deterministic_skill_snapshot("review", &entries).unwrap();
        assert_eq!(first.bytes(), second.bytes());
        assert_eq!(first.manifest(), second.manifest());

        let decoded = validate_skill_snapshot("review", first.manifest(), first.bytes()).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn semantic_manifest_rejects_corrupted_snapshot_content() {
        let entries = fixture();
        let snapshot = build_deterministic_skill_snapshot("review", &entries).unwrap();
        let mut changed = entries.clone();
        let OwnedSkillEntry::File { bytes, .. } = &mut changed[2] else {
            unreachable!();
        };
        bytes.extend_from_slice(b"changed\n");
        let corrupt = build_deterministic_skill_snapshot("review", &changed).unwrap();
        assert!(matches!(
            validate_skill_snapshot("review", snapshot.manifest(), corrupt.bytes()),
            Err(SnapshotError::ManifestMismatch)
        ));
    }
}
