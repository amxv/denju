use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use denju_core::{ResourceId, parse_skill_document, rewrite_skill_document_name};
use denju_sync::{ManagedSkillName, allocate_projection_names};
use thiserror::Error;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    LocalDatabase, LocalDbError, LocalPaths, ResolvedHarnessRoots, SubscriptionRecord,
    create_native_directory_link, detect_unmanaged_skills,
    materialize::{MaterializationError, remove_owned_link},
};

/// Rebuild both harness views from local desired state. Old managed invocation links are
/// removed before new aliases are exposed, so a collision transition can temporarily hide
/// a skill but can never expose two resources under the same invocation name.
pub async fn reconcile_harness_projections(
    paths: &LocalPaths,
    db: &LocalDatabase,
    roots: &ResolvedHarnessRoots,
) -> Result<Vec<(String, String)>, ProjectionError> {
    let subscriptions = db.subscriptions().await?;
    let reserved = unmanaged_names(roots)?;
    let mut managed = Vec::new();
    for record in &subscriptions {
        if record.materialized_revision_id.is_none() {
            continue;
        }
        managed.push(ManagedSkillName {
            resource_id: ResourceId::from_str(&record.resource_id)
                .map_err(|error| ProjectionError::Corrupt(error.to_string()))?,
            owner: record.owner.clone(),
            skill_name: record.skill_name.clone(),
        });
    }
    let assignments = allocate_projection_names(&managed, &reserved);

    // Remove only invocation paths whose desired name changed. Collision transitions are
    // fail-closed: every old canonical invocation is removed before any new aliases appear,
    // while unchanged links remain continuously visible across background sync cycles.
    for record in &subscriptions {
        let desired = assignments
            .iter()
            .find(|assignment| assignment.resource_id.to_string() == record.resource_id)
            .map(|assignment| assignment.harness_name.as_str());
        if record
            .harness_name
            .as_deref()
            .is_some_and(|current| Some(current) != desired)
        {
            remove_subscription_projection(paths, roots, record)?;
        }
    }

    let mut projected = Vec::with_capacity(assignments.len());

    for assignment in assignments {
        let record = subscriptions
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
        let target = if assignment.derived {
            derived_view(paths, record, &canonical, &assignment.harness_name)?
        } else {
            canonical
        };

        let codex = roots
            .codex_root
            .join(&record.owner)
            .join(&assignment.harness_name);
        create_projection_link(paths, &target, &codex)?;
        let claude = roots.claude_root.join(&assignment.harness_name);
        create_projection_link(paths, &target, &claude)?;
        db.set_subscription_harness_name(
            record.resource_id.clone(),
            assignment.harness_name.clone(),
            now_unix_ms(),
        )
        .await?;
        projected.push((record.locator.clone(), assignment.harness_name));
    }
    Ok(projected)
}

pub fn remove_subscription_projection(
    paths: &LocalPaths,
    roots: &ResolvedHarnessRoots,
    record: &SubscriptionRecord,
) -> Result<(), ProjectionError> {
    let Some(harness_name) = record.harness_name.as_deref() else {
        return Ok(());
    };
    let codex = roots.codex_root.join(&record.owner).join(harness_name);
    remove_managed_projection_link(paths, &codex)?;
    if let Some(owner_dir) = codex.parent() {
        let _ = fs::remove_dir(owner_dir);
    }
    remove_managed_projection_link(paths, &roots.claude_root.join(harness_name))?;
    Ok(())
}

fn unmanaged_names(roots: &ResolvedHarnessRoots) -> Result<BTreeSet<String>, ProjectionError> {
    let mut names = BTreeSet::new();
    for skill_dir in detect_unmanaged_skills(roots)? {
        if let Some(name) = skill_dir.file_name().and_then(|name| name.to_str()) {
            names.insert(name.to_owned());
        }
    }
    Ok(names)
}

