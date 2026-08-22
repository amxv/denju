use std::{fs, str::FromStr};

use denju_core::{ResourceId, RevisionId};
use denju_local::{
    DesiredSkillMaterialization, ManagedSkillRecord, OwnedSkillRecord, RegistryRenameState,
    WorkspaceStatus, apply_registry_rename, materialize_skill_snapshot,
};
use denju_wire::{CliErrorCode, PrivateSkill};

use crate::{
    context::{InstalledContext, client_error, local_error, now_unix_ms},
    setup::RuntimeError,
};

pub(crate) async fn preseed_owned_desired_if_missing(
    context: &InstalledContext,
    remote: &PrivateSkill,
) -> Result<(), RuntimeError> {
    if context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .iter()
        .any(|record| record.resource_id == remote.resource_id)
    {
        return Ok(());
    }
    let resource_generation = i64::try_from(remote.generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "owned resource generation exceeds local storage",
        )
    })?;
    let workspace_generation = i64::try_from(remote.workspace_generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "owned workspace generation exceeds local storage",
        )
    })?;
    context
        .db
        .upsert_owned_skill_desired(
            OwnedSkillRecord {
                resource_id: remote.resource_id.clone(),
                locator: remote.locator.clone(),
                owner: remote.owner.clone(),
                skill_name: remote.name.clone(),
                resource_generation,
                workspace_generation,
                desired_revision_id: remote.revision_id.clone(),
                harness_name: None,
                materialized_revision_id: None,
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)
}

