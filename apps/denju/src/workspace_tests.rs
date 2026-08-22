use denju_core::{OwnedSkillEntry, ResourceId, RevisionId, build_deterministic_skill_snapshot};
use denju_local::{
    DesiredSkillMaterialization, IdentityRecord, LocalPaths, OwnedSkillRecord, WorkspaceStatus,
    ensure_local_layout, materialize_skill_snapshot,
};
use tempfile::TempDir;

use super::*;

async fn fixture() -> (TempDir, LocalPaths, LocalDatabase, OwnedSkillRecord) {
    let home = tempfile::tempdir().unwrap();
    let paths = LocalPaths::from_home(home.path().to_owned());
    ensure_local_layout(&paths).unwrap();
    let db = LocalDatabase::open(&paths.state_db).await.unwrap();
    db.save_identity(
        IdentityRecord {
            user_id: "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a2".into(),
            namespace_id: "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a3".into(),
            username: "@alice".into(),
            session_id: Some("session".into()),
            session_backend: Some("file".into()),
            author_principal_id: Some("01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a4".into()),
        },
        1,
    )
    .await
    .unwrap();
    let resource_id = ResourceId::from_str("01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1").unwrap();
    let revision_id = RevisionId::from_bytes([7; 32]);
    let entries = vec![
        OwnedSkillEntry::File {
            path: "SKILL.md".into(),
            bytes: skill_document("review").into_bytes(),
            executable: false,
        },
        OwnedSkillEntry::File {
            path: "notes.txt".into(),
            bytes: b"base\n".to_vec(),
            executable: false,
        },
    ];
    let snapshot = build_deterministic_skill_snapshot("review", &entries).unwrap();
    let record = OwnedSkillRecord {
        resource_id: resource_id.to_string(),
        locator: "@alice/review".into(),
        owner: "alice".into(),
        skill_name: "review".into(),
        resource_generation: 1,
        workspace_generation: 1,
        desired_revision_id: revision_id.to_string(),
        harness_name: None,
        materialized_revision_id: None,
    };
    db.upsert_owned_skill_desired(record.clone(), 1)
        .await
        .unwrap();
    let generation = materialize_skill_snapshot(
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
    db.ensure_workspace_baseline(
        record.resource_id.clone(),
        1,
        revision_id.to_string(),
        snapshot.manifest().root_tree().to_string(),
        generation.display().to_string(),
        2,
    )
    .await
    .unwrap();
    let record = db.owned_skills().await.unwrap().remove(0);
    (home, paths, db, record)
}

fn skill_document(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Reviews code safely.\n---\n# Review\n")
}

#[tokio::test]
async fn coherent_edit_queues_exactly_one_revision() {
    let (_home, paths, db, _record) = fixture().await;
    fs::write(paths.skills.join("alice/review/notes.txt"), b"changed\n").unwrap();

    let (first, blockers) = capture_local_edits(&paths, &db, false).await.unwrap();
    assert!(blockers.is_empty());
    assert_eq!(first.queued, 1);
    assert_eq!(db.queued_local_revisions().await.unwrap().len(), 1);

    let (second, blockers) = capture_local_edits(&paths, &db, false).await.unwrap();
    assert!(blockers.is_empty());
    assert_eq!(second.queued, 0);
    assert_eq!(db.queued_local_revisions().await.unwrap().len(), 1);
}

#[tokio::test]
async fn metadata_generation_advance_does_not_rewrite_workspace_cas_state() {
    let (_home, paths, db, record) = fixture().await;
    fs::write(paths.skills.join("alice/review/notes.txt"), b"changed\n").unwrap();
    let (_pass, blockers) = capture_local_edits(&paths, &db, false).await.unwrap();
    assert!(blockers.is_empty());
    let before = db.queued_local_revisions().await.unwrap().remove(0);
    assert_eq!(before.expected_generation, 1);

    db.pause_workspace(
        record.resource_id.clone(),
        WorkspaceStatus::Quota,
        "quota".into(),
        None,
        3,
    )
    .await
    .unwrap();
    db.advance_owned_metadata_generation(record.resource_id.clone(), 1, 2, 4)
        .await
        .unwrap();

    let after = db.queued_local_revisions().await.unwrap().remove(0);
    assert_eq!(after.operation_id, before.operation_id);
    assert_eq!(after.revision_id, before.revision_id);
    assert_eq!(
        after.expected_head_revision_id,
        before.expected_head_revision_id
    );
    assert_eq!(after.parent_revision_ids, before.parent_revision_ids);
    assert_eq!(after.expected_generation, 1);
    let state = db
        .workspace_state(record.resource_id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.base_generation, 1);
    assert_eq!(state.status, WorkspaceStatus::Quota);
    let owned = db
        .owned_skills()
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.resource_id == record.resource_id)
        .unwrap();
    assert_eq!(owned.resource_generation, 2);
    assert_eq!(owned.workspace_generation, 1);
}

