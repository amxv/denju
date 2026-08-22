use std::{fs, str::FromStr};

use denju_core::{
    OperationId, OwnedSkillEntry, ResourceId, RevisionId, build_deterministic_skill_snapshot,
};
use tempfile::tempdir;
use uuid::Uuid;

use super::*;
use crate::{
    DesiredSkillMaterialization, ensure_local_layout, materialize_skill_snapshot,
    prepare_harness_roots,
};

fn skill_snapshot(name: &str) -> denju_core::DeterministicSkillSnapshot {
    build_deterministic_skill_snapshot(
        name,
        &[
            OwnedSkillEntry::File {
                path: "SKILL.md".to_owned(),
                bytes: format!(
                    "---\nname: {name}\ndescription: Lifecycle recovery fixture.\n---\n# {name}\n"
                )
                .into_bytes(),
                executable: false,
            },
            OwnedSkillEntry::File {
                path: "notes.txt".to_owned(),
                bytes: b"durable user bytes\n".to_vec(),
                executable: false,
            },
        ],
    )
    .unwrap()
}

async fn seed_owned(
    paths: &LocalPaths,
    db: &LocalDatabase,
    roots: &ResolvedHarnessRoots,
    resource_id: ResourceId,
    name: &str,
    revision_id: RevisionId,
) -> ManagedSkillRecord {
    let snapshot = skill_snapshot(name);
    db.upsert_owned_skill_desired(
        OwnedSkillRecord {
            resource_id: resource_id.to_string(),
            locator: format!("@alice/{name}"),
            owner: "alice".to_owned(),
            skill_name: name.to_owned(),
            resource_generation: 1,
            workspace_generation: 1,
            desired_revision_id: revision_id.to_string(),
            harness_name: None,
            materialized_revision_id: None,
        },
        1,
    )
    .await
    .unwrap();
    let generation = materialize_skill_snapshot(
        paths,
        db,
        &DesiredSkillMaterialization {
            resource_id,
            owner: "alice".to_owned(),
            skill_name: name.to_owned(),
            revision_id,
            manifest: snapshot.manifest().clone(),
        },
        snapshot.bytes(),
    )
    .await
    .unwrap();
    db.ensure_workspace_baseline(
        resource_id.to_string(),
        1,
        revision_id.to_string(),
        snapshot.manifest().root_tree().to_string(),
        generation.display().to_string(),
        2,
    )
    .await
    .unwrap();
    reconcile_harness_projections(paths, db, roots)
        .await
        .unwrap();
    db.managed_skills()
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.resource_id == resource_id.to_string())
        .unwrap()
}

fn roots(home: &std::path::Path) -> ResolvedHarnessRoots {
    ResolvedHarnessRoots {
        codex_root: home.join(".agents/skills/denju"),
        claude_root: home.join(".claude/skills"),
    }
}

#[tokio::test]
async fn clean_registry_rename_stages_authoritative_generation_before_switching() {
    let home = tempdir().unwrap();
    let paths = LocalPaths::from_home(home.path().to_owned());
    ensure_local_layout(&paths).unwrap();
    let roots = roots(home.path());
    prepare_harness_roots(&roots).unwrap();
    let db = LocalDatabase::open(&paths.state_db).await.unwrap();
    let resource_id = ResourceId::from_str("01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1").unwrap();
    let old_revision = RevisionId::from_bytes([1; 32]);
    let old = seed_owned(&paths, &db, &roots, resource_id, "review", old_revision).await;
    let renamed = skill_snapshot("renamed");
    let renamed_revision = RevisionId::from_bytes([2; 32]);

    apply_registry_rename(
        &paths,
        &db,
        &roots,
        &old,
        RegistryRenameState {
            resource_id: resource_id.to_string(),
            owner: "alice".to_owned(),
            name: "renamed".to_owned(),
            locator: "@alice/renamed".to_owned(),
            resource_generation: 2,
            workspace_generation: 2,
            revision_id: renamed_revision.to_string(),
            root_tree_id: renamed.manifest().root_tree().to_string(),
        },
        false,
        Some((renamed.manifest(), renamed.bytes())),
    )
    .await
    .unwrap();

    assert!(!paths.skills.join("alice/review").exists());
    let renamed_path = paths.skills.join("alice/renamed");
    assert!(renamed_path.exists());
    let skill = fs::read_to_string(renamed_path.join("SKILL.md")).unwrap();
    assert!(skill.contains("name: renamed"));
    assert_eq!(
        fs::read(renamed_path.join("notes.txt")).unwrap(),
        b"durable user bytes\n"
    );
    let owned = db.owned_skills().await.unwrap();
    assert_eq!(owned.len(), 1);
    assert_eq!(owned[0].resource_id, resource_id.to_string());
    assert_eq!(owned[0].locator, "@alice/renamed");
    assert_eq!(owned[0].desired_revision_id, renamed_revision.to_string());
    assert!(db.local_lifecycle_journals().await.unwrap().is_empty());
}

