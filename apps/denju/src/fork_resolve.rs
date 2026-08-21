use std::{fs, str::FromStr};

use denju_core::{
    ResourceLocator, SkillMergeResult, build_deterministic_skill_snapshot, merge_skill_entries,
    rewrite_skill_document_name, skill_document_declared_name, validate_skill_snapshot,
};
use denju_local::{
    LocalForkRecord, ManagedDesiredKind, ManagedSkillRecord, journaled_remove_managed_skill,
};
use denju_wire::{
    CliErrorCode, PrivateRevisionCommitRequest, PrivateRevisionCommitResponse,
    PrivateRevisionRequest, PrivateSkill, PublicSkillManifest, SubscriptionMutationKind,
    private_revision_request_hash,
};

use crate::{
    fork_ops::{
        ForkResolveOutcome, fetch_revision_entries, find_owned, install_owned_revision,
        internal_error, local_revision_snapshot, new_operation, ordered_history,
        promote_local_forks, require_identity, upload_entries,
    },
    public::{InstalledContext, client_error, installed_context, local_error, now_unix_ms},
    setup::RuntimeError,
};

pub(crate) async fn resolve(
    upstream_locator: &str,
    as_name: Option<&str>,
    merge_into: Option<&str>,
    discard: bool,
) -> Result<ForkResolveOutcome, RuntimeError> {
    let choices =
        usize::from(as_name.is_some()) + usize::from(merge_into.is_some()) + usize::from(discard);
    if choices != 1 {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "choose exactly one of `--as <new-name>`, `--merge-into @you/skill`, or `--discard`",
        ));
    }
    let context = installed_context(true).await?;
    let identity = require_identity(&context).await?;
    let upstream = context
        .client
        .subscription_target(upstream_locator)
        .await
        .map_err(client_error)?;
    let mut fork = context
        .db
        .local_fork_for_upstream(upstream.resource_id.clone())
        .await
        .map_err(local_error)?
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::NotFound,
                format!("no unresolved local fork exists for {}", upstream.locator),
            )
        })?;
    if fork.state != "name_conflict" {
        return Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            format!(
                "{} does not have a fork name collision to resolve",
                upstream.locator
            ),
        ));
    }

    if discard {
        let dirty_upstream_generation = context
            .paths
            .generations
            .join(&fork.upstream_resource_id)
            .join(&fork.created_from_revision_id);
        if dirty_upstream_generation.exists() {
            fs::remove_dir_all(&dirty_upstream_generation).map_err(local_error)?;
        }
        remove_local_fork(&context, &fork).await?;
        crate::public::sync_once().await?;
        return Ok(ForkResolveOutcome {
            state: "discarded",
            upstream_locator: upstream.locator,
            locator: None,
        });
    }

    if let Some(new_name) = as_name {
        denju_core::validate_skill_name(new_name).map_err(|error| {
            RuntimeError::new(CliErrorCode::InvalidArguments, error.to_string())
        })?;
        fork.desired_name = new_name.to_owned();
        fork.state = "local".to_owned();
        context
            .db
            .save_local_fork(fork.clone(), now_unix_ms())
            .await
            .map_err(local_error)?;
        promote_local_forks(&context).await?;
        if let Some(blocked) = context
            .db
            .local_fork_for_upstream(upstream.resource_id.clone())
            .await
            .map_err(local_error)?
        {
            if blocked.state == "name_conflict" {
                return Err(RuntimeError::new(
                    CliErrorCode::InvalidArguments,
                    format!(
                        "@{}/{} already exists; choose another `--as` name",
                        identity.username, new_name
                    ),
                ));
            }
            return Err(RuntimeError::new(
                CliErrorCode::LocalState,
                "fork collision resolution did not finish its durable promotion",
            )
            .recovery("denju sync"));
        }
        return Ok(ForkResolveOutcome {
            state: "renamed",
            upstream_locator: upstream.locator,
            locator: Some(format!(
                "@{}/{}",
                identity
                    .username
                    .strip_prefix('@')
                    .unwrap_or(&identity.username),
                new_name
            )),
        });
    }

    let target_locator = merge_into.expect("exactly one resolution choice checked");
    let target = find_owned(&context, target_locator).await?;
    let history = ordered_history(
        &context
            .db
            .local_revision_history(fork.resource_id.clone())
            .await
            .map_err(local_error)?,
        &fork.created_from_revision_id,
    )?;
    let local_head = history.last().ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "local fork has no revision history",
        )
    })?;
    let (_, local_entries, _) = local_revision_snapshot(&context, local_head)?;
    let upstream_locator_parsed =
        ResourceLocator::from_str(&fork.upstream_locator).map_err(local_error)?;
    let base_entries = fetch_revision_entries(
        &context,
        &fork.upstream_locator,
        upstream_locator_parsed.name(),
        &fork.created_from_revision_id,
    )
    .await?;
    let target_bytes = context
        .client
        .download_snapshot(&target.snapshot)
        .await
        .map_err(client_error)?;
    let target_manifest = target
        .manifest
        .to_core()
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error))?;
    let target_entries = validate_skill_snapshot(&target.name, &target_manifest, &target_bytes)
        .map_err(local_error)?;
    let mut merged = match merge_skill_entries(&base_entries, &target_entries, &local_entries) {
        SkillMergeResult::Clean { entries } => entries,
        SkillMergeResult::Conflicted { conflicts } => {
            let paths = conflicts
                .into_iter()
                .map(|conflict| conflict.path)
                .collect::<Vec<_>>();
            return Err(RuntimeError::new(
                CliErrorCode::LocalState,
                format!(
                    "cannot merge local fork into {}: conflicts in {}",
                    target.locator,
                    paths.join(", ")
                ),
            ));
        }
    };
    normalize_skill_name(&mut merged, &target.name)?;
    let snapshot =
        build_deterministic_skill_snapshot(&target.name, &merged).map_err(local_error)?;
    let manifest = PublicSkillManifest::from_core(snapshot.manifest());
    if manifest == target.manifest {
        finish_merge_resolution(
            &context,
            &fork,
            &upstream,
            &target,
            &manifest,
            snapshot.bytes(),
        )
        .await?;
        return Ok(ForkResolveOutcome {
            state: "merged",
            upstream_locator: upstream.locator,
            locator: Some(target.locator),
        });
    }
    let operation = new_operation()?;
    let operation_id = operation.to_string();
    let request_hash =
        private_revision_request_hash(&denju_wire::PrivateRevisionRequestHashInput {
            operation_id: &operation_id,
            resource_id: &target.resource_id,
            expected_generation: target.generation,
            expected_head_revision_id: &target.revision_id,
            parent_revision_ids: std::slice::from_ref(&target.revision_id),
            manifest: &manifest,
            revision_author_principal_id: None,
            fork_sync: None,
            historical_skill_name: None,
        })
        .map_err(internal_error)?;
    let prepared = context
        .client
        .prepare_private_revision(&PrivateRevisionRequest {
            operation_id: operation_id.clone(),
            resource_id: target.resource_id.clone(),
            expected_generation: target.generation,
            expected_head_revision_id: target.revision_id.clone(),
            parent_revision_ids: vec![target.revision_id.clone()],
            manifest: manifest.clone(),
            revision_author_principal_id: None,
            fork_sync: None,
            historical_skill_name: None,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    upload_entries(&context, &prepared.uploads, &merged).await?;
    let committed = context
        .client
        .commit_private_revision(&PrivateRevisionCommitRequest {
            operation_id: operation.to_string(),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)?;
    let revision = match committed {
        PrivateRevisionCommitResponse::Advanced { revision } => revision,
        PrivateRevisionCommitResponse::Diverged { .. } => {
            return Err(RuntimeError::new(
                CliErrorCode::LocalState,
                "merge target advanced while resolving the fork collision; retry the resolution",
            ));
        }
    };
    let updated_target = PrivateSkill {
        revision_id: revision.revision_id.clone(),
        generation: revision.generation,
        manifest: revision.manifest.clone(),
        description: revision.description,
        ..target.clone()
    };
    finish_merge_resolution(
        &context,
        &fork,
        &upstream,
        &updated_target,
        &revision.manifest,
        snapshot.bytes(),
    )
    .await?;
    Ok(ForkResolveOutcome {
        state: "merged",
        upstream_locator: upstream.locator,
        locator: Some(target.locator),
    })
}

async fn remove_local_fork(
    context: &InstalledContext,
    fork: &LocalForkRecord,
) -> Result<(), RuntimeError> {
    let owned = context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|record| record.resource_id == fork.resource_id)
        .ok_or_else(|| {
            RuntimeError::new(CliErrorCode::LocalState, "local fork lost owned state")
        })?;
    journaled_remove_managed_skill(
        &context.paths,
        &context.db,
        &context.roots,
        &ManagedSkillRecord {
            resource_id: owned.resource_id,
            locator: owned.locator,
            owner: owned.owner,
            skill_name: owned.skill_name,
            harness_name: owned.harness_name,
            materialized_revision_id: owned.materialized_revision_id,
        },
        ManagedDesiredKind::Owned,
    )
    .await
    .map_err(local_error)
}

async fn finish_merge_resolution(
    context: &InstalledContext,
    fork: &LocalForkRecord,
    upstream: &denju_wire::SubscriptionTarget,
    target: &PrivateSkill,
    manifest: &PublicSkillManifest,
    snapshot: &[u8],
) -> Result<(), RuntimeError> {
    install_owned_revision(
        context,
        target,
        target.generation,
        &target.revision_id,
        manifest,
        snapshot,
    )
    .await?;
    crate::public::mutate_subscription(
        &context.client,
        SubscriptionMutationKind::Unsubscribe,
        &upstream.resource_id,
        upstream.generation,
        None,
        false,
    )
    .await?;
    remove_local_fork(context, fork).await?;
    Ok(())
}

fn normalize_skill_name(
    entries: &mut [denju_core::OwnedSkillEntry],
    target_name: &str,
) -> Result<(), RuntimeError> {
    let skill_md = entries
        .iter_mut()
        .find_map(|entry| match entry {
            denju_core::OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                Some(bytes)
            }
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeError::new(CliErrorCode::LocalState, "merged fork is missing SKILL.md")
        })?;
    let current_name = skill_document_declared_name(skill_md).map_err(local_error)?;
    if current_name != target_name {
        *skill_md = rewrite_skill_document_name(&current_name, skill_md, target_name)
            .map_err(local_error)?;
    }
    Ok(())
}
