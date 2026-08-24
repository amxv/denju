use denju_local::{
    LocalDatabase, LocalPaths, OwnedSkillRecord, PackApplyJournalPayload,
    PackMaterializedSkillRecord, ResolvedHarnessRoots, ensure_local_layout,
};
use tempfile::tempdir;

use super::*;

#[tokio::test]
async fn disappearing_owned_workspace_recovers_through_stale_pack_overlap() {
    let home = tempdir().unwrap();
    let paths = LocalPaths::from_home(home.path().to_owned());
    ensure_local_layout(&paths).unwrap();
    let db = LocalDatabase::open(&paths.state_db).await.unwrap();
    let roots = ResolvedHarnessRoots {
        codex_root: home.path().join("codex"),
        claude_root: home.path().join("claude"),
    };
    let resource_id = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1".to_owned();
    let revision = "11".repeat(32);
    let owned = OwnedSkillRecord {
        resource_id: resource_id.clone(),
        locator: "@acme/review".to_owned(),
        owner: "acme".to_owned(),
        skill_name: "review".to_owned(),
        resource_generation: 2,
        workspace_generation: 1,
        desired_revision_id: revision.clone(),
        harness_name: Some("review".to_owned()),
        materialized_revision_id: Some(revision.clone()),
    };
    db.upsert_owned_skill_desired(owned.clone(), 1)
        .await
        .unwrap();
    let operation_id = OperationId::from_uuid(Uuid::now_v7()).unwrap();
    db.create_pack_apply_journal(
        operation_id,
        PackApplyJournalPayload {
            old_skills: Vec::new(),
            new_skills: Vec::new(),
        },
        1,
    )
    .await
    .unwrap();
    db.commit_pack_apply(
        operation_id,
        vec![PackMaterializedSkillRecord {
            resource_id: resource_id.clone(),
            locator: owned.locator.clone(),
            owner: owned.owner.clone(),
            skill_name: owned.skill_name.clone(),
            resource_generation: 2,
            desired_revision_id: revision.clone(),
            desired_root_tree_id: "root".to_owned(),
            harness_name: Some("review".to_owned()),
            materialized_revision_id: revision,
        }],
        Vec::new(),
        2,
    )
    .await
    .unwrap();

    // This is the transition reached after refresh_catalog() observes that the team and
    // its enforced pack disappeared: the old pack projection still awaits journaled
    // removal while the former owned workspace row is no longer policy-suppressed.
    assert!(db.managed_skills().await.is_err());

    remove_disappeared_owned(&paths, &db, &roots, &owned)
        .await
        .unwrap();

    assert!(db.owned_skills().await.unwrap().is_empty());
    assert_eq!(db.pack_materialized_skills().await.unwrap().len(), 1);
}