fn derived_view(
    paths: &LocalPaths,
    record: &SubscriptionRecord,
    canonical: &Path,
    harness_name: &str,
) -> Result<PathBuf, ProjectionError> {
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
        return Ok(root);
    }

    let source = fs::canonicalize(canonical)?;
    let parent = root
        .parent()
        .ok_or_else(|| ProjectionError::Corrupt("derived root has no parent".to_owned()))?;
    fs::create_dir_all(parent)?;
    let stage = parent.join(format!(".stage-{}", Uuid::now_v7()));
    copy_derived_tree(&source, &stage, &record.skill_name, harness_name)?;
    fs::rename(&stage, &root)?;
    Ok(root)
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
            } else if fs::hard_link(item.path(), &target).is_err() {
                // This is an explicit collision-derived generation, not a harness projection
                // fallback. Cross-device files may be materialized when hard-linking is impossible.
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
    #[error("failed to copy derived view: {0}")]
    Walk(walkdir::Error),
    #[error("projection state is corrupt: {0}")]
    Corrupt(String),
    #[error("refusing to overwrite unmanaged projection path {path}", path = .0.display())]
    RefuseOverwrite(PathBuf),
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use denju_core::ResourceId;
    use tempfile::tempdir;

    use super::*;
    use crate::{ensure_local_layout, prepare_harness_roots};

    fn skill_document(name: &str, owner: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: Reviews code for {owner}.\nmetadata:\n  owner: {owner}\n---\n# Review\n"
        )
    }

    async fn insert_materialized(
        db: &LocalDatabase,
        paths: &LocalPaths,
        id: &str,
        owner: &str,
        name: &str,
        revision: &str,
    ) {
        let resource_id = ResourceId::from_str(id).unwrap();
        let canonical = paths.skills.join(owner).join(name);
        fs::create_dir_all(&canonical).unwrap();
        fs::write(canonical.join("SKILL.md"), skill_document(name, owner)).unwrap();
        db.upsert_subscription_desired(
            SubscriptionRecord {
                resource_id: resource_id.to_string(),
                locator: format!("@{owner}/{name}"),
                owner: owner.to_owned(),
                skill_name: name.to_owned(),
                resource_generation: 1,
                release_version: 1,
                desired_revision_id: revision.to_owned(),
                harness_name: None,
                materialized_revision_id: None,
            },
            1,
        )
        .await
        .unwrap();
        db.mark_subscription_materialized(resource_id.to_string(), revision.to_owned(), 2)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn same_agent_skills_name_gets_stable_derived_aliases_in_both_harnesses() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        ensure_local_layout(&paths).unwrap();
        let roots = ResolvedHarnessRoots {
            codex_root: home.path().join(".agents/skills/denju"),
            claude_root: home.path().join(".claude/skills"),
        };
        prepare_harness_roots(&roots).unwrap();
        let db = LocalDatabase::open(&paths.state_db).await.unwrap();
        insert_materialized(
            &db,
            &paths,
            "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
            "alice",
            "review",
            "1111111111111111111111111111111111111111111111111111111111111111",
        )
        .await;
        insert_materialized(
            &db,
            &paths,
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
            "bob",
            "review",
            "2222222222222222222222222222222222222222222222222222222222222222",
        )
        .await;

        let first = reconcile_harness_projections(&paths, &db, &roots)
            .await
            .unwrap();
        assert_eq!(first.len(), 2);
        assert!(
            first
                .iter()
                .all(|(_, name)| name != "review" && name.len() <= 64)
        );
        for (locator, harness_name) in &first {
            let owner = locator.trim_start_matches('@').split_once('/').unwrap().0;
            let claude = roots.claude_root.join(harness_name);
            let codex = roots.codex_root.join(owner).join(harness_name);
            assert!(
                fs::symlink_metadata(&claude)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert!(
                fs::symlink_metadata(&codex)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            let projected = fs::read(claude.join("SKILL.md")).unwrap();
            parse_skill_document(harness_name, &projected).unwrap();
        }

        let second = reconcile_harness_projections(&paths, &db, &roots)
            .await
            .unwrap();
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn projection_cleanup_refuses_user_replacement() {
        let home = tempdir().unwrap();
        let paths = LocalPaths::from_home(home.path().to_owned());
        ensure_local_layout(&paths).unwrap();
        let roots = ResolvedHarnessRoots {
            codex_root: home.path().join(".agents/skills/denju"),
            claude_root: home.path().join(".claude/skills"),
        };
        prepare_harness_roots(&roots).unwrap();
        let outside = home.path().join("user-skill");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("SKILL.md"), skill_document("review", "user")).unwrap();
        let link = roots.claude_root.join("review");
        create_native_directory_link(&outside, &link).unwrap();
        let record = SubscriptionRecord {
            resource_id: "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1".to_owned(),
            locator: "@alice/review".to_owned(),
            owner: "alice".to_owned(),
            skill_name: "review".to_owned(),
            resource_generation: 1,
            release_version: 1,
            desired_revision_id: "11".repeat(32),
            harness_name: Some("review".to_owned()),
            materialized_revision_id: Some("11".repeat(32)),
        };
        let error = remove_subscription_projection(&paths, &roots, &record).unwrap_err();
        assert!(matches!(error, ProjectionError::RefuseOverwrite(_)));
        assert!(fs::symlink_metadata(link).is_ok());
    }
}
