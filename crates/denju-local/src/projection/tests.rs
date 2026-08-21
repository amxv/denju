use std::str::FromStr;

use denju_core::{OwnedSkillEntry, ResourceId, RevisionId, build_deterministic_skill_snapshot};
use tempfile::tempdir;

use super::*;
use crate::{
    DesiredSkillMaterialization, OwnedSkillRecord, ensure_local_layout, materialize_skill_snapshot,
    prepare_harness_roots,
};

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
            retain_on_delete: false,
            retained_after_delete: false,
        },
        1,
    )
    .await
    .unwrap();
    db.mark_skill_materialized(resource_id.to_string(), revision.to_owned(), 2)
        .await
        .unwrap();
}

async fn insert_owned_materialized(
    db: &LocalDatabase,
    paths: &LocalPaths,
    id: &str,
    owner: &str,
    name: &str,
    revision_byte: u8,
) {
    let resource_id = ResourceId::from_str(id).unwrap();
    let revision_id = RevisionId::from_bytes([revision_byte; 32]);
    let entries = vec![
        OwnedSkillEntry::File {
            path: "SKILL.md".into(),
            bytes: skill_document(name, owner).into_bytes(),
            executable: false,
        },
        OwnedSkillEntry::File {
            path: "notes.txt".into(),
            bytes: format!("notes for {owner}\n").into_bytes(),
            executable: false,
        },
    ];
    let snapshot = build_deterministic_skill_snapshot(name, &entries).unwrap();
    db.upsert_owned_skill_desired(
        OwnedSkillRecord {
            resource_id: resource_id.to_string(),
            locator: format!("@{owner}/{name}"),
            owner: owner.to_owned(),
            skill_name: name.to_owned(),
            resource_generation: 1,
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
            owner: owner.to_owned(),
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
async fn owned_collision_view_tracks_edit_direction_without_feedback_loop() {
    let home = tempdir().unwrap();
    let paths = LocalPaths::from_home(home.path().to_owned());
    ensure_local_layout(&paths).unwrap();
    let roots = ResolvedHarnessRoots {
        codex_root: home.path().join(".agents/skills/denju"),
        claude_root: home.path().join(".claude/skills"),
    };
    prepare_harness_roots(&roots).unwrap();
    let db = LocalDatabase::open(&paths.state_db).await.unwrap();
    insert_owned_materialized(
        &db,
        &paths,
        "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
        "alice",
        "review",
        1,
    )
    .await;
    insert_owned_materialized(
        &db,
        &paths,
        "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
        "bob",
        "review",
        2,
    )
    .await;
    reconcile_harness_projections(&paths, &db, &roots)
        .await
        .unwrap();
    let alice = db
        .owned_skills()
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.owner == "alice")
        .unwrap();
    let harness = alice.harness_name.clone().unwrap();
    assert_ne!(harness, "review");
    let derived = paths.derived.join(&alice.resource_id).join(format!(
        "{}-{harness}",
        alice.materialized_revision_id.as_deref().unwrap()
    ));

    fs::write(derived.join("notes.txt"), b"edited through derived\n").unwrap();
    let canonical = paths.skills.join("alice/review");
    assert_eq!(
        fs::read(canonical.join("notes.txt")).unwrap(),
        b"notes for alice\n"
    );
    assert!(
        reconcile_owned_derived_projection(&paths, &db, &alice)
            .await
            .unwrap()
    );
    assert_eq!(
        fs::read(canonical.join("notes.txt")).unwrap(),
        b"edited through derived\n"
    );

    fs::write(derived.join("from-derived.txt"), b"derived\n").unwrap();
    assert!(
        reconcile_owned_derived_projection(&paths, &db, &alice)
            .await
            .unwrap()
    );
    assert_eq!(
        fs::read(canonical.join("from-derived.txt")).unwrap(),
        b"derived\n"
    );
    let canonical_skill = fs::read(canonical.join("SKILL.md")).unwrap();
    parse_skill_document("review", &canonical_skill).unwrap();

    fs::write(canonical.join("from-canonical.txt"), b"canonical\n").unwrap();
    assert!(
        !reconcile_owned_derived_projection(&paths, &db, &alice)
            .await
            .unwrap()
    );
    assert_eq!(
        fs::read(derived.join("from-canonical.txt")).unwrap(),
        b"canonical\n"
    );

    fs::write(canonical.join("canonical-only.txt"), b"a\n").unwrap();
    fs::write(derived.join("derived-only.txt"), b"b\n").unwrap();
    let error = reconcile_owned_derived_projection(&paths, &db, &alice)
        .await
        .unwrap_err();
    assert!(matches!(error, ProjectionError::DivergedDerivedEdit(_)));
}

async fn assert_writeback_recovery(interrupted_at: JournalState) {
    let home = tempdir().unwrap();
    let paths = LocalPaths::from_home(home.path().to_owned());
    ensure_local_layout(&paths).unwrap();
    let db = LocalDatabase::open(&paths.state_db).await.unwrap();
    let resource_id = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1";
    insert_owned_materialized(&db, &paths, resource_id, "alice", "review", 1).await;

    let target_entries = vec![
        OwnedSkillEntry::File {
            path: "SKILL.md".into(),
            bytes: skill_document("review", "alice").into_bytes(),
            executable: false,
        },
        OwnedSkillEntry::File {
            path: "notes.txt".into(),
            bytes: b"recovered writeback\n".to_vec(),
            executable: false,
        },
    ];
    let target = build_deterministic_skill_snapshot("review", &target_entries).unwrap();
    let operation = OperationId::from_uuid(Uuid::now_v7()).unwrap();
    let resource_root = paths.generations.join(resource_id);
    let stage = resource_root.join(format!(".writeback-{operation}"));
    let generation = resource_root.join(format!("workspace-{operation}"));
    let canonical = paths.skills.join("alice/review");
    let payload = WorkspaceWritebackJournalPayload {
        resource_id: resource_id.to_owned(),
        skill_name: "review".to_owned(),
        harness_name: "denju-alice-review-a1b2c3".to_owned(),
        target_root_tree_id: target.manifest().root_tree().to_string(),
        stage_dir: stage.display().to_string(),
        generation_dir: generation.display().to_string(),
        canonical_path: canonical.display().to_string(),
    };
    db.create_workspace_writeback_journal(operation, payload, 10)
        .await
        .unwrap();

    write_generation(&paths, &stage, &target_entries).unwrap();
    if interrupted_at != JournalState::Planned {
        db.update_workspace_writeback_journal(
            operation,
            JournalState::Planned,
            JournalState::Staged,
            11,
        )
        .await
        .unwrap();
    }
    if matches!(
        interrupted_at,
        JournalState::Verified | JournalState::Switched
    ) {
        fs::rename(&stage, &generation).unwrap();
        db.update_workspace_writeback_journal(
            operation,
            JournalState::Staged,
            JournalState::Verified,
            12,
        )
        .await
        .unwrap();
    }
    if interrupted_at == JournalState::Switched {
        atomic_switch_directory_link(&generation, &canonical, operation).unwrap();
        db.update_workspace_writeback_journal(
            operation,
            JournalState::Verified,
            JournalState::Switched,
            13,
        )
        .await
        .unwrap();
    }

    recover_workspace_writebacks(&paths, &db).await.unwrap();
    assert!(db.workspace_writeback_journals().await.unwrap().is_empty());
    if interrupted_at == JournalState::Planned {
        assert_eq!(
            fs::read(canonical.join("notes.txt")).unwrap(),
            b"notes for alice\n"
        );
        assert!(!stage.exists());
    } else {
        assert_eq!(
            fs::read(canonical.join("notes.txt")).unwrap(),
            b"recovered writeback\n"
        );
        let state = db
            .workspace_state(resource_id.to_owned())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state.working_generation_path,
            generation.display().to_string()
        );
        let baseline = db
            .derived_projection_state(resource_id.to_owned())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            baseline.baseline_root_tree_id,
            target.manifest().root_tree().to_string()
        );
    }
}

#[tokio::test]
async fn workspace_writeback_recovers_each_interruption_boundary() {
    for state in [
        JournalState::Planned,
        JournalState::Staged,
        JournalState::Verified,
        JournalState::Switched,
    ] {
        assert_writeback_recovery(state).await;
    }
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
        retain_on_delete: false,
        retained_after_delete: false,
    };
    let error = remove_subscription_projection(&paths, &roots, &record).unwrap_err();
    assert!(matches!(error, ProjectionError::RefuseOverwrite(_)));
    assert!(fs::symlink_metadata(link).is_ok());
}
