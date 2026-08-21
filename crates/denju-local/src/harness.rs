use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;
use walkdir::WalkDir;

use crate::{HarnessConfig, LocalPaths, TEST_HOME_ENV};

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
    if std::env::var_os(TEST_HOME_ENV).is_some() {
        return isolated_test_harness_roots(paths);
    }
    resolve_harness_roots_for(paths, recorded, &HarnessEnvironment::current())
}

fn isolated_test_harness_roots(paths: &LocalPaths) -> Result<ResolvedHarnessRoots, HarnessError> {
    // Test runs intentionally ignore inherited CODEX_HOME/CLAUDE_CONFIG_DIR and recorded
    // harness state. This is a hard safety boundary: test projection I/O stays beneath the
    // explicitly marked DENJU_TEST_HOME and can never reach a developer's real harness roots.
    let roots = ResolvedHarnessRoots {
        codex_root: paths.home.join(".agents/skills/denju"),
        claude_root: paths.home.join(".claude/skills"),
    };
    validate_isolated_test_root(&paths.home, &roots.codex_root)?;
    validate_isolated_test_root(&paths.home, &roots.claude_root)?;
    Ok(roots)
}

fn validate_isolated_test_root(home: &Path, root: &Path) -> Result<(), HarnessError> {
    let relative = root
        .strip_prefix(home)
        .map_err(|_| HarnessError::UnsafeTestHarnessRoot {
            path: root.to_owned(),
            reason: "outside DENJU_TEST_HOME".to_owned(),
        })?;
    let mut cursor = home.to_owned();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(HarnessError::UnsafeTestHarnessRoot {
                path: root.to_owned(),
                reason: "contains a non-normal path component".to_owned(),
            });
        };
        cursor.push(component);
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(HarnessError::UnsafeTestHarnessRoot {
                    path: root.to_owned(),
                    reason: format!("test harness ancestor is a symlink: {}", cursor.display()),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(HarnessError::Io(error)),
        }
    }
    Ok(())
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
    #[error("unsafe isolated test harness root {path}: {reason}", path = path.display())]
    UnsafeTestHarnessRoot { path: PathBuf, reason: String },
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

    #[test]
    fn isolated_test_roots_ignore_custom_real_harness_shapes() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        let isolated = isolated_test_harness_roots(&paths).unwrap();
        assert_eq!(
            isolated.codex_root,
            home.path().join(".agents/skills/denju")
        );
        assert_eq!(isolated.claude_root, home.path().join(".claude/skills"));
        for protected_suffix in [".gg/codex", ".gg/claude", ".codex", ".claude", ".agents"] {
            let protected = PathBuf::from("/developer-home").join(protected_suffix);
            assert!(!isolated.codex_root.starts_with(&protected));
            assert!(!isolated.claude_root.starts_with(&protected));
        }
    }

    #[cfg(unix)]
    #[test]
    fn isolated_test_roots_reject_symlink_escape() {
        let home = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join(".agents")).unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        assert!(matches!(
            isolated_test_harness_roots(&paths),
            Err(HarnessError::UnsafeTestHarnessRoot { .. })
        ));
    }
}