#[tokio::test]
async fn interrupted_rename_recovers_from_verified_boundary() {
    let home = tempdir().unwrap();
    let paths = LocalPaths::from_home(home.path().to_owned());
    ensure_local_layout(&paths).unwrap();
    let roots = roots(home.path());
    prepare_harness_roots(&roots).unwrap();
    let db = LocalDatabase::open(&paths.state_db).await.unwrap();
    let resource_id = ResourceId::from_str("01890f47-6a1d-7ad0-8f43-9a4d8c29f002").unwrap();
    let old_revision = RevisionId::from_bytes([3; 32]);
    let old = seed_owned(&paths, &db, &roots, resource_id, "review", old_revision).await;
    let renamed = skill_snapshot("renamed");
    let renamed_revision = RevisionId::from_bytes([4; 32]);
    let staged = stage_skill_generation(
        &paths,
        &DesiredSkillMaterialization {
            resource_id,
            owner: "alice".to_owned(),
            skill_name: "renamed".to_owned(),
            revision_id: renamed_revision,
            manifest: renamed.manifest().clone(),
        },
        renamed.bytes(),
        OperationId::from_uuid(Uuid::now_v7()).unwrap(),
    )
    .unwrap();
    let operation_id = OperationId::from_uuid(Uuid::now_v7()).unwrap();
    db.create_local_lifecycle_journal(
        operation_id,
        LocalLifecyclePayload::Rename {
            resource_id: resource_id.to_string(),
            old_owner: old.owner.clone(),
            old_name: old.skill_name.clone(),
            old_harness_name: old.harness_name.clone(),
            new_owner: "alice".to_owned(),
            new_name: "renamed".to_owned(),
            new_locator: "@alice/renamed".to_owned(),
            remote_resource_generation: 2,
            remote_workspace_generation: 2,
            remote_revision_id: renamed_revision.to_string(),
            remote_root_tree_id: renamed.manifest().root_tree().to_string(),
            working_generation_path: staged.display().to_string(),
            preserve_working: false,
        },
    )
    .await
    .unwrap();

    let new_canonical = paths.skills.join("alice/renamed");
    ensure_canonical_link(&staged, &new_canonical, operation_id).unwrap();
    db.advance_local_lifecycle(operation_id, JournalState::Planned)
        .await
        .unwrap();

    let old_harness = old.harness_name.as_deref().unwrap();
    let blocker = roots.codex_root.join("alice").join(old_harness);
    remove_link(&blocker);
    fs::create_dir_all(&blocker).unwrap();
    fs::write(blocker.join("sentinel"), b"unmanaged").unwrap();

    let error = recover_local_lifecycle(&paths, &db, &roots)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("projection"));
    assert_eq!(
        db.local_lifecycle_state(operation_id).await.unwrap(),
        JournalState::Verified
    );
    assert_eq!(
        db.owned_skills().await.unwrap()[0].locator,
        "@alice/renamed"
    );
    assert!(paths.skills.join("alice/review").exists());
    assert!(new_canonical.exists());

    fs::remove_dir_all(&blocker).unwrap();
    recover_local_lifecycle(&paths, &db, &roots).await.unwrap();
    assert!(!paths.skills.join("alice/review").exists());
    assert!(new_canonical.exists());
    assert!(db.local_lifecycle_journals().await.unwrap().is_empty());
    assert_eq!(
        fs::read(new_canonical.join("notes.txt")).unwrap(),
        b"durable user bytes\n"
    );
}

