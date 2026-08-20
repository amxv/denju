use std::collections::BTreeSet;
use std::{fs, str::FromStr};

use denju_client::ClientError;
use denju_core::{AuthorPrincipalId, OperationId, Revision, RevisionId};
use denju_local::{
    LocalDatabase, LocalPaths, LocalRevisionRecord, OwnedSkillRecord, WorkspaceScanError,
    WorkspaceStatus, create_native_directory_link, reconcile_owned_derived_projection,
    recover_workspace_writebacks, scan_owned_workspace, workspace_blob_path,
};
use denju_wire::{
    ApiErrorCode, CliErrorCode, PrivateRevisionCommitRequest, PrivateRevisionRequest,
    PublicSkillManifest, private_revision_request_hash,
};
use uuid::Uuid;

use crate::{public::InstalledContext, setup::RuntimeError};

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct WorkspacePass {
    pub(crate) scanned: usize,
    pub(crate) queued: usize,
    pub(crate) hashed_files: usize,
    pub(crate) reused_files: usize,
}

pub(crate) async fn capture_local_edits(
    paths: &LocalPaths,
    db: &LocalDatabase,
    force_full_hash: bool,
) -> Result<(WorkspacePass, Vec<RuntimeError>), RuntimeError> {
    recover_workspace_writebacks(paths, db)
        .await
        .map_err(local_error)?;
    let Some(identity) = db.identity().await.map_err(local_error)? else {
        return Ok((WorkspacePass::default(), Vec::new()));
    };
    let Some(author_text) = identity.author_principal_id else {
        // Phase-5 installations learn the claimed-user author principal on their next
        // authenticated registry pass. Until then, preserve edits in place rather than
        // inventing installation-authored history for a claimed user.
        return Ok((WorkspacePass::default(), Vec::new()));
    };
    let author = AuthorPrincipalId::from_str(&author_text)
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
    let mut pass = WorkspacePass::default();
    let mut blockers = Vec::new();

    for record in db.owned_skills().await.map_err(local_error)? {
        let holder = format!("workspace-scan-{}", Uuid::now_v7());
        let resource_key = format!("skill:{}", record.resource_id);
        if !db
            .claim_lease(resource_key.clone(), holder.clone(), now_unix_ms(), 60_000)
            .await
            .map_err(local_error)?
        {
            continue;
        }
        let result = capture_one(paths, db, &record, author, force_full_hash).await;
        let _ = db.release_lease(resource_key, holder).await;
        match result {
            Ok(stats) => {
                pass.scanned += 1;
                pass.queued += usize::from(stats.queued);
                pass.hashed_files += stats.hashed_files;
                pass.reused_files += stats.reused_files;
            }
            Err(error) => blockers.push(error),
        }
    }
    Ok((pass, blockers))
}

struct CaptureOne {
    queued: bool,
    hashed_files: usize,
    reused_files: usize,
}

