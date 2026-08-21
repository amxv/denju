use std::collections::{BTreeMap, BTreeSet};
use std::{fs, str::FromStr};

use denju_client::ClientError;
use denju_core::{AuthorPrincipalId, OperationId, Revision, RevisionId};
use denju_local::{
    LocalDatabase, LocalPaths, LocalRevisionRecord, OwnedSkillRecord,
    WorkspaceContentConflictRecord, WorkspaceScanError, WorkspaceStatus,
    create_native_directory_link, reconcile_owned_derived_projection, recover_workspace_writebacks,
    scan_owned_workspace, workspace_blob_path,
};
use denju_wire::{
    ApiErrorCode, CliErrorCode, PrivateRevisionCommitRequest, PrivateRevisionCommitResponse,
    PrivateRevisionRequest, PrivateWorkspaceConflict, PublicSkillManifest,
    private_revision_request_hash,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{public::InstalledContext, setup::RuntimeError};

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct WorkspacePass {
    pub(crate) scanned: usize,
    pub(crate) queued: usize,
    pub(crate) hashed_files: usize,
    pub(crate) reused_files: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StatusOutcome {
    pub(crate) state: &'static str,
    pub(crate) resources: Vec<ResourceStatus>,
    pub(crate) forks: Vec<ForkStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ForkStatus {
    pub(crate) resource_id: String,
    pub(crate) locator: String,
    pub(crate) state: &'static str,
    pub(crate) message: String,
    pub(crate) next_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResourceStatus {
    pub(crate) resource_id: String,
    pub(crate) locator: String,
    pub(crate) state: WorkspaceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) conflict: Option<ConflictStatus>,
    pub(crate) next_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ConflictStatus {
    pub(crate) conflict_id: String,
    pub(crate) base_revision_id: String,
    pub(crate) head_revision_ids: Vec<String>,
    pub(crate) active_revision_id: String,
    pub(crate) conflict_paths: Vec<String>,
}

pub(crate) async fn status() -> Result<StatusOutcome, RuntimeError> {
    let paths = LocalPaths::discover().map_err(local_error)?;
    if !paths.state_db.is_file() {
        return Err(
            RuntimeError::new(CliErrorCode::SetupRequired, "Denju is not set up")
                .recovery("denju setup"),
        );
    }
    let db = LocalDatabase::open(&paths.state_db)
        .await
        .map_err(local_error)?;
    let locators = db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .map(|record| (record.resource_id, record.locator))
        .collect::<BTreeMap<_, _>>();
    let mut resources = Vec::new();
    for state in db.workspace_states().await.map_err(local_error)? {
        if state.status == WorkspaceStatus::Clean {
            continue;
        }
        let locator = locators
            .get(&state.resource_id)
            .cloned()
            .unwrap_or_else(|| state.resource_id.clone());
        let conflict = if state.status == WorkspaceStatus::Conflict {
            db.workspace_content_conflict(state.resource_id.clone())
                .await
                .map_err(local_error)?
                .map(|conflict| ConflictStatus {
                    conflict_id: conflict.conflict_id,
                    base_revision_id: conflict.base_revision_id,
                    head_revision_ids: conflict.head_revision_ids,
                    active_revision_id: conflict.active_revision_id,
                    conflict_paths: conflict.conflict_paths,
                })
        } else {
            None
        };
        let next_commands = status_commands(&locator, state.status, conflict.as_ref());
        resources.push(ResourceStatus {
            resource_id: state.resource_id,
            locator,
            state: state.status,
            message: state.error_message,
            conflict,
            next_commands,
        });
    }
    let mut forks = Vec::new();
    for fork in db.local_forks().await.map_err(local_error)? {
        if fork.state != "name_conflict" {
            continue;
        }
        forks.push(ForkStatus {
            resource_id: fork.resource_id,
            locator: fork.upstream_locator.clone(),
            state: "name_conflict",
            message: format!(
                "the automatic fork name `{}` is already owned by this identity",
                fork.desired_name
            ),
            next_commands: vec![
                format!(
                    "denju fork resolve {} --as <new-name>",
                    fork.upstream_locator
                ),
                format!(
                    "denju fork resolve {} --merge-into @you/<skill>",
                    fork.upstream_locator
                ),
                format!("denju fork resolve {} --discard", fork.upstream_locator),
            ],
        });
    }
    if db
        .identity()
        .await
        .map_err(local_error)?
        .is_some_and(|identity| identity.session_backend.is_some())
        && let Ok(context) = crate::public::installed_context(true).await
        && let Ok(catalog) = context.client.private_skills().await
    {
        for skill in catalog.skills {
            let Some(provenance) = skill.fork else {
                continue;
            };
            if let Ok(history) = context
                .client
                .skill_history(&provenance.upstream_locator)
                .await
                && history.workspace_revision_id != provenance.sync_base_revision_id
            {
                forks.push(ForkStatus {
                    resource_id: skill.resource_id,
                    locator: skill.locator.clone(),
                    state: "upstream_ahead",
                    message: format!(
                        "{} advanced from {} to {}",
                        provenance.upstream_locator,
                        short_id(&provenance.sync_base_revision_id),
                        short_id(&history.workspace_revision_id)
                    ),
                    next_commands: vec![format!("denju fork sync {}", skill.locator)],
                });
            }
        }
    }
    Ok(StatusOutcome {
        state: if resources.is_empty() && forks.is_empty() {
            "healthy"
        } else {
            "attention_required"
        },
        resources,
        forks,
    })
}

fn short_id(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
}

fn status_commands(
    locator: &str,
    state: WorkspaceStatus,
    conflict: Option<&ConflictStatus>,
) -> Vec<String> {
    if let Some(conflict) = conflict
        && conflict.head_revision_ids.len() == 2
    {
        return vec![
            format!(
                "denju diff {locator} {} {}",
                conflict.head_revision_ids[0], conflict.head_revision_ids[1]
            ),
            format!("denju restore {locator} {}", conflict.head_revision_ids[0]),
            format!("denju restore {locator} {}", conflict.head_revision_ids[1]),
            "denju sync".to_owned(),
        ];
    }
    match state {
        WorkspaceStatus::Queued | WorkspaceStatus::Quota | WorkspaceStatus::Conflict => {
            vec!["denju sync".to_owned()]
        }
        WorkspaceStatus::PendingRename => vec![format!("denju rename {locator} <new-name>")],
        WorkspaceStatus::PausedValidation => vec!["denju sync".to_owned()],
        WorkspaceStatus::Clean => Vec::new(),
    }
}

pub(crate) async fn capture_local_edits(
    paths: &LocalPaths,
    db: &LocalDatabase,
    force_full_hash: bool,
) -> Result<(WorkspacePass, Vec<RuntimeError>), RuntimeError> {
    recover_workspace_writebacks(paths, db)
        .await
        .map_err(local_error)?;
    let user_author = db
        .identity()
        .await
        .map_err(local_error)?
        .and_then(|identity| identity.author_principal_id);
    let installation_author = db
        .installation()
        .await
        .map_err(local_error)?
        .map(|installation| installation.author_principal_id);
    let local_fork_ids = db
        .local_forks()
        .await
        .map_err(local_error)?
        .into_iter()
        .map(|fork| fork.resource_id)
        .collect::<BTreeSet<_>>();
    let mut pass = WorkspacePass::default();
    let mut blockers = Vec::new();

    for record in db.owned_skills().await.map_err(local_error)? {
        let local_only = local_fork_ids.contains(&record.resource_id);
        let author_text = user_author.as_ref().or({
            if local_only {
                installation_author.as_ref()
            } else {
                None
            }
        });
        let Some(author_text) = author_text else {
            continue;
        };
        let author = AuthorPrincipalId::from_str(author_text)
            .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
        let holder = format!("workspace-scan-{}", Uuid::now_v7());
        let resource_key = format!("skill:{}", record.resource_id);
        if !db
            .claim_lease(resource_key.clone(), holder.clone(), now_unix_ms(), 60_000)
            .await
            .map_err(local_error)?
        {
            continue;
        }
        let result = capture_one(paths, db, &record, author, force_full_hash, local_only).await;
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
    local_only: bool,
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

    if state.status == WorkspaceStatus::Conflict
        && let Some(conflict) = db
            .workspace_content_conflict(record.resource_id.clone())
            .await
            .map_err(local_error)?
    {
        let queued = db.queued_local_revisions().await.map_err(local_error)?;
        if queued
            .iter()
            .any(|revision| revision.resource_id == record.resource_id)
            || !conflict.resolution_required
            || root_tree == conflict.working_root_tree_id
        {
            return Ok(CaptureOne {
                queued: false,
                hashed_files: scan.stats.hashed_files,
                reused_files: scan.stats.reused_files,
            });
        }
        let operation = OperationId::from_str(&conflict.conflict_id)
            .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
        let parents = conflict
            .head_revision_ids
            .iter()
            .map(|parent| {
                RevisionId::from_str(parent)
                    .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let revision = Revision::new(scan.manifest.root_tree(), parents, author, operation)
            .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
        let manifest = PublicSkillManifest::from_core(&scan.manifest);
        db.enqueue_local_revision(
            LocalRevisionRecord {
                operation_id: operation.to_string(),
                resource_id: record.resource_id.clone(),
                revision_id: revision.id().to_string(),
                expected_head_revision_id: conflict.active_revision_id,
                parent_revision_ids: revision.parents().iter().map(ToString::to_string).collect(),
                expected_generation: conflict.remote_generation,
                root_tree_id: root_tree,
                manifest_json: serde_json::to_string(&manifest).map_err(|error| {
                    RuntimeError::new(CliErrorCode::Internal, error.to_string())
                })?,
                state: "queued".to_owned(),
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
        return Ok(CaptureOne {
            queued: true,
            hashed_files: scan.stats.hashed_files,
            reused_files: scan.stats.reused_files,
        });
    }

    if local_only {
        let parent = RevisionId::from_str(&state.local_head_revision_id).map_err(local_error)?;
        let operation = OperationId::from_uuid(Uuid::now_v7()).map_err(local_error)?;
        let revision = Revision::new(scan.manifest.root_tree(), vec![parent], author, operation)
            .map_err(local_error)?;
        let manifest = PublicSkillManifest::from_core(&scan.manifest);
        db.commit_local_only_revision(
            LocalRevisionRecord {
                operation_id: operation.to_string(),
                resource_id: record.resource_id.clone(),
                revision_id: revision.id().to_string(),
                expected_head_revision_id: parent.to_string(),
                parent_revision_ids: vec![parent.to_string()],
                expected_generation: state.base_generation,
                root_tree_id: root_tree,
                manifest_json: serde_json::to_string(&manifest).map_err(|error| {
                    RuntimeError::new(CliErrorCode::Internal, error.to_string())
                })?,
                state: "synced".to_owned(),
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
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
            expected_head_revision_id: parent.to_string(),
            parent_revision_ids: vec![parent.to_string()],
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

pub(crate) async fn upload_one(
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
    let request_hash =
        private_revision_request_hash(&denju_wire::PrivateRevisionRequestHashInput {
            operation_id: &operation,
            resource_id: &revision.resource_id,
            expected_generation,
            expected_head_revision_id: &revision.expected_head_revision_id,
            parent_revision_ids: &revision.parent_revision_ids,
            manifest: &manifest,
            revision_author_principal_id: None,
            fork_sync: None,
            historical_skill_name: None,
        })
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let request = PrivateRevisionRequest {
        operation_id: operation.clone(),
        resource_id: revision.resource_id.clone(),
        expected_generation,
        expected_head_revision_id: revision.expected_head_revision_id.clone(),
        parent_revision_ids: revision.parent_revision_ids.clone(),
        manifest,
        revision_author_principal_id: None,
        fork_sync: None,
        historical_skill_name: None,
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
    match committed {
        PrivateRevisionCommitResponse::Advanced {
            revision: committed,
        } => {
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
                        RuntimeError::new(
                            CliErrorCode::LocalState,
                            "registry generation is too large",
                        )
                    })?,
                    committed.revision_id,
                    now_unix_ms(),
                )
                .await
                .map_err(local_error)
        }
        PrivateRevisionCommitResponse::Diverged {
            resource_id,
            revision_id,
            conflict,
        } => {
            if resource_id != revision.resource_id || revision_id != revision.revision_id {
                return Err(RuntimeError::new(
                    CliErrorCode::LocalState,
                    "registry preserved a different detached private revision identity",
                ));
            }
            context
                .db
                .mark_local_revision_detached_stored(operation, now_unix_ms())
                .await
                .map_err(local_error)?;
            persist_workspace_conflict(
                context,
                &conflict,
                revision.root_tree_id.clone(),
                false,
                Vec::new(),
            )
            .await?;
            Ok(())
        }
    }
}

pub(crate) async fn persist_workspace_conflict(
    context: &InstalledContext,
    conflict: &PrivateWorkspaceConflict,
    working_root_tree_id: String,
    resolution_required: bool,
    conflict_paths: Vec<String>,
) -> Result<(), RuntimeError> {
    if conflict.head_revision_ids.len() != 2 {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            "registry returned a workspace conflict without exactly two heads",
        ));
    }
    let generation = i64::try_from(conflict.generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "workspace conflict generation exceeds local storage",
        )
    })?;
    context
        .db
        .save_workspace_content_conflict(
            WorkspaceContentConflictRecord {
                conflict_id: conflict.conflict_id.clone(),
                resource_id: conflict.resource_id.clone(),
                base_revision_id: conflict.base_revision_id.clone(),
                head_revision_ids: conflict.head_revision_ids.clone(),
                active_revision_id: conflict.active_revision_id.clone(),
                remote_generation: generation,
                working_root_tree_id,
                resolution_required,
                conflict_paths,
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    let message = format!(
        "private workspace {} has concurrent edits on two preserved heads",
        conflict.resource_id
    );
    context
        .db
        .pause_workspace(
            conflict.resource_id.clone(),
            WorkspaceStatus::Conflict,
            message,
            None,
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
#[path = "workspace_tests.rs"]
mod tests;
