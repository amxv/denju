use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
};

use denju_core::{
    BlobId, OperationId, OwnedSkillEntry, build_deterministic_skill_snapshot,
    rewrite_skill_document_name, skill_document_declared_name,
};
use denju_local::{
    ManagedSkillRecord, RegistryRenameState, WorkspaceStatus, apply_registry_rename,
    read_skill_source,
};
use denju_wire::{
    CliErrorCode, DeleteSkillResponse, DeprecateSkillRequest, DeprecateSkillResponse,
    HistoryPruneResponse, PrivateRevisionRequest, PublicSkillManifest, RenameSkillRequest,
    RenameSkillResponse, ResourceLifecycleRequest, UnpublishSkillResponse, UsageResponse,
    delete_skill_request_hash, deprecate_skill_request_hash, history_prune_request_hash,
    private_revision_request_hash, rename_skill_request_hash, unpublish_skill_request_hash,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    public::{client_error, installed_context, local_error, sync_once},
    setup::RuntimeError,
};

#[derive(Debug, Clone, Serialize)]
pub struct UsageOutcome {
    #[serde(flatten)]
    pub registry: UsageResponse,
    pub queued_local_bytes: u64,
}

pub async fn rename(locator: &str, new_name: &str) -> Result<RenameSkillResponse, RuntimeError> {
    let initial = installed_context(true).await?;
    let old = owned_record(&initial.db, locator).await?;
    let canonical = initial.paths.skills.join(&old.owner).join(&old.skill_name);
    let working_generation = std::fs::canonicalize(&canonical).map_err(local_error)?;
    let entries = read_skill_source(&working_generation).map_err(local_error)?;
    let declared = entries
        .iter()
        .find_map(|entry| match entry {
            denju_core::OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                Some(skill_document_declared_name(bytes))
            }
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::ContentVerification,
                format!("{locator} is missing SKILL.md"),
            )
        })?
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error.to_string()))?;
    let preserve_working = if declared == new_name {
        true
    } else if declared == old.skill_name {
        false
    } else {
        return Err(RuntimeError::new(
            CliErrorCode::ContentVerification,
            format!("{locator} declares name {declared}; requested rename target is {new_name}"),
        )
        .recovery(format!("denju rename {locator} {declared}")));
    };

    if !preserve_working {
        sync_once().await?;
    } else if let Some(state) = initial
        .db
        .workspace_state(old.resource_id.clone())
        .await
        .map_err(local_error)?
        && matches!(
            state.status,
            WorkspaceStatus::Queued | WorkspaceStatus::Conflict | WorkspaceStatus::Quota
        )
    {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            format!("{locator} has unresolved local work and cannot be renamed yet"),
        )
        .recovery("denju sync"));
    }

    let context = installed_context(true).await?;
    let old = owned_record(&context.db, locator).await?;
    let generation = u64::try_from(old.resource_generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "stored resource generation is invalid",
        )
    })?;
    let prepared_revision_operation_id = if preserve_working {
        Some(prepare_pending_rename_content(&context, &old, new_name, &entries, generation).await?)
    } else {
        None
    };
    let operation_id = new_operation_id()?;
    let request_hash = rename_skill_request_hash(
        &operation_id,
        &old.resource_id,
        generation,
        new_name,
        prepared_revision_operation_id.as_deref(),
    )
    .map_err(internal_error)?;
    let outcome = context
        .client
        .rename_skill(&RenameSkillRequest {
            operation_id,
            resource_id: old.resource_id.clone(),
            expected_generation: generation,
            new_name: new_name.to_owned(),
            prepared_revision_operation_id,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;

    let remote = context
        .client
        .private_skills()
        .await
        .map_err(client_error)?
        .skills
        .into_iter()
        .find(|skill| skill.resource_id == old.resource_id)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "renamed resource disappeared from the private workspace catalog",
            )
            .recovery("denju sync")
        })?;
    let authoritative = if preserve_working {
        None
    } else {
        if remote.snapshot.size_bytes > context.limits.max_transfer_bytes {
            return Err(RuntimeError::new(
                CliErrorCode::ContentVerification,
                format!(
                    "snapshot for {} exceeds registry transfer limit",
                    remote.locator
                ),
            ));
        }
        let manifest = remote
            .manifest
            .to_core()
            .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error))?;
        let snapshot = context
            .client
            .download_snapshot(&remote.snapshot)
            .await
            .map_err(client_error)?;
        Some((manifest, snapshot))
    };
    apply_registry_rename(
        &context.paths,
        &context.db,
        &context.roots,
        &ManagedSkillRecord {
            resource_id: old.resource_id.clone(),
            locator: old.locator,
            owner: old.owner,
            skill_name: old.skill_name,
            harness_name: old.harness_name,
            materialized_revision_id: old.materialized_revision_id,
        },
        RegistryRenameState {
            resource_id: remote.resource_id,
            owner: remote.owner,
            name: remote.name,
            locator: remote.locator,
            generation: i64::try_from(remote.generation).map_err(|_| {
                RuntimeError::new(CliErrorCode::LocalState, "registry generation is too large")
            })?,
            revision_id: remote.revision_id,
            root_tree_id: remote.manifest.root_tree_id,
        },
        preserve_working,
        authoritative
            .as_ref()
            .map(|(manifest, snapshot)| (manifest, snapshot.as_slice())),
    )
    .await
    .map_err(local_error)?;
    sync_once().await?;
    Ok(outcome)
}

