use std::{fs, io, path::Path};

use denju_core::OwnedSkillEntry;
use thiserror::Error;
use walkdir::WalkDir;

/// Read one user-owned skill directory without following links. Semantic validation stays
/// in denju-core; this boundary only preserves the filesystem facts needed by that validator.
pub fn read_skill_source(root: &Path) -> Result<Vec<OwnedSkillEntry>, SourceError> {
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(SourceError::NotDirectory(root.display().to_string()));
    }

    let mut entries = Vec::new();
    for item in WalkDir::new(root).follow_links(false).min_depth(1) {
        let item = item.map_err(SourceError::Walk)?;
        let relative = item
            .path()
            .strip_prefix(root)
            .map_err(|error| SourceError::InvalidPath(error.to_string()))?;
        let path = relative
            .to_str()
            .ok_or(SourceError::NonUtf8Path)?
            .replace('\\', "/");
        let file_type = item.file_type();
        if file_type.is_symlink() {
            let target = fs::read_link(item.path())?;
            let target = target
                .to_str()
                .ok_or(SourceError::NonUtf8LinkTarget)?
                .replace('\\', "/");
            entries.push(OwnedSkillEntry::Symlink { path, target });
        } else if file_type.is_dir() {
            entries.push(OwnedSkillEntry::Directory { path });
        } else if file_type.is_file() {
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                item.metadata()
                    .map_err(SourceError::Walk)?
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
            return Err(SourceError::UnsupportedEntry(
                item.path().display().to_string(),
            ));
        }
    }
    Ok(entries)
}

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("skill source must be a real directory, not a file or link: {0}")]
    NotDirectory(String),
    #[error("skill source filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("failed to walk skill source: {0}")]
    Walk(walkdir::Error),
    #[error("skill source path escaped its root: {0}")]
    InvalidPath(String),
    #[error("skill source contains a non-UTF-8 path")]
    NonUtf8Path,
    #[error("skill source contains a non-UTF-8 symlink target")]
    NonUtf8LinkTarget,
    #[error("skill source contains unsupported filesystem entry: {0}")]
    UnsupportedEntry(String),
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn source_reader_preserves_dotfiles_executable_bits_and_links() {
        let root = tempdir().unwrap();
        fs::write(root.path().join(".env.example"), b"SAFE=1\n").unwrap();
        fs::create_dir(root.path().join("scripts")).unwrap();
        let script = root.path().join("scripts/run.sh");
        fs::write(&script, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink("run.sh", root.path().join("scripts/current")).unwrap();
        }

        let entries = read_skill_source(root.path()).unwrap();
        assert!(entries.iter().any(|entry| {
            matches!(entry, OwnedSkillEntry::File { path, .. } if path == ".env.example")
        }));
        #[cfg(unix)]
        assert!(entries.iter().any(|entry| {
            matches!(entry, OwnedSkillEntry::File { path, executable: true, .. } if path == "scripts/run.sh")
        }));
        #[cfg(unix)]
        assert!(entries.iter().any(|entry| {
            matches!(entry, OwnedSkillEntry::Symlink { path, target } if path == "scripts/current" && target == "run.sh")
        }));
    }
}
