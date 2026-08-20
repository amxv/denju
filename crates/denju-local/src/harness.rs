use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;
use walkdir::WalkDir;

use crate::{HarnessConfig, LocalPaths};

const CODEX_MARKER: &str = ".denju-managed-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedHarnessRoots {
    pub codex_root: PathBuf,
    pub claude_root: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessEnvironment {
    pub codex_home: Option<PathBuf>,
    pub claude_config_dir: Option<PathBuf>,
}

impl HarnessEnvironment {
    pub fn current() -> Self {
        Self {
            codex_home: std::env::var_os("CODEX_HOME").map(PathBuf::from),
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        }
    }
}

pub fn resolve_harness_roots(
    paths: &LocalPaths,
    recorded: Option<&HarnessConfig>,
) -> Result<ResolvedHarnessRoots, HarnessError> {
    resolve_harness_roots_for(paths, recorded, &HarnessEnvironment::current())
}

pub fn resolve_harness_roots_for(
    paths: &LocalPaths,
    recorded: Option<&HarnessConfig>,
    environment: &HarnessEnvironment,
) -> Result<ResolvedHarnessRoots, HarnessError> {
    let codex_root = if let Some(codex_home) = &environment.codex_home {
        absolute_from_home(paths, codex_home.clone()).join("skills/denju")
    } else if let Some(recorded) = recorded {
        let recorded = PathBuf::from(&recorded.codex_root);
        if is_managed_codex_root(&recorded) {
            recorded
        } else {
            resolve_unset_codex_root(paths)?
        }
    } else {
        resolve_unset_codex_root(paths)?
    };

    let claude_config = environment
        .claude_config_dir
        .clone()
        .map(|path| absolute_from_home(paths, path))
        .unwrap_or_else(|| paths.home.join(".claude"));

    Ok(ResolvedHarnessRoots {
        codex_root,
        claude_root: claude_config.join("skills"),
    })
}

pub fn prepare_harness_roots(roots: &ResolvedHarnessRoots) -> Result<(), HarnessError> {
    fs::create_dir_all(&roots.codex_root)?;
    fs::write(roots.codex_root.join(CODEX_MARKER), b"denju-managed-v1\n")?;
    fs::create_dir_all(&roots.claude_root)?;
    Ok(())
}

pub fn remove_old_codex_projection(
    old: Option<&HarnessConfig>,
    current: &ResolvedHarnessRoots,
) -> Result<(), HarnessError> {
    let Some(old) = old else {
        return Ok(());
    };
    let old = PathBuf::from(&old.codex_root);
    if old == current.codex_root || !old.exists() {
        return Ok(());
    }
    if !is_managed_codex_root(&old) {
        return Err(HarnessError::UnmanagedOldCodexRoot(old));
    }
    fs::remove_dir_all(old)?;
    Ok(())
}

pub fn detect_unmanaged_skills(roots: &ResolvedHarnessRoots) -> Result<Vec<PathBuf>, HarnessError> {
    let mut skills = BTreeSet::new();
    if let Some(codex_skills) = roots.codex_root.parent() {
        collect_skills(codex_skills, Some(&roots.codex_root), &mut skills)?;
    }
    collect_skills(&roots.claude_root, None, &mut skills)?;
    Ok(skills.into_iter().collect())
}

fn collect_skills(
    root: &Path,
    excluded: Option<&PathBuf>,
    skills: &mut BTreeSet<PathBuf>,
) -> Result<(), HarnessError> {
    if !root.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(root).follow_links(false).max_depth(6) {
        let entry = entry.map_err(HarnessError::Walk)?;
        if entry.file_name() != "SKILL.md" || !entry.file_type().is_file() {
            continue;
        }
        let Some(parent) = entry.path().parent() else {
            continue;
        };
        if excluded.is_some_and(|excluded| parent.starts_with(excluded)) {
            continue;
        }
        skills.insert(parent.to_owned());
    }
    Ok(())
}

fn resolve_unset_codex_root(paths: &LocalPaths) -> Result<PathBuf, HarnessError> {
    let candidates = [
        paths.home.join(".agents/skills/denju"),
        paths.home.join(".codex/skills/denju"),
    ];
    let managed = candidates
        .iter()
        .filter(|candidate| is_managed_codex_root(candidate))
        .cloned()
        .collect::<Vec<_>>();
    match managed.as_slice() {
        [] => Ok(candidates[0].clone()),
        [only] => Ok(only.clone()),
        _ => Err(HarnessError::DuplicateCodexProjections(managed)),
    }
}

fn is_managed_codex_root(path: &Path) -> bool {
    path.join(CODEX_MARKER).is_file()
}

fn absolute_from_home(paths: &LocalPaths, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        paths.home.join(path)
    }
}

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("harness filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("multiple Denju-managed Codex projections exist: {0:?}")]
    DuplicateCodexProjections(Vec<PathBuf>),
    #[error("refusing to remove old Codex root without a Denju marker: {path}", path = .0.display())]
    UnmanagedOldCodexRoot(PathBuf),
    #[error("failed to scan harness skills: {0}")]
    Walk(walkdir::Error),
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn default_codex_root_is_agents_and_duplicate_markers_are_rejected() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        let roots =
            resolve_harness_roots_for(&paths, None, &HarnessEnvironment::default()).unwrap();
        assert_eq!(roots.codex_root, home.path().join(".agents/skills/denju"));

        for root in [
            home.path().join(".agents/skills/denju"),
            home.path().join(".codex/skills/denju"),
        ] {
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join(CODEX_MARKER), "managed").unwrap();
        }
        assert!(matches!(
            resolve_harness_roots_for(&paths, None, &HarnessEnvironment::default()),
            Err(HarnessError::DuplicateCodexProjections(_))
        ));
    }
}