pub async fn unpublish(locator: &str) -> Result<UnpublishSkillResponse, RuntimeError> {
    sync_once().await?;
    let context = installed_context(true).await?;
    let record = owned_record(&context.db, locator).await?;
    let request = lifecycle_request(
        &record.resource_id,
        record.resource_generation,
        unpublish_skill_request_hash,
    )?;
    let outcome = context
        .client
        .unpublish_skill(&request)
        .await
        .map_err(client_error)?;
    sync_once().await?;
    Ok(outcome)
}

pub async fn delete(
    locator: &str,
    json: bool,
    yes: bool,
) -> Result<DeleteSkillResponse, RuntimeError> {
    confirm_destructive(
        json,
        yes,
        &format!("Delete {locator}? [y/N] "),
        &format!("denju delete {locator} --yes"),
    )?;
    sync_once().await?;
    let context = installed_context(true).await?;
    let record = owned_record(&context.db, locator).await?;
    let request = lifecycle_request(
        &record.resource_id,
        record.resource_generation,
        delete_skill_request_hash,
    )?;
    let outcome = context
        .client
        .delete_skill(&request)
        .await
        .map_err(client_error)?;
    sync_once().await?;
    Ok(outcome)
}

pub async fn deprecate(
    locator: &str,
    replacement: Option<&str>,
    undo: bool,
) -> Result<DeprecateSkillResponse, RuntimeError> {
    if undo && replacement.is_some() {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "--undo cannot be combined with --replacement",
        ));
    }
    sync_once().await?;
    let context = installed_context(true).await?;
    let record = owned_record(&context.db, locator).await?;
    let replacement_resource_id = match replacement {
        Some(locator) => Some(
            context
                .client
                .show_public_skill(locator)
                .await
                .map_err(client_error)?
                .skill
                .resource_id,
        ),
        None => None,
    };
    let generation = u64::try_from(record.resource_generation)
        .map_err(|_| RuntimeError::new(CliErrorCode::LocalState, "stored generation is invalid"))?;
    let operation_id = new_operation_id()?;
    let request_hash = deprecate_skill_request_hash(
        &operation_id,
        &record.resource_id,
        generation,
        !undo,
        replacement_resource_id.as_deref(),
    )
    .map_err(internal_error)?;
    let outcome = context
        .client
        .deprecate_skill(&DeprecateSkillRequest {
            operation_id,
            resource_id: record.resource_id,
            expected_generation: generation,
            deprecated: !undo,
            replacement_resource_id,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    sync_once().await?;
    Ok(outcome)
}

pub async fn usage() -> Result<UsageOutcome, RuntimeError> {
    let context = installed_context(true).await?;
    let registry = context.client.usage().await.map_err(client_error)?;
    let queued = context
        .db
        .queued_local_revisions()
        .await
        .map_err(local_error)?;
    let mut blobs = BTreeMap::<String, u64>::new();
    for revision in queued {
        let manifest: PublicSkillManifest = serde_json::from_str(&revision.manifest_json)
            .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
        for entry in manifest.entries {
            if let denju_wire::PublicSkillManifestEntry::File { blob_id, size, .. } = entry {
                blobs.entry(blob_id).or_insert(size);
            }
        }
    }
    let queued_local_bytes = blobs.values().try_fold(0_u64, |total, size| {
        total.checked_add(*size).ok_or_else(|| {
            RuntimeError::new(CliErrorCode::LocalState, "queued byte count overflow")
        })
    })?;
    Ok(UsageOutcome {
        registry,
        queued_local_bytes,
    })
}

pub async fn prune_history(
    locator: &str,
    json: bool,
    yes: bool,
) -> Result<HistoryPruneResponse, RuntimeError> {
    confirm_destructive(
        json,
        yes,
        &format!("Prune eligible private history for {locator}? [y/N] "),
        &format!("denju history prune {locator} --yes"),
    )?;
    let context = installed_context(true).await?;
    let (_pass, blockers) =
        crate::workspace::capture_local_edits(&context.paths, &context.db, false).await?;
    if let Some(blocker) = blockers.into_iter().next() {
        return Err(blocker);
    }
    let record = owned_record(&context.db, locator).await?;
    if let Some(state) = context
        .db
        .workspace_state(record.resource_id.clone())
        .await
        .map_err(local_error)?
        && matches!(
            state.status,
            WorkspaceStatus::Conflict
                | WorkspaceStatus::PendingRename
                | WorkspaceStatus::PausedValidation
        )
    {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            format!("{locator} has unresolved local workspace state"),
        )
        .recovery("denju sync"));
    }
    let request = lifecycle_request(
        &record.resource_id,
        record.resource_generation,
        history_prune_request_hash,
    )?;
    let outcome = context
        .client
        .prune_skill_history(&request)
        .await
        .map_err(client_error)?;
    let new_generation = i64::try_from(outcome.generation).map_err(|_| {
        RuntimeError::new(CliErrorCode::LocalState, "registry generation is too large")
    })?;
    context
        .db
        .advance_owned_metadata_generation(
            record.resource_id,
            record.resource_generation,
            new_generation,
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    match sync_once().await {
        Ok(_) => {}
        Err(error) if error.code == CliErrorCode::QuotaExceeded => {}
        Err(error) => return Err(error),
    }
    Ok(outcome)
}

