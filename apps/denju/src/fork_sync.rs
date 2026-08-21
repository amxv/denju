use std::str::FromStr;

use denju_core::{
    ResourceLocator, SkillMergeResult, build_deterministic_skill_snapshot, merge_skill_entries,
    validate_skill_snapshot,
};
use denju_wire::{
    CliErrorCode, ForkSyncIntent, PrivateRevisionCommitRequest, PrivateRevisionCommitResponse,
    PrivateRevisionRequest, PublicSkillManifest, private_revision_request_hash,
};

use crate::{
    fork_ops::{
        ForkSyncOutcome, fetch_revision_entries, find_owned, install_owned_revision,
        internal_error, new_operation, require_identity, upload_entries,
    },
    public::{client_error, installed_context, local_error},
    setup::RuntimeError,
};

pub(crate) async fn sync(locator: &str) -> Result<ForkSyncOutcome, RuntimeError> {
    let context = installed_context(true).await?;
    require_identity(&context).await?;
    let (_, local_blockers) =
        crate::workspace::capture_local_edits(&context.paths, &context.db, false).await?;
    if let Some(blocker) = local_blockers.into_iter().next() {
        return Err(blocker);
    }
    let (_, upload_blockers) = crate::workspace::drain_queued_revisions(&context).await?;
    if let Some(blocker) = upload_blockers.into_iter().next() {
        return Err(blocker);
    }

    let remote = find_owned(&context, locator).await?;
    let provenance = remote.fork.clone().ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::InvalidArguments,
            format!("{} is not a fork", remote.locator),
        )
    })?;
    if !remote.conflicts.is_empty() {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            format!("{} has an unresolved workspace conflict", remote.locator),
        )
        .recovery("denju status"));
    }
    let upstream_history = context
        .client
        .skill_history(&provenance.upstream_locator)
        .await
        .map_err(client_error)?;
    let upstream_revision = upstream_history.workspace_revision_id;
    if upstream_revision == provenance.sync_base_revision_id {
        return Ok(ForkSyncOutcome {
            state: "current",
            locator: remote.locator,
            revision_id: remote.revision_id,
            upstream_locator: provenance.upstream_locator,
            upstream_revision_id: upstream_revision,
        });
    }

    let upstream_locator =
        ResourceLocator::from_str(&provenance.upstream_locator).map_err(local_error)?;
    let base = fetch_revision_entries(
        &context,
        &provenance.upstream_locator,
        upstream_locator.name(),
        &provenance.sync_base_revision_id,
    )
    .await?;
    let upstream = fetch_revision_entries(
        &context,
        &provenance.upstream_locator,
        upstream_locator.name(),
        &upstream_revision,
    )
    .await?;
    let fork_bytes = context
        .client
        .download_snapshot(&remote.snapshot)
        .await
        .map_err(client_error)?;
    let fork_manifest = remote
        .manifest
        .to_core()
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error))?;
    let fork_entries =
        validate_skill_snapshot(&remote.name, &fork_manifest, &fork_bytes).map_err(local_error)?;
    let merged = match merge_skill_entries(&base, &fork_entries, &upstream) {
        SkillMergeResult::Clean { entries } => entries,
        SkillMergeResult::Conflicted { conflicts } => {
            let paths = conflicts
                .into_iter()
                .map(|conflict| conflict.path)
                .collect::<Vec<_>>();
            return Err(RuntimeError::new(
                CliErrorCode::LocalState,
                format!(
                    "{} conflicts with upstream in {}; edit the fork and retry `denju fork sync {}`",
                    remote.locator,
                    paths.join(", "),
                    remote.locator
                ),
            ));
        }
    };
    let snapshot = build_deterministic_skill_snapshot(&remote.name, &merged)
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error.to_string()))?;
    let operation = new_operation()?;
    let manifest = PublicSkillManifest::from_core(snapshot.manifest());
    let mut parents = vec![remote.revision_id.clone(), upstream_revision.clone()];
    parents.sort();
    let intent = ForkSyncIntent {
        expected_sync_base_revision_id: provenance.sync_base_revision_id.clone(),
        upstream_revision_id: upstream_revision.clone(),
    };
    let operation_id = operation.to_string();
    let request_hash =
        private_revision_request_hash(&denju_wire::PrivateRevisionRequestHashInput {
            operation_id: &operation_id,
            resource_id: &remote.resource_id,
            expected_generation: remote.generation,
            expected_head_revision_id: &remote.revision_id,
            parent_revision_ids: &parents,
            manifest: &manifest,
            revision_author_principal_id: None,
            fork_sync: Some(&intent),
            historical_skill_name: None,
        })
        .map_err(internal_error)?;
    let request = PrivateRevisionRequest {
        operation_id: operation_id.clone(),
        resource_id: remote.resource_id.clone(),
        expected_generation: remote.generation,
        expected_head_revision_id: remote.revision_id.clone(),
        parent_revision_ids: parents,
        manifest: manifest.clone(),
        revision_author_principal_id: None,
        fork_sync: Some(intent),
        historical_skill_name: None,
        request_hash: request_hash.to_string(),
    };
    let prepared = context
        .client
        .prepare_private_revision(&request)
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
                "fork head diverged while applying upstream sync; retry after `denju sync`",
            ));
        }
    };
    install_owned_revision(
        &context,
        &remote,
        revision.generation,
        &revision.revision_id,
        &manifest,
        snapshot.bytes(),
    )
    .await?;
    Ok(ForkSyncOutcome {
        state: "synced",
        locator: remote.locator,
        revision_id: revision.revision_id,
        upstream_locator: provenance.upstream_locator,
        upstream_revision_id: upstream_revision,
    })
}
