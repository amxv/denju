use std::str::FromStr;

use denju_core::{
    AuthorPrincipalId, OperationId, ResourceId, Revision, RevisionId, SkillMergeResult,
    build_deterministic_skill_snapshot, merge_skill_entries, validate_skill_snapshot,
};
use denju_local::{
    DesiredSkillMaterialization, LocalRevisionRecord, OwnedSkillRecord, materialize_skill_snapshot,
    scan_owned_workspace, store_workspace_entries,
};
use denju_wire::{
    CliErrorCode, PrivateRevisionResponse, PrivateSkill, PrivateWorkspaceConflict,
    PublicSkillManifest,
};

use crate::{
    public::InstalledContext,
    setup::RuntimeError,
    workspace::{persist_workspace_conflict, upload_one},
};

pub(crate) async fn reconcile_workspace_conflict(
    context: &InstalledContext,
    remote: &PrivateSkill,
    conflict: &PrivateWorkspaceConflict,
) -> Result<usize, RuntimeError> {
    if conflict.resource_id != remote.resource_id || conflict.head_revision_ids.len() != 2 {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            "registry returned an invalid private workspace conflict",
        ));
    }
    let record = context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|record| record.resource_id == remote.resource_id)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                format!("{} has no owned local workspace", remote.locator),
            )
        })?;

    // Re-scan immediately before deciding whether an automatic merge is safe. The losing device
    // intentionally still exposes its detached head, while the winning device exposes the active
    // head; either is a valid untouched starting point. Anything else is newer local work and is
    // preserved until the user explicitly resolves the conflict.
    let current_scan = scan_owned_workspace(&context.paths, &context.db, &record, false)
        .await
        .map_err(|error| {
            RuntimeError::new(CliErrorCode::LocalState, error.to_string()).recovery("denju sync")
        })?;
    let current_root = current_scan.manifest.root_tree().to_string();

    let base = fetch_revision_snapshot(context, remote, &conflict.base_revision_id).await?;
    let head_a = fetch_revision_snapshot(context, remote, &conflict.head_revision_ids[0]).await?;
    let head_b = fetch_revision_snapshot(context, remote, &conflict.head_revision_ids[1]).await?;
    let current_is_preserved_head =
        current_root == head_a.root_tree_id || current_root == head_b.root_tree_id;
    let has_extra_queued_save = context
        .db
        .queued_local_revisions()
        .await
        .map_err(local_error)?
        .iter()
        .any(|revision| revision.resource_id == remote.resource_id);
    if !current_is_preserved_head || has_extra_queued_save {
        persist_workspace_conflict(context, conflict, current_root, true, Vec::new()).await?;
        let message = format!(
            "{} has newer local work in addition to two preserved concurrent heads; the working tree was preserved",
            remote.locator
        );
        return Err(
            RuntimeError::new(CliErrorCode::LocalState, message).recovery(format!(
                "denju diff {} {} {}",
                remote.locator, conflict.head_revision_ids[0], conflict.head_revision_ids[1]
            )),
        );
    }

    match merge_skill_entries(&base.entries, &head_a.entries, &head_b.entries) {
        SkillMergeResult::Conflicted { conflicts } => {
            let paths = conflicts
                .iter()
                .map(|item| item.path.clone())
                .collect::<Vec<_>>();
            persist_workspace_conflict(context, conflict, current_root, true, paths.clone())
                .await?;
            let joined = paths.join(", ");
            let message = format!(
                "{} has concurrent edits that require resolution in: {joined}; both revision heads are preserved",
                remote.locator
            );
            Err(
                RuntimeError::new(CliErrorCode::LocalState, message).recovery(format!(
                    "denju diff {} {} {}",
                    remote.locator, conflict.head_revision_ids[0], conflict.head_revision_ids[1]
                )),
            )
        }
        SkillMergeResult::Clean { entries } => {
            persist_workspace_conflict(context, conflict, current_root, false, Vec::new()).await?;
            commit_merge_entries(context, remote, conflict, entries)
                .await
                .map(|_| 1)
        }
    }
}

