use denju_core::OperationId;
use tempfile::tempdir;
use uuid::Uuid;

use super::*;

#[tokio::test]
async fn sqlite_worker_uses_wal_and_persists_journal() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("state.db");
    let db = LocalDatabase::open(&path).await.unwrap();
    let operation_id = OperationId::from_uuid(Uuid::now_v7()).unwrap();
    db.create_bootstrap_journal(
        operation_id,
        BootstrapJournalPayload {
            registry_origin: "http://127.0.0.1:7788".to_owned(),
            credential_hash: "00".repeat(32),
            credential_backend: None,
            installation_id: None,
            author_principal_id: None,
        },
        1,
    )
    .await
    .unwrap();
    assert_eq!(
        db.bootstrap_journal().await.unwrap().unwrap().state,
        JournalState::Planned
    );
    db.quick_check().await.unwrap();

    let mode: String = db
        .call(|connection| {
            connection
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .map_err(LocalDbError::from)
        })
        .await
        .unwrap();
    assert_eq!(mode.to_ascii_lowercase(), "wal");
}

#[tokio::test]
async fn local_schema_converges_directly_to_current_version() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("state.db");
    let db = LocalDatabase::open(&path).await.unwrap();
    let version: i64 = db
        .call(|connection| {
            connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .map_err(LocalDbError::from)
        })
        .await
        .unwrap();
    assert_eq!(version, 8);
}