#[tokio::test]
async fn interrupted_removal_recovers_without_reintroducing_desired_state() {
    let home = tempdir().unwrap();
    let paths = LocalPaths::from_home(home.path().to_owned());
    ensure_local_layout(&paths).unwrap();
    let roots = roots(home.path());
    prepare_harness_roots(&roots).unwrap();
    let db = LocalDatabase::open(&paths.state_db).await.unwrap();
    let resource_id = ResourceId::from_str("01890f47-6a1e-72ce-88bf-ef23fc661004").unwrap();
    let revision = RevisionId::from_bytes([5; 32]);
    let record = seed_owned(&paths, &db, &roots, resource_id, "review", revision).await;
    let operation_id = OperationId::from_uuid(Uuid::now_v7()).unwrap();
    db.create_local_lifecycle_journal(
        operation_id,
        LocalLifecyclePayload::Remove {
            resource_id: resource_id.to_string(),
            owner: "alice".to_owned(),
            skill_name: "review".to_owned(),
            harness_name: record.harness_name.clone(),
            desired_kind: ManagedDesiredKind::Owned,
        },
    )
    .await
    .unwrap();

    remove_managed_skill_projection(&paths, &roots, &record).unwrap();
    db.advance_local_lifecycle(operation_id, JournalState::Planned)
        .await
        .unwrap();
    let canonical = paths.skills.join("alice/review");
    remove_link(&canonical);
    fs::create_dir_all(&canonical).unwrap();
    fs::write(canonical.join("sentinel"), b"unmanaged").unwrap();

    assert!(recover_local_lifecycle(&paths, &db, &roots).await.is_err());
    assert_eq!(
        db.local_lifecycle_state(operation_id).await.unwrap(),
        JournalState::Staged
    );
    assert_eq!(db.owned_skills().await.unwrap().len(), 1);

    fs::remove_dir_all(&canonical).unwrap();
    recover_local_lifecycle(&paths, &db, &roots).await.unwrap();
    assert!(db.owned_skills().await.unwrap().is_empty());
    assert!(db.managed_skills().await.unwrap().is_empty());
    assert!(db.local_lifecycle_journals().await.unwrap().is_empty());
}

#[tokio::test]
async fn pack_detach_preserves_captured_edit_then_allows_authoritative_rebuild() {
    let home = tempdir().unwrap();
    let paths = LocalPaths::from_home(home.path().to_owned());
    ensure_local_layout(&paths).unwrap();
    let roots = roots(home.path());
    prepare_harness_roots(&roots).unwrap();
    let db = LocalDatabase::open(&paths.state_db).await.unwrap();
    let resource_id = ResourceId::from_str("01890f47-6a1f-7cc2-98c1-5f6c1ed8a3a1").unwrap();
    let revision = RevisionId::from_bytes([6; 32]);
    let snapshot = skill_snapshot("review");
    let resource_id_text = resource_id.to_string();
    let revision_text = revision.to_string();
    let root_tree = snapshot.manifest().root_tree().to_string();
    db.call({
        let resource_id_text = resource_id_text.clone();
        let revision_text = revision_text.clone();
        let root_tree = root_tree.clone();
        move |connection| {
            connection.execute(
                "INSERT INTO pack_materialized_skills \
                 (resource_id,locator,owner,skill_name,resource_generation,desired_revision_id,desired_root_tree_id,harness_name,materialized_revision_id,updated_at_unix_ms) \
                 VALUES (?1,'@alice/review','alice','review',1,?2,?3,NULL,?2,1)",
                rusqlite::params![resource_id_text, revision_text, root_tree],
            )?;
            Ok(())
        }
    })
    .await
    .unwrap();
    let generation = materialize_skill_snapshot(
        &paths,
        &db,
        &DesiredSkillMaterialization {
            resource_id,
            owner: "alice".to_owned(),
            skill_name: "review".to_owned(),
            revision_id: revision,
            manifest: snapshot.manifest().clone(),
        },
        snapshot.bytes(),
    )
    .await
    .unwrap();
    reconcile_harness_projections(&paths, &db, &roots)
        .await
        .unwrap();
    fs::write(generation.join("notes.txt"), b"captured enforced edit\n").unwrap();
    let record = db
        .managed_skills()
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.resource_id == resource_id_text)
        .unwrap();

    journaled_remove_managed_skill(&paths, &db, &roots, &record, ManagedDesiredKind::Pack)
        .await
        .unwrap();
    assert!(!paths.skills.join("alice/review").exists());
    assert!(db.pack_materialized_skills().await.unwrap().is_empty());
    assert_eq!(
        fs::read(generation.join("notes.txt")).unwrap(),
        b"captured enforced edit\n"
    );

    let rebuilt = stage_skill_generation(
        &paths,
        &DesiredSkillMaterialization {
            resource_id,
            owner: "alice".to_owned(),
            skill_name: "review".to_owned(),
            revision_id: revision,
            manifest: snapshot.manifest().clone(),
        },
        snapshot.bytes(),
        OperationId::from_uuid(Uuid::now_v7()).unwrap(),
    )
    .unwrap();
    assert_eq!(rebuilt, generation);
    assert_eq!(
        fs::read(rebuilt.join("notes.txt")).unwrap(),
        b"durable user bytes\n"
    );
}

fn remove_link(path: &std::path::Path) {
    if fs::symlink_metadata(path).is_err() {
        return;
    }
    #[cfg(unix)]
    fs::remove_file(path).unwrap();
    #[cfg(windows)]
    fs::remove_dir(path).unwrap();
}