pub(crate) async fn settle_resolved_workspace_conflict(
    context: &InstalledContext,
    remote: &PrivateSkill,
    record: &OwnedSkillRecord,
) -> Result<(), RuntimeError> {
    let Some(conflict) = context
        .db
        .workspace_content_conflict(remote.resource_id.clone())
        .await
        .map_err(local_error)?
    else {
        return Ok(());
    };
    if !remote.conflicts.is_empty() {
        return Ok(());
    }
    let has_queued = context
        .db
        .queued_local_revisions()
        .await
        .map_err(local_error)?
        .iter()
        .any(|revision| revision.resource_id == remote.resource_id);
    let scan = scan_owned_workspace(&context.paths, &context.db, record, false)
        .await
        .map_err(|error| {
            RuntimeError::new(CliErrorCode::LocalState, error.to_string()).recovery("denju status")
        })?;
    if has_queued || scan.manifest.root_tree().to_string() != conflict.working_root_tree_id {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            format!(
                "{} was resolved on another device while this device had newer local resolution work; the working tree was preserved",
                remote.locator
            ),
        )
        .recovery("denju status"));
    }
    context
        .db
        .clear_workspace_content_conflict(remote.resource_id.clone())
        .await
        .map_err(local_error)?;
    context
        .db
        .resume_workspace(remote.resource_id.clone(), now_unix_ms())
        .await
        .map_err(local_error)
}

pub(crate) async fn resolve_workspace_conflict_with_revision(
    context: &InstalledContext,
    locator: &str,
    revision_id: &str,
) -> Result<PrivateRevisionResponse, RuntimeError> {
    let remote = context
        .client
        .private_skills()
        .await
        .map_err(client_error)?
        .skills
        .into_iter()
        .find(|skill| skill.locator == locator)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::NotFound,
                format!("{locator} is not an owned private skill"),
            )
        })?;
    let conflict = remote.conflicts.first().cloned().ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::InvalidArguments,
            format!("{locator} has no active concurrent-edit conflict"),
        )
    })?;
    if remote.conflicts.len() != 1 {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            format!("{locator} has inconsistent concurrent-edit conflict state"),
        )
        .recovery("denju doctor"));
    }
    let target = fetch_revision_snapshot(context, &remote, revision_id).await?;
    commit_merge_entries(context, &remote, &conflict, target.entries).await
}