pub(crate) async fn sync_owned_skill(
    context: &InstalledContext,
    remote: &PrivateSkill,
) -> Result<usize, RuntimeError> {
    let existing = context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|record| record.resource_id == remote.resource_id);
    let resource_generation = i64::try_from(remote.generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "owned resource generation exceeds local storage",
        )
    })?;
    let workspace_generation = i64::try_from(remote.workspace_generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "owned workspace generation exceeds local storage",
        )
    })?;
    if remote.conflicts.len() > 1 {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            format!(
                "{} has multiple unresolved workspace conflicts; registry state is inconsistent",
                remote.locator
            ),
        )
        .recovery("denju doctor"));
    }
    if existing.is_some()
        && let Some(conflict) = remote.conflicts.first()
    {
        return crate::workspace_merge::reconcile_workspace_conflict(context, remote, conflict)
            .await;
    }
    if let Some(local) = existing.as_ref() {
        crate::workspace_merge::settle_resolved_workspace_conflict(context, remote, local).await?;
    }
    if let Some(local) = existing.as_ref()
        && (local.owner != remote.owner || local.skill_name != remote.name)
    {
        let workspace = context
            .db
            .workspace_state(remote.resource_id.clone())
            .await
            .map_err(local_error)?
            .ok_or_else(|| {
                RuntimeError::new(
                    CliErrorCode::LocalState,
                    format!("{} has no local workspace state", local.locator),
                )
                .recovery("denju doctor")
            })?;
        let preserve_working = workspace.status == WorkspaceStatus::PendingRename
            && workspace.pending_rename.as_deref() == Some(remote.name.as_str());
        if workspace.status != WorkspaceStatus::Clean && !preserve_working {
            return Err(RuntimeError::new(
                CliErrorCode::LocalState,
                format!(
                    "{} changed identity remotely while local work is unresolved",
                    local.locator
                ),
            )
            .recovery("denju sync"));
        }
        let manifest = remote
            .manifest
            .to_core()
            .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error))?;
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
            Some(
                context
                    .client
                    .download_snapshot(&remote.snapshot)
                    .await
                    .map_err(client_error)?,
            )
        };
        apply_registry_rename(
            &context.paths,
            &context.db,
            &context.roots,
            &ManagedSkillRecord {
                resource_id: local.resource_id.clone(),
                locator: local.locator.clone(),
                owner: local.owner.clone(),
                skill_name: local.skill_name.clone(),
                harness_name: local.harness_name.clone(),
                materialized_revision_id: local.materialized_revision_id.clone(),
            },
            RegistryRenameState {
                resource_id: remote.resource_id.clone(),
                owner: remote.owner.clone(),
                name: remote.name.clone(),
                locator: remote.locator.clone(),
                resource_generation,
                workspace_generation,
                revision_id: remote.revision_id.clone(),
                root_tree_id: manifest.root_tree().to_string(),
            },
            preserve_working,
            authoritative
                .as_ref()
                .map(|snapshot| (&manifest, snapshot.as_slice())),
        )
        .await
        .map_err(local_error)?;
        return Ok(usize::from(!preserve_working));
    }
    context
        .db
        .upsert_owned_skill_desired(
            OwnedSkillRecord {
                resource_id: remote.resource_id.clone(),
                locator: remote.locator.clone(),
                owner: remote.owner.clone(),
                skill_name: remote.name.clone(),
                resource_generation,
                workspace_generation,
                desired_revision_id: remote.revision_id.clone(),
                harness_name: None,
                materialized_revision_id: None,
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    if let Some(state) = context
        .db
        .workspace_state(remote.resource_id.clone())
        .await
        .map_err(local_error)?
        && state.status != WorkspaceStatus::Clean
    {
        return Ok(0);
    }
    let already_current = existing.as_ref().is_some_and(|record| {
        record.materialized_revision_id.as_deref() == Some(remote.revision_id.as_str())
            && canonical_targets_revision(
                context,
                &remote.resource_id,
                &remote.owner,
                &remote.name,
                &remote.revision_id,
            )
    });
    if already_current {
        let root_tree = remote
            .manifest
            .to_core()
            .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error))?
            .root_tree()
            .to_string();
        let working_generation =
            fs::canonicalize(context.paths.skills.join(&remote.owner).join(&remote.name))
                .map_err(local_error)?;
        context
            .db
            .ensure_workspace_baseline(
                remote.resource_id.clone(),
                workspace_generation,
                remote.revision_id.clone(),
                root_tree.clone(),
                working_generation.display().to_string(),
                now_unix_ms(),
            )
            .await
            .map_err(local_error)?;
        // A publish advances the resource generation while intentionally retaining the same
        // private workspace revision. Refresh the clean workspace CAS baseline even when no
        // bytes need rematerialization, otherwise the next local edit falsely conflicts with
        // the generation change caused by our own publish.
        context
            .db
            .advance_clean_workspace_baseline(
                remote.resource_id.clone(),
                workspace_generation,
                remote.revision_id.clone(),
                root_tree,
                working_generation.display().to_string(),
                now_unix_ms(),
            )
            .await
            .map_err(local_error)?;
        return Ok(0);
    }
    if remote.snapshot.size_bytes > context.limits.max_transfer_bytes {
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
        .download_snapshot(&remote.snapshot)
        .await
        .map_err(client_error)?;
    let desired = DesiredSkillMaterialization {
        resource_id: ResourceId::from_str(&remote.resource_id).map_err(local_error)?,
        owner: remote.owner.clone(),
        skill_name: remote.name.clone(),
        revision_id: RevisionId::from_str(&remote.revision_id).map_err(local_error)?,
        manifest: remote
            .manifest
            .to_core()
            .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error))?,
    };
    let generation = materialize_skill_snapshot(&context.paths, &context.db, &desired, &bytes)
        .await
        .map_err(|error| {
            RuntimeError::new(CliErrorCode::ContentVerification, error.to_string())
                .recovery("denju sync")
        })?;
    context
        .db
        .clear_workspace_file_index(remote.resource_id.clone())
        .await
        .map_err(local_error)?;
    context
        .db
        .ensure_workspace_baseline(
            remote.resource_id.clone(),
            workspace_generation,
            remote.revision_id.clone(),
            desired.manifest.root_tree().to_string(),
            generation.display().to_string(),
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    context
        .db
        .advance_clean_workspace_baseline(
            remote.resource_id.clone(),
            workspace_generation,
            remote.revision_id.clone(),
            desired.manifest.root_tree().to_string(),
            generation.display().to_string(),
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    if let Some(conflict) = remote.conflicts.first() {
        return crate::workspace_merge::reconcile_workspace_conflict(context, remote, conflict)
            .await
            .map(|merged| 1 + merged);
    }
    Ok(1)
}

fn canonical_targets_revision(
    context: &InstalledContext,
    resource_id: &str,
    owner: &str,
    skill_name: &str,
    revision_id: &str,
) -> bool {
    let canonical = context.paths.skills.join(owner).join(skill_name);
    let expected = context
        .paths
        .generations
        .join(resource_id)
        .join(revision_id);
    match (fs::canonicalize(canonical), fs::canonicalize(expected)) {
        (Ok(canonical), Ok(expected)) => canonical == expected,
        _ => false,
    }
}