#[tokio::test]
async fn invalid_save_stays_visible_and_pauses_without_revision() {
    let (_home, paths, db, record) = fixture().await;
    let invalid = b"---\nname: review\n---\n# broken but visible\n";
    fs::write(paths.skills.join("alice/review/SKILL.md"), invalid).unwrap();

    let (_pass, blockers) = capture_local_edits(&paths, &db, false).await.unwrap();
    assert_eq!(blockers.len(), 1);
    assert!(db.queued_local_revisions().await.unwrap().is_empty());
    let state = db
        .workspace_state(record.resource_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, WorkspaceStatus::PausedValidation);
    assert_eq!(
        fs::read(paths.skills.join("alice/review/SKILL.md")).unwrap(),
        invalid
    );
}

#[tokio::test]
async fn direct_name_edit_becomes_pending_rename_with_exact_recovery() {
    let (_home, paths, db, record) = fixture().await;
    fs::write(
        paths.skills.join("alice/review/SKILL.md"),
        skill_document("renamed"),
    )
    .unwrap();

    let (_pass, blockers) = capture_local_edits(&paths, &db, false).await.unwrap();
    assert_eq!(blockers.len(), 1);
    assert_eq!(
        blockers[0].recovery.as_deref(),
        Some("denju rename @alice/review renamed")
    );
    assert!(db.queued_local_revisions().await.unwrap().is_empty());
    let state = db
        .workspace_state(record.resource_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.status, WorkspaceStatus::PendingRename);
    assert_eq!(state.pending_rename.as_deref(), Some("renamed"));
}

#[tokio::test]
async fn missing_managed_root_is_restored_without_registry_mutation() {
    let (_home, paths, db, _record) = fixture().await;
    let canonical = paths.skills.join("alice/review");
    #[cfg(unix)]
    fs::remove_file(&canonical).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&canonical).unwrap();
    assert!(!canonical.exists());

    let (pass, blockers) = capture_local_edits(&paths, &db, false).await.unwrap();
    assert!(blockers.is_empty());
    assert_eq!(pass.queued, 0);
    assert!(canonical.join("SKILL.md").is_file());
}

#[tokio::test]
async fn concurrent_scanners_do_not_duplicate_revision_for_same_tree() {
    let (_home, paths, db, _record) = fixture().await;
    fs::write(paths.skills.join("alice/review/notes.txt"), b"raced\n").unwrap();
    let paths_a = paths.clone();
    let paths_b = paths.clone();
    let db_a = db.clone();
    let db_b = db.clone();
    let (a, b) = tokio::join!(
        capture_local_edits(&paths_a, &db_a, false),
        capture_local_edits(&paths_b, &db_b, false)
    );
    assert!(a.unwrap().1.is_empty());
    assert!(b.unwrap().1.is_empty());
    assert_eq!(db.queued_local_revisions().await.unwrap().len(), 1);
}