async fn commit_merge_entries(
    context: &InstalledContext,
    remote: &PrivateSkill,
    conflict: &PrivateWorkspaceConflict,
    entries: Vec<denju_core::OwnedSkillEntry>,
) -> Result<PrivateRevisionResponse, RuntimeError> {
    let identity = context
        .db
        .identity()
        .await
        .map_err(local_error)?
        .ok_or_else(|| {
            RuntimeError::new(CliErrorCode::LocalState, "claimed identity is missing")
        })?;
    let author =
        AuthorPrincipalId::from_str(identity.author_principal_id.as_deref().ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "claimed identity has no author principal",
            )
        })?)
        .map_err(local_error)?;
    let operation = OperationId::from_str(&conflict.conflict_id).map_err(local_error)?;
    let parents = conflict
        .head_revision_ids
        .iter()
        .map(|value| RevisionId::from_str(value).map_err(local_error))
        .collect::<Result<Vec<_>, _>>()?;
    let snapshot =
        build_deterministic_skill_snapshot(&remote.name, &entries).map_err(local_error)?;
    let revision = Revision::new(snapshot.manifest().root_tree(), parents, author, operation)
        .map_err(local_error)?;
    store_workspace_entries(&context.paths, &entries).map_err(local_error)?;
    let manifest = PublicSkillManifest::from_core(snapshot.manifest());
    context
        .db
        .enqueue_local_revision(
            LocalRevisionRecord {
                operation_id: operation.to_string(),
                resource_id: remote.resource_id.clone(),
                revision_id: revision.id().to_string(),
                expected_head_revision_id: conflict.active_revision_id.clone(),
                parent_revision_ids: revision.parents().iter().map(ToString::to_string).collect(),
                expected_generation: i64::try_from(conflict.generation).map_err(|_| {
                    RuntimeError::new(
                        CliErrorCode::LocalState,
                        "workspace conflict generation exceeds local storage",
                    )
                })?,
                root_tree_id: snapshot.manifest().root_tree().to_string(),
                manifest_json: serde_json::to_string(&manifest).map_err(|error| {
                    RuntimeError::new(CliErrorCode::Internal, error.to_string())
                })?,
                state: "queued".to_owned(),
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    let merge_revision = context
        .db
        .queued_local_revisions()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|item| item.operation_id == conflict.conflict_id)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "queued merge revision disappeared",
            )
        })?;
    upload_one(context, &merge_revision).await?;

    let refreshed = context
        .client
        .private_skills()
        .await
        .map_err(client_error)?
        .skills
        .into_iter()
        .find(|skill| skill.resource_id == remote.resource_id)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::NotFound,
                format!("{} disappeared after merge", remote.locator),
            )
        })?;
    if !refreshed.conflicts.is_empty() || refreshed.revision_id != revision.id().to_string() {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            format!(
                "{} merge did not settle the active workspace ref",
                remote.locator
            ),
        )
        .recovery("denju sync"));
    }
    let generation = i64::try_from(refreshed.generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "merged resource generation exceeds local storage",
        )
    })?;
    let workspace_generation = i64::try_from(refreshed.workspace_generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "merged workspace generation exceeds local storage",
        )
    })?;
    context
        .db
        .upsert_owned_skill_desired(
            OwnedSkillRecord {
                resource_id: refreshed.resource_id.clone(),
                locator: refreshed.locator.clone(),
                owner: refreshed.owner.clone(),
                skill_name: refreshed.name.clone(),
                resource_generation: generation,
                workspace_generation,
                desired_revision_id: refreshed.revision_id.clone(),
                harness_name: None,
                materialized_revision_id: None,
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    let desired = DesiredSkillMaterialization {
        resource_id: ResourceId::from_str(&refreshed.resource_id).map_err(local_error)?,
        owner: refreshed.owner.clone(),
        skill_name: refreshed.name.clone(),
        revision_id: RevisionId::from_str(&refreshed.revision_id).map_err(local_error)?,
        manifest: refreshed
            .manifest
            .to_core()
            .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error))?,
    };
    let generation_path =
        materialize_skill_snapshot(&context.paths, &context.db, &desired, snapshot.bytes())
            .await
            .map_err(local_error)?;
    context
        .db
        .clear_workspace_file_index(refreshed.resource_id.clone())
        .await
        .map_err(local_error)?;
    context
        .db
        .advance_clean_workspace_baseline(
            refreshed.resource_id.clone(),
            workspace_generation,
            refreshed.revision_id.clone(),
            desired.manifest.root_tree().to_string(),
            generation_path.display().to_string(),
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    context
        .db
        .clear_workspace_content_conflict(refreshed.resource_id.clone())
        .await
        .map_err(local_error)?;
    Ok(PrivateRevisionResponse {
        resource_id: refreshed.resource_id,
        generation: refreshed.workspace_generation,
        revision_id: refreshed.revision_id,
        description: refreshed.description,
        manifest: refreshed.manifest,
    })
}

struct RevisionSnapshot {
    entries: Vec<denju_core::OwnedSkillEntry>,
    root_tree_id: String,
}

async fn fetch_revision_snapshot(
    context: &InstalledContext,
    remote: &PrivateSkill,
    revision_id: &str,
) -> Result<RevisionSnapshot, RuntimeError> {
    let detail = fetch_revision_detail(context, remote, revision_id).await?;
    if detail.snapshot.size_bytes > context.limits.max_transfer_bytes {
        return Err(RuntimeError::new(
            CliErrorCode::ContentVerification,
            format!(
                "snapshot for {} exceeds registry transfer limit",
                remote.locator
            ),
        ));
    }
    let bytes = context
        .client
        .download_snapshot(&detail.snapshot)
        .await
        .map_err(client_error)?;
    let manifest = detail
        .manifest
        .to_core()
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error))?;
    let entries = validate_skill_snapshot(&remote.name, &manifest, &bytes).map_err(local_error)?;
    Ok(RevisionSnapshot {
        entries,
        root_tree_id: manifest.root_tree().to_string(),
    })
}

async fn fetch_revision_detail(
    context: &InstalledContext,
    remote: &PrivateSkill,
    revision_id: &str,
) -> Result<denju_wire::SkillRevisionDetail, RuntimeError> {
    let detail = context
        .client
        .skill_revision(&remote.locator, revision_id)
        .await
        .map_err(client_error)?;
    if detail.resource_id != remote.resource_id || detail.revision_id != revision_id {
        return Err(RuntimeError::new(
            CliErrorCode::ContentVerification,
            "registry revision detail did not match the requested private workspace history",
        ));
    }
    Ok(detail)
}

fn client_error(error: denju_client::ClientError) -> RuntimeError {
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