async fn capture_one(
    paths: &LocalPaths,
    db: &LocalDatabase,
    record: &OwnedSkillRecord,
    author: AuthorPrincipalId,
    force_full_hash: bool,
) -> Result<CaptureOne, RuntimeError> {
    if let Err(error) = reconcile_owned_derived_projection(paths, db, record).await {
        let message = error.to_string();
        if matches!(error, denju_local::ProjectionError::DivergedDerivedEdit(_))
            && db
                .workspace_state(record.resource_id.clone())
                .await
                .map_err(local_error)?
                .is_some()
        {
            db.pause_workspace(
                record.resource_id.clone(),
                WorkspaceStatus::Conflict,
                message.clone(),
                None,
                now_unix_ms(),
            )
            .await
            .map_err(local_error)?;
        }
        return Err(RuntimeError::new(CliErrorCode::LocalState, message).recovery("denju sync"));
    }
    let scan = match scan_owned_workspace(paths, db, record, force_full_hash).await {
        Ok(scan) => scan,
        Err(WorkspaceScanError::MissingCanonical { .. }) => {
            restore_missing_canonical(paths, db, record).await?;
            match scan_owned_workspace(paths, db, record, true).await {
                Ok(scan) => scan,
                Err(error) => return pause_scan_error(db, record, error).await,
            }
        }
        Err(error) => return pause_scan_error(db, record, error).await,
    };

    let root_tree = scan.manifest.root_tree().to_string();
    let working_generation_path = scan.working_generation_path.display().to_string();
    let mut state = db
        .workspace_state(record.resource_id.clone())
        .await
        .map_err(local_error)?;
    if state.is_none() {
        db.ensure_workspace_baseline(
            record.resource_id.clone(),
            record.resource_generation,
            record.desired_revision_id.clone(),
            root_tree.clone(),
            working_generation_path.clone(),
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
        state = db
            .workspace_state(record.resource_id.clone())
            .await
            .map_err(local_error)?;
    }
    let state = state.ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            format!("workspace state is missing for {}", record.locator),
        )
    })?;
    if state.working_generation_path != working_generation_path {
        db.set_workspace_working_generation(
            record.resource_id.clone(),
            working_generation_path,
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    }

    if state.valid_root_tree_id == root_tree {
        if matches!(
            state.status,
            WorkspaceStatus::PausedValidation | WorkspaceStatus::PendingRename
        ) {
            db.resume_workspace(record.resource_id.clone(), now_unix_ms())
                .await
                .map_err(local_error)?;
        }
        return Ok(CaptureOne {
            queued: false,
            hashed_files: scan.stats.hashed_files,
            reused_files: scan.stats.reused_files,
        });
    }

    let queued = db.queued_local_revisions().await.map_err(local_error)?;
    let latest = queued
        .iter()
        .filter(|revision| revision.resource_id == record.resource_id)
        .max_by_key(|revision| revision.expected_generation);
    let expected_generation = latest
        .map(|revision| revision.expected_generation.saturating_add(1))
        .unwrap_or(state.base_generation);
    let parent_text = latest
        .map(|revision| revision.revision_id.as_str())
        .unwrap_or(state.local_head_revision_id.as_str());
    let parent = RevisionId::from_str(parent_text)
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
    let operation = OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let revision = Revision::new(scan.manifest.root_tree(), vec![parent], author, operation)
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
    let manifest = PublicSkillManifest::from_core(&scan.manifest);
    db.enqueue_local_revision(
        LocalRevisionRecord {
            operation_id: operation.to_string(),
            resource_id: record.resource_id.clone(),
            revision_id: revision.id().to_string(),
            parent_revision_id: parent.to_string(),
            expected_generation,
            root_tree_id: root_tree,
            manifest_json: serde_json::to_string(&manifest)
                .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?,
            state: "queued".to_owned(),
        },
        now_unix_ms(),
    )
    .await
    .map_err(local_error)?;
    Ok(CaptureOne {
        queued: true,
        hashed_files: scan.stats.hashed_files,
        reused_files: scan.stats.reused_files,
    })
}

async fn pause_scan_error(
    db: &LocalDatabase,
    record: &OwnedSkillRecord,
    error: WorkspaceScanError,
) -> Result<CaptureOne, RuntimeError> {
    let (status, message, pending, recovery, code) = match error {
        WorkspaceScanError::PendingRename { requested } => (
            WorkspaceStatus::PendingRename,
            format!(
                "{} declares name {requested}; rename is an explicit registry operation",
                record.locator
            ),
            Some(requested.clone()),
            format!("denju rename {} {requested}", record.locator),
            CliErrorCode::ContentVerification,
        ),
        WorkspaceScanError::Validation(detail) => (
            WorkspaceStatus::PausedValidation,
            format!("{} is locally invalid: {detail}", record.locator),
            None,
            "fix the skill, then run denju sync".to_owned(),
            CliErrorCode::ContentVerification,
        ),
        other => {
            return Err(
                RuntimeError::new(CliErrorCode::LocalState, other.to_string())
                    .recovery("denju doctor"),
            );
        }
    };
    if db
        .workspace_state(record.resource_id.clone())
        .await
        .map_err(local_error)?
        .is_some()
    {
        db.pause_workspace(
            record.resource_id.clone(),
            status,
            message.clone(),
            pending,
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    }
    Err(RuntimeError::new(code, message).recovery(recovery))
}

async fn restore_missing_canonical(
    paths: &LocalPaths,
    db: &LocalDatabase,
    record: &OwnedSkillRecord,
) -> Result<(), RuntimeError> {
    let target = if let Some(state) = db
        .workspace_state(record.resource_id.clone())
        .await
        .map_err(local_error)?
    {
        std::path::PathBuf::from(state.working_generation_path)
    } else {
        let revision = record.materialized_revision_id.as_deref().ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                format!("{} has no materialized revision to restore", record.locator),
            )
        })?;
        paths.generations.join(&record.resource_id).join(revision)
    };
    if !target.is_dir() {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            format!("{} lost its managed generation", record.locator),
        )
        .recovery("denju doctor"));
    }
    let canonical = paths.skills.join(&record.owner).join(&record.skill_name);
    if let Some(parent) = canonical.parent() {
        fs::create_dir_all(parent).map_err(local_error)?;
    }
    if fs::symlink_metadata(&canonical).is_err() {
        create_native_directory_link(&target, &canonical).map_err(local_error)?;
    }
    Ok(())
}