#[tokio::test]
async fn untouched_conflict_head_is_not_mistaken_for_a_resolution() {
    let (_home, paths, db, record) = fixture().await;
    fs::write(
        paths.skills.join("alice/review/notes.txt"),
        b"detached head\n",
    )
    .unwrap();
    let scan = scan_owned_workspace(&paths, &db, &record, true)
        .await
        .unwrap();
    let working_root = scan.manifest.root_tree().to_string();
    install_test_conflict(&db, &record, working_root, true).await;

    let (pass, blockers) = capture_local_edits(&paths, &db, false).await.unwrap();
    assert!(blockers.is_empty());
    assert_eq!(pass.queued, 0);
    assert!(db.queued_local_revisions().await.unwrap().is_empty());
}

#[tokio::test]
async fn conflict_resolution_queues_only_after_working_tree_changes() {
    let (_home, paths, db, record) = fixture().await;
    fs::write(
        paths.skills.join("alice/review/notes.txt"),
        b"detached head\n",
    )
    .unwrap();
    let scan = scan_owned_workspace(&paths, &db, &record, true)
        .await
        .unwrap();
    let working_root = scan.manifest.root_tree().to_string();
    install_test_conflict(&db, &record, working_root, true).await;
    let (first, blockers) = capture_local_edits(&paths, &db, false).await.unwrap();
    assert!(blockers.is_empty());
    assert_eq!(first.queued, 0);

    fs::write(
        paths.skills.join("alice/review/notes.txt"),
        b"resolved working result\n",
    )
    .unwrap();
    let (second, blockers) = capture_local_edits(&paths, &db, false).await.unwrap();
    assert!(blockers.is_empty());
    assert_eq!(second.queued, 1);
    let queued = db.queued_local_revisions().await.unwrap().remove(0);
    assert_eq!(queued.operation_id, "01890f47-6a1d-7ad0-8f43-9a4d8c29f002");
    assert_eq!(queued.parent_revision_ids.len(), 2);
    assert!(
        queued
            .parent_revision_ids
            .contains(&RevisionId::from_bytes([8; 32]).to_string())
    );
    assert!(
        queued
            .parent_revision_ids
            .contains(&RevisionId::from_bytes([9; 32]).to_string())
    );
}

#[test]
fn conflict_status_exposes_exact_diff_restore_and_sync_commands() {
    let head_a = RevisionId::from_bytes([8; 32]).to_string();
    let head_b = RevisionId::from_bytes([9; 32]).to_string();
    let conflict = ConflictStatus {
        conflict_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".into(),
        base_revision_id: RevisionId::from_bytes([7; 32]).to_string(),
        head_revision_ids: vec![head_a.clone(), head_b.clone()],
        active_revision_id: head_b.clone(),
        conflict_paths: vec!["notes.txt".into()],
    };
    assert_eq!(
        status_commands("@alice/review", WorkspaceStatus::Conflict, Some(&conflict)),
        vec![
            format!("denju diff @alice/review {head_a} {head_b}"),
            format!("denju restore @alice/review {head_a}"),
            format!("denju restore @alice/review {head_b}"),
            "denju sync".to_owned(),
        ]
    );
}

async fn install_test_conflict(
    db: &LocalDatabase,
    record: &OwnedSkillRecord,
    working_root_tree_id: String,
    resolution_required: bool,
) {
    let active = RevisionId::from_bytes([9; 32]).to_string();
    db.save_workspace_content_conflict(
        WorkspaceContentConflictRecord {
            conflict_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".into(),
            resource_id: record.resource_id.clone(),
            base_revision_id: RevisionId::from_bytes([7; 32]).to_string(),
            head_revision_ids: vec![RevisionId::from_bytes([8; 32]).to_string(), active.clone()],
            active_revision_id: active,
            remote_generation: 2,
            working_root_tree_id,
            resolution_required,
            conflict_paths: vec!["notes.txt".into()],
        },
        3,
    )
    .await
    .unwrap();
    db.pause_workspace(
        record.resource_id.clone(),
        WorkspaceStatus::Conflict,
        "concurrent edits".into(),
        None,
        4,
    )
    .await
    .unwrap();
}