async fn owned_record(
    db: &denju_local::LocalDatabase,
    locator: &str,
) -> Result<denju_local::OwnedSkillRecord, RuntimeError> {
    db.owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|record| record.locator == locator)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::NotFound,
                format!("{locator} is not an owned skill on this identity"),
            )
        })
}

async fn prepare_pending_rename_content(
    context: &crate::public::InstalledContext,
    old: &denju_local::OwnedSkillRecord,
    new_name: &str,
    working_entries: &[OwnedSkillEntry],
    generation: u64,
) -> Result<String, RuntimeError> {
    let mut prepared_entries = working_entries.to_vec();
    let skill_md = prepared_entries
        .iter_mut()
        .find_map(|entry| match entry {
            OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => Some(bytes),
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::ContentVerification,
                format!("{} is missing SKILL.md", old.locator),
            )
        })?;
    *skill_md = rewrite_skill_document_name(new_name, skill_md, &old.skill_name)
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error.to_string()))?;
    let snapshot = build_deterministic_skill_snapshot(&old.skill_name, &prepared_entries)
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error.to_string()))?;
    let manifest = PublicSkillManifest::from_core(snapshot.manifest());
    let operation_id = new_operation_id()?;
    let request_hash = private_revision_request_hash(
        &operation_id,
        &old.resource_id,
        generation,
        &old.desired_revision_id,
        std::slice::from_ref(&old.desired_revision_id),
        &manifest,
    )
    .map_err(internal_error)?;
    let prepared = context
        .client
        .prepare_private_revision(&PrivateRevisionRequest {
            operation_id: operation_id.clone(),
            resource_id: old.resource_id.clone(),
            expected_generation: generation,
            expected_head_revision_id: old.desired_revision_id.clone(),
            parent_revision_ids: vec![old.desired_revision_id.clone()],
            manifest,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    if prepared.state != denju_wire::PrivateRevisionOperationState::Prepared {
        return Err(RuntimeError::new(
            CliErrorCode::Internal,
            "fresh rename preparation unexpectedly committed a private revision",
        ));
    }
    let bytes_by_blob = prepared_entries
        .iter()
        .filter_map(|entry| match entry {
            OwnedSkillEntry::File { bytes, .. } => Some((BlobId::hash(bytes).to_string(), bytes)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    for upload in &prepared.uploads {
        let bytes = bytes_by_blob.get(&upload.blob_id).ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::Internal,
                format!("rename staging requested unknown blob {}", upload.blob_id),
            )
        })?;
        context
            .client
            .upload_staged_blob(upload, bytes)
            .await
            .map_err(client_error)?;
    }
    Ok(operation_id)
}

fn lifecycle_request(
    resource_id: &str,
    generation: i64,
    hash: fn(&str, &str, u64) -> Result<denju_wire::RequestHash, denju_wire::RequestHashError>,
) -> Result<ResourceLifecycleRequest, RuntimeError> {
    let generation = u64::try_from(generation)
        .map_err(|_| RuntimeError::new(CliErrorCode::LocalState, "stored generation is invalid"))?;
    let operation_id = new_operation_id()?;
    let request_hash = hash(&operation_id, resource_id, generation).map_err(internal_error)?;
    Ok(ResourceLifecycleRequest {
        operation_id,
        resource_id: resource_id.to_owned(),
        expected_generation: generation,
        request_hash: request_hash.to_string(),
    })
}

fn new_operation_id() -> Result<String, RuntimeError> {
    OperationId::from_uuid(Uuid::now_v7())
        .map(|id| id.to_string())
        .map_err(internal_error)
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

fn confirm_destructive(
    json: bool,
    yes: bool,
    prompt: &str,
    recovery: &str,
) -> Result<(), RuntimeError> {
    if yes {
        return Ok(());
    }
    if json || !io::stdin().is_terminal() {
        return Err(RuntimeError::new(
            CliErrorCode::ConfirmationRequired,
            "destructive operation requires confirmation",
        )
        .recovery(recovery));
    }
    eprint!("{prompt}");
    io::stderr()
        .flush()
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(RuntimeError::new(
            CliErrorCode::ConfirmationRequired,
            "destructive operation was not confirmed",
        )
        .recovery(recovery))
    }
}

fn internal_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::Internal, error.to_string())
}