#[tokio::test]
async fn phase9_schema_keeps_phase8_local_revision_insert_shape() {
    let dir = tempdir().unwrap();
    let db = LocalDatabase::open(dir.path().join("state.db"))
        .await
        .unwrap();
    db.upsert_owned_skill_desired(
        OwnedSkillRecord {
            resource_id: "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1".to_owned(),
            locator: "@alice/review".to_owned(),
            owner: "alice".to_owned(),
            skill_name: "review".to_owned(),
            resource_generation: 1,
            desired_revision_id: "11".repeat(32),
            harness_name: None,
            materialized_revision_id: None,
        },
        1,
    )
    .await
    .unwrap();

    db.call(|connection| {
        connection.execute(
            "INSERT INTO local_revisions \
             (operation_id,resource_id,revision_id,parent_revision_id,expected_generation,root_tree_id,manifest_json,state,created_at_unix_ms,updated_at_unix_ms) \
             VALUES (?1,?2,?3,?4,1,?5,'{}','queued',1,1)",
            rusqlite::params![
                "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
                "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1",
                "22".repeat(32),
                "11".repeat(32),
                "33".repeat(32),
            ],
        )?;
        let merge_parent: Option<String> = connection.query_row(
            "SELECT merge_parent_revision_id FROM local_revisions WHERE revision_id=?1",
            rusqlite::params!["22".repeat(32)],
            |row| row.get(0),
        )?;
        assert!(merge_parent.is_none());
        connection.execute(
            "UPDATE local_revisions SET state='synced',updated_at_unix_ms=2 WHERE revision_id=?1",
            rusqlite::params!["22".repeat(32)],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn merge_revision_parent_order_round_trips_canonically() {
    let dir = tempdir().unwrap();
    let db = LocalDatabase::open(dir.path().join("state.db"))
        .await
        .unwrap();
    let resource_id = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1".to_owned();
    let active_head = "ff".repeat(32);
    let detached_head = "00".repeat(32);
    db.upsert_owned_skill_desired(
        OwnedSkillRecord {
            resource_id: resource_id.clone(),
            locator: "@alice/review".to_owned(),
            owner: "alice".to_owned(),
            skill_name: "review".to_owned(),
            resource_generation: 2,
            desired_revision_id: active_head.clone(),
            harness_name: None,
            materialized_revision_id: None,
        },
        1,
    )
    .await
    .unwrap();
    db.ensure_workspace_baseline(
        resource_id.clone(),
        2,
        active_head.clone(),
        "33".repeat(32),
        dir.path().join("generation").display().to_string(),
        1,
    )
    .await
    .unwrap();
    db.enqueue_local_revision(
        crate::LocalRevisionRecord {
            operation_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".to_owned(),
            resource_id,
            revision_id: "44".repeat(32),
            expected_head_revision_id: active_head.clone(),
            parent_revision_ids: vec![active_head.clone(), detached_head.clone()],
            expected_generation: 2,
            root_tree_id: "55".repeat(32),
            manifest_json: "{}".to_owned(),
            state: "queued".to_owned(),
        },
        2,
    )
    .await
    .unwrap();
    let queued = db.queued_local_revisions().await.unwrap().remove(0);
    assert_eq!(queued.expected_head_revision_id, active_head);
    assert_eq!(
        queued.parent_revision_ids,
        vec![detached_head, "ff".repeat(32)]
    );
}

#[tokio::test]
async fn leases_expire_and_are_holder_scoped() {
    let dir = tempdir().unwrap();
    let db = LocalDatabase::open(dir.path().join("state.db"))
        .await
        .unwrap();
    assert!(
        db.claim_lease("skill:a".into(), "cli".into(), 100, 50)
            .await
            .unwrap()
    );
    assert!(
        !db.claim_lease("skill:a".into(), "daemon".into(), 120, 50)
            .await
            .unwrap()
    );
    db.release_lease("skill:a".into(), "daemon".into())
        .await
        .unwrap();
    assert!(
        !db.claim_lease("skill:a".into(), "daemon".into(), 120, 50)
            .await
            .unwrap()
    );
    assert!(
        db.claim_lease("skill:a".into(), "daemon".into(), 151, 50)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn owned_skills_share_projection_state_without_becoming_subscriptions() {
    let dir = tempdir().unwrap();
    let db = LocalDatabase::open(dir.path().join("state.db"))
        .await
        .unwrap();
    let owned_id = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1".to_owned();
    db.upsert_owned_skill_desired(
        OwnedSkillRecord {
            resource_id: owned_id.clone(),
            locator: "@alice/owned".to_owned(),
            owner: "alice".to_owned(),
            skill_name: "owned".to_owned(),
            resource_generation: 1,
            desired_revision_id: "11".repeat(32),
            harness_name: None,
            materialized_revision_id: None,
        },
        1,
    )
    .await
    .unwrap();
    db.upsert_subscription_desired(
        SubscriptionRecord {
            resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".to_owned(),
            locator: "@bob/public".to_owned(),
            owner: "bob".to_owned(),
            skill_name: "public".to_owned(),
            resource_generation: 1,
            release_version: 1,
            desired_revision_id: "22".repeat(32),
            harness_name: None,
            materialized_revision_id: None,
            retain_on_delete: false,
            retained_after_delete: false,
            live_private: false,
            desired_root_tree_id: "33".repeat(32),
        },
        1,
    )
    .await
    .unwrap();

    db.mark_skill_materialized(owned_id.clone(), "11".repeat(32), 2)
        .await
        .unwrap();
    db.set_managed_harness_name(owned_id.clone(), "owned".to_owned(), 3)
        .await
        .unwrap();

    assert_eq!(db.subscriptions().await.unwrap().len(), 1);
    let owned = db.owned_skills().await.unwrap();
    assert_eq!(owned.len(), 1);
    assert_eq!(owned[0].resource_id, owned_id);
    assert_eq!(owned[0].harness_name.as_deref(), Some("owned"));
    assert_eq!(db.managed_skills().await.unwrap().len(), 2);
}

#[tokio::test]
async fn local_fork_provenance_is_immutable_and_cascades_with_owned_skill() {
    let dir = tempdir().unwrap();
    let db = LocalDatabase::open(dir.path().join("state.db"))
        .await
        .unwrap();
    let resource_id = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1".to_owned();
    db.upsert_owned_skill_desired(
        OwnedSkillRecord {
            resource_id: resource_id.clone(),
            locator: "@local/review".to_owned(),
            owner: "local".to_owned(),
            skill_name: "review".to_owned(),
            resource_generation: 1,
            desired_revision_id: "11".repeat(32),
            harness_name: None,
            materialized_revision_id: Some("11".repeat(32)),
        },
        1,
    )
    .await
    .unwrap();
    db.save_local_fork(
        crate::LocalForkRecord {
            resource_id: resource_id.clone(),
            upstream_resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".to_owned(),
            upstream_locator: "@alice/review".to_owned(),
            created_from_revision_id: "22".repeat(32),
            sync_base_revision_id: "22".repeat(32),
            desired_name: "review".to_owned(),
            state: "local".to_owned(),
        },
        2,
    )
    .await
    .unwrap();

    // Saving mutable fork state must not be able to rewrite provenance.
    db.save_local_fork(
        crate::LocalForkRecord {
            resource_id: resource_id.clone(),
            upstream_resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f099".to_owned(),
            upstream_locator: "@mallory/rewritten".to_owned(),
            created_from_revision_id: "99".repeat(32),
            sync_base_revision_id: "33".repeat(32),
            desired_name: "review-local".to_owned(),
            state: "name_conflict".to_owned(),
        },
        3,
    )
    .await
    .unwrap();
    let fork = db
        .local_fork_for_upstream("01890f47-6a1d-7ad0-8f43-9a4d8c29f002".to_owned())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fork.upstream_locator, "@alice/review");
    assert_eq!(fork.created_from_revision_id, "22".repeat(32));
    assert_eq!(fork.sync_base_revision_id, "33".repeat(32));
    assert_eq!(fork.desired_name, "review-local");
    assert_eq!(fork.state, "name_conflict");

    db.remove_owned_skill(resource_id).await.unwrap();
    assert!(db.local_forks().await.unwrap().is_empty());
}

#[tokio::test]
async fn import_journal_resumes_only_until_complete() {
    let dir = tempdir().unwrap();
    let db = LocalDatabase::open(dir.path().join("state.db"))
        .await
        .unwrap();
    let operation_id = OperationId::from_uuid(Uuid::now_v7()).unwrap();
    let source = dir.path().join("source/review").display().to_string();
    let payload = ImportJournalPayload {
        source_path: source.clone(),
        skill_name: "review".to_owned(),
        request_hash: "11".repeat(32),
        manifest_json: "{}".to_owned(),
        snapshot_sha256: "22".repeat(32),
        snapshot_size_bytes: 42,
        snapshot_path: dir.path().join("snapshot.tar.zst").display().to_string(),
        resource_id: None,
        locator: None,
        revision_id: None,
    };
    db.create_import_journal(operation_id, payload.clone(), 1)
        .await
        .unwrap();
    assert_eq!(
        db.import_journal_for_source(source.clone())
            .await
            .unwrap()
            .unwrap()
            .state,
        JournalState::Planned
    );

    let mut expected = JournalState::Planned;
    for (next, now) in [
        (JournalState::Staged, 2),
        (JournalState::Verified, 3),
        (JournalState::Switched, 4),
        (JournalState::Complete, 5),
    ] {
        db.update_import_journal(operation_id, expected, next, payload.clone(), now)
            .await
            .unwrap();
        expected = next;
    }
    assert!(
        db.import_journal_for_source(source)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn account_delete_journal_survives_authority_and_cleanup_boundaries() {
    let dir = tempdir().unwrap();
    let db = LocalDatabase::open(dir.path().join("state.db"))
        .await
        .unwrap();
    let operation_id = OperationId::from_uuid(Uuid::now_v7()).unwrap();
    let payload = AccountDeleteJournalPayload {
        username: "@alice".to_owned(),
        session_backend: "file".to_owned(),
        installation_backend: "file".to_owned(),
        removed_local_skills: 3,
    };
    db.create_account_delete_journal(operation_id, payload.clone(), 1)
        .await
        .unwrap();
    let journal = db.account_delete_journal().await.unwrap().unwrap();
    assert_eq!(journal.state, JournalState::Planned);
    assert_eq!(journal.payload, payload);

    db.advance_account_delete_journal(
        operation_id,
        JournalState::Planned,
        JournalState::Staged,
        payload.clone(),
        2,
    )
    .await
    .unwrap();
    db.advance_account_delete_journal(
        operation_id,
        JournalState::Staged,
        JournalState::Verified,
        payload.clone(),
        3,
    )
    .await
    .unwrap();
    db.advance_account_delete_journal(
        operation_id,
        JournalState::Verified,
        JournalState::Switched,
        payload,
        4,
    )
    .await
    .unwrap();
    assert_eq!(
        db.account_delete_journal().await.unwrap().unwrap().state,
        JournalState::Switched
    );
    db.finish_account_delete_journal(operation_id)
        .await
        .unwrap();
    assert!(db.account_delete_journal().await.unwrap().is_none());
}
