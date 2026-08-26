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

pub(crate) fn unique_harness_roots(roots: &ResolvedHarnessRoots) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut unique = Vec::new();
    for root in [&roots.codex_root, &roots.claude_root] {
        let resolved = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        if seen.insert(resolved.clone()) {
            unique.push(resolved);
        }
    }
    unique
}

pub(crate) fn managed_skill_storage_roots(paths: &LocalPaths) -> [PathBuf; 3] {
    [
        fs::canonicalize(&paths.skills).unwrap_or_else(|_| paths.skills.clone()),
        fs::canonicalize(&paths.generations).unwrap_or_else(|_| paths.generations.clone()),
        fs::canonicalize(&paths.derived).unwrap_or_else(|_| paths.derived.clone()),
    ]
}

pub(crate) fn is_managed_skill_target(managed_roots: &[PathBuf], path: &Path) -> bool {
    fs::canonicalize(path).is_ok_and(|resolved| {
        managed_roots
            .iter()
            .any(|managed_root| resolved.starts_with(managed_root))
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessEnvironment {
    pub claude_config_dir: Option<PathBuf>,
}

impl HarnessEnvironment {
    pub fn current() -> Self {
        Self {
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
    // Test runs intentionally ignore inherited harness overrides and recorded harness state.
    // This is a hard safety boundary: test projection I/O stays beneath the
    // explicitly marked DENJU_TEST_HOME and can never reach a developer's real harness roots.
    let roots = ResolvedHarnessRoots {
        codex_root: paths.home.join(".agents/skills"),
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
    // Codex's current user-level Agent Skills root is independent of CODEX_HOME. Project each
    // Denju-managed skill directly into the shared ~/.agents/skills root so other harnesses that
    // implement the same user-skill convention can discover the same links too. Recorded
    // legacy/custom Codex roots are intentionally not reused; callers migrate them after the
    // new root is prepared.
    let codex_root = paths.home.join(".agents/skills");

    let claude_root = if let Some(claude_config_dir) = &environment.claude_config_dir {
        absolute_from_home(paths, claude_config_dir.clone()).join("skills")
    } else if let Some(recorded) = recorded {
        PathBuf::from(&recorded.claude_root)
    } else {
        paths.home.join(".claude/skills")
    };

    Ok(ResolvedHarnessRoots {
        codex_root,
        claude_root,
    })
}

pub fn prepare_harness_roots(roots: &ResolvedHarnessRoots) -> Result<(), HarnessError> {
    fs::create_dir_all(&roots.codex_root)?;
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

pub fn detect_unmanaged_skills(
    paths: &LocalPaths,
    roots: &ResolvedHarnessRoots,
) -> Result<Vec<PathBuf>, HarnessError> {
    let managed_roots = managed_skill_storage_roots(paths);
    let mut skills = BTreeSet::new();
    for root in unique_harness_roots(roots) {
        collect_skills(&managed_roots, &root, &mut skills)?;
    }
    Ok(skills.into_iter().collect())
}

fn collect_skills(
    managed_roots: &[PathBuf],
    root: &Path,
    skills: &mut BTreeSet<PathBuf>,
) -> Result<(), HarnessError> {
    if !root.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(root).follow_links(false).max_depth(6) {
        let entry = entry.map_err(HarnessError::Walk)?;
        let skill_dir = if entry.file_name() == "SKILL.md" && entry.path().is_file() {
            let Some(parent) = entry.path().parent() else {
                continue;
            };
            parent
        } else if entry.file_type().is_symlink() && entry.path().join("SKILL.md").is_file() {
            // Flat Agent Skills roots commonly contain directory symlinks. WalkDir correctly
            // avoids traversing them, so recognize the symlink itself as a skill directory
            // without following arbitrary directory trees outside the configured root.
            entry.path()
        } else {
            continue;
        };
        // Unix symlink projections are not traversed by WalkDir, but Windows may use a native
        // junction fallback. Canonical-target filtering keeps Denju-owned links out of the
        // unmanaged-name set on both platforms.
        if is_managed_skill_target(managed_roots, skill_dir) {
            continue;
        }
        skills.insert(skill_dir.to_owned());
    }
    Ok(())
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
    fn codex_root_is_always_the_shared_agents_root() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        let legacy = home.path().join("custom-codex/skills/denju");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join(CODEX_MARKER), "managed").unwrap();
        let recorded = HarnessConfig {
            codex_root: legacy.display().to_string(),
            claude_root: home
                .path()
                .join("custom-claude/skills")
                .display()
                .to_string(),
        };
        let roots =
            resolve_harness_roots_for(&paths, Some(&recorded), &HarnessEnvironment::default())
                .unwrap();
        assert_eq!(roots.codex_root, home.path().join(".agents/skills"));
        assert_eq!(roots.claude_root, home.path().join("custom-claude/skills"));
    }

    #[test]
    fn isolated_test_roots_ignore_custom_real_harness_shapes() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        let isolated = isolated_test_harness_roots(&paths).unwrap();
        assert_eq!(isolated.codex_root, home.path().join(".agents/skills"));
        assert_eq!(isolated.claude_root, home.path().join(".claude/skills"));
        for protected_suffix in [".gg/codex", ".gg/claude", ".codex", ".claude", ".agents"] {
            let protected = PathBuf::from("/developer-home").join(protected_suffix);
            assert!(!isolated.codex_root.starts_with(&protected));
            assert!(!isolated.claude_root.starts_with(&protected));
        }
    }

    #[test]
    fn recorded_claude_root_survives_missing_service_environment() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        let codex_root = home.path().join("custom-codex/skills/denju");
        let claude_root = home.path().join("custom-claude/skills");
        let recorded = HarnessConfig {
            codex_root: codex_root.display().to_string(),
            claude_root: claude_root.display().to_string(),
        };

        let roots =
            resolve_harness_roots_for(&paths, Some(&recorded), &HarnessEnvironment::default())
                .unwrap();

        assert_eq!(roots.codex_root, home.path().join(".agents/skills"));
        assert_eq!(roots.claude_root, claude_root);
    }

    #[test]
    fn prepared_shared_codex_root_replaces_recorded_managed_legacy_root() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        let old = home.path().join("custom-codex/skills/denju");
        fs::create_dir_all(old.join("alice/review")).unwrap();
        fs::write(old.join(CODEX_MARKER), "managed").unwrap();
        let recorded = HarnessConfig {
            codex_root: old.display().to_string(),
            claude_root: home
                .path()
                .join("custom-claude/skills")
                .display()
                .to_string(),
        };
        let roots =
            resolve_harness_roots_for(&paths, Some(&recorded), &HarnessEnvironment::default())
                .unwrap();

        prepare_harness_roots(&roots).unwrap();
        remove_old_codex_projection(Some(&recorded), &roots).unwrap();

        assert!(roots.codex_root.is_dir());
        assert!(!roots.codex_root.join(CODEX_MARKER).exists());
        assert!(!old.exists());
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