pub(crate) async fn drain_queued_revisions(
    context: &InstalledContext,
) -> Result<(usize, Vec<RuntimeError>), RuntimeError> {
    let mut uploaded = 0;
    let mut blockers = Vec::new();
    let mut blocked_resources = BTreeSet::new();
    for revision in context
        .db
        .queued_local_revisions()
        .await
        .map_err(local_error)?
    {
        if blocked_resources.contains(&revision.resource_id) {
            continue;
        }
        match upload_one(context, &revision).await {
            Ok(()) => uploaded += 1,
            Err(error) if is_workspace_blocker(&error) => {
                blocked_resources.insert(revision.resource_id.clone());
                blockers.push(error);
            }
            Err(error) => return Err(error),
        }
    }
    Ok((uploaded, blockers))
}

async fn upload_one(
    context: &InstalledContext,
    revision: &LocalRevisionRecord,
) -> Result<(), RuntimeError> {
    let manifest: PublicSkillManifest = serde_json::from_str(&revision.manifest_json)
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
    let operation = revision.operation_id.clone();
    let expected_generation = u64::try_from(revision.expected_generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "queued workspace generation is invalid",
        )
    })?;
    let request_hash = private_revision_request_hash(
        &operation,
        &revision.resource_id,
        expected_generation,
        &revision.parent_revision_id,
        &manifest,
    )
    .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let request = PrivateRevisionRequest {
        operation_id: operation.clone(),
        resource_id: revision.resource_id.clone(),
        expected_generation,
        expected_parent_revision_id: revision.parent_revision_id.clone(),
        manifest,
        request_hash: request_hash.to_string(),
    };
    let prepared = match context.client.prepare_private_revision(&request).await {
        Ok(prepared) => prepared,
        Err(error) => return pause_remote_blocker(context, revision, error).await,
    };
    if prepared.revision_id != revision.revision_id {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            "registry computed a different private revision identity",
        ));
    }
    for upload in &prepared.uploads {
        let blob = denju_core::BlobId::from_str(&upload.blob_id)
            .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
        let bytes = fs::read(workspace_blob_path(&context.paths, blob)).map_err(local_error)?;
        context
            .client
            .upload_staged_blob(upload, &bytes)
            .await
            .map_err(client_error)?;
    }
    let committed = match context
        .client
        .commit_private_revision(&PrivateRevisionCommitRequest {
            operation_id: operation.clone(),
            request_hash: request_hash.to_string(),
        })
        .await
    {
        Ok(committed) => committed,
        Err(error) => return pause_remote_blocker(context, revision, error).await,
    };
    if committed.revision_id != revision.revision_id {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            "registry committed a different private revision identity",
        ));
    }
    context
        .db
        .mark_local_revision_synced(
            operation,
            i64::try_from(committed.generation).map_err(|_| {
                RuntimeError::new(CliErrorCode::LocalState, "registry generation is too large")
            })?,
            committed.revision_id,
            now_unix_ms(),
        )
        .await
        .map_err(local_error)
}

async fn pause_remote_blocker(
    context: &InstalledContext,
    revision: &LocalRevisionRecord,
    error: ClientError,
) -> Result<(), RuntimeError> {
    match &error {
        ClientError::Registry(api) if api.code == ApiErrorCode::QuotaExceeded => {
            let message = format!(
                "{} is queued locally because registry storage quota is exhausted",
                revision.resource_id
            );
            context
                .db
                .pause_workspace(
                    revision.resource_id.clone(),
                    WorkspaceStatus::Quota,
                    message.clone(),
                    None,
                    now_unix_ms(),
                )
                .await
                .map_err(local_error)?;
            Err(RuntimeError::new(CliErrorCode::QuotaExceeded, message).recovery("denju usage"))
        }
        ClientError::Registry(api) if api.code == ApiErrorCode::GenerationConflict => {
            let message = format!(
                "private workspace {} advanced on another device; both heads were preserved",
                revision.resource_id
            );
            context
                .db
                .pause_workspace(
                    revision.resource_id.clone(),
                    WorkspaceStatus::Conflict,
                    message.clone(),
                    None,
                    now_unix_ms(),
                )
                .await
                .map_err(local_error)?;
            Err(RuntimeError::new(CliErrorCode::LocalState, message).recovery("denju sync"))
        }
        _ => Err(client_error(error)),
    }
}

fn is_workspace_blocker(error: &RuntimeError) -> bool {
    matches!(
        error.code,
        CliErrorCode::QuotaExceeded | CliErrorCode::ContentVerification | CliErrorCode::LocalState
    )
}

fn client_error(error: ClientError) -> RuntimeError {
    crate::public::client_error(error)
}

fn local_error(error: impl std::fmt::Display) -> RuntimeError {
    crate::public::local_error(error)
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
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
}
