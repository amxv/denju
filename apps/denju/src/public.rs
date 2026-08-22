use std::{collections::BTreeSet, fs, str::FromStr};

use denju_client::RegistryClient;
use denju_core::{OperationId, ResourceId, RevisionId};
use denju_local::{
    DesiredSkillMaterialization, LocalDatabase, LocalPaths, ManagedDesiredKind, ManagedSkillRecord,
    OwnedSkillRecord, RegistryRenameState, SubscriptionRecord, WorkspaceStatus,
    apply_registry_rename, journaled_remove_managed_skill, materialize_skill_snapshot,
    prepare_harness_roots, reconcile_canonical_links, reconcile_harness_projections,
    recover_local_lifecycle, recover_materializations, resolve_harness_roots,
};
use denju_wire::{
    CliErrorCode, PrivateSkill, PublicSkillDetail, PublicSkillSearchResponse, SubscribedSkill,
    SubscriptionContent, SubscriptionMutationKind, SubscriptionMutationRequest, SyncKnownResource,
    SyncReconcileRequest, subscription_request_hash,
};
use serde::Serialize;
use uuid::Uuid;

use crate::setup::RuntimeError;

#[derive(Debug, Clone, Serialize)]
pub struct SubscribeOutcome {
    pub state: &'static str,
    pub locator: String,
    pub description: String,
    pub revision_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_version: Option<u64>,
    pub live_private: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<denju_wire::SkillDeprecation>,
    pub harness_name: String,
    pub sync: SyncOutcome,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnsubscribeOutcome {
    pub state: &'static str,
    pub locator: String,
    pub sync: SyncOutcome,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncOutcome {
    pub desired: usize,
    pub materialized: usize,
    pub removed: usize,
    pub projections: Vec<ProjectionOutcome>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectionOutcome {
    pub locator: String,
    pub harness_name: String,
}

pub async fn search(query: &str) -> Result<PublicSkillSearchResponse, RuntimeError> {
    let context = catalog_context().await?;
    context
        .client
        .search_public_skills(query, 20, None)
        .await
        .map_err(client_error)
}

pub async fn show(locator: &str) -> Result<PublicSkillDetail, RuntimeError> {
    let context = catalog_context().await?;
    context
        .client
        .show_public_skill(locator)
        .await
        .map_err(client_error)
}

async fn catalog_context() -> Result<InstalledContext, RuntimeError> {
    let context = installed_context(false).await?;
    let has_session = context
        .db
        .identity()
        .await
        .map_err(local_error)?
        .is_some_and(|identity| identity.session_backend.is_some());
    if has_session {
        installed_context(true).await
    } else {
        Ok(context)
    }
}

pub async fn subscribe(
    locator: &str,
    release_version: Option<u64>,
    retain_on_delete: bool,
) -> Result<SubscribeOutcome, RuntimeError> {
    let context = installed_context(true).await?;
    let target = context
        .client
        .subscription_target(locator)
        .await
        .map_err(client_error)?;
    mutate_subscription(
        &context.client,
        SubscriptionMutationKind::Subscribe,
        &target.resource_id,
        target.generation,
        release_version,
        retain_on_delete,
    )
    .await?;
    let db = context.db.clone();
    let (sync, blockers) = sync_with_context(context).await?;
    if let Some(blocker) = blockers.into_iter().next() {
        return Err(blocker);
    }
    let harness_name = sync
        .projections
        .iter()
        .find(|projection| projection.locator == target.locator)
        .map(|projection| projection.harness_name.clone())
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "subscription synchronized without a harness projection",
            )
            .recovery("denju sync")
        })?;
    let local = db
        .subscription(target.resource_id.clone())
        .await
        .map_err(local_error)?
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "subscription synchronized without durable local desired state",
            )
        })?;
    Ok(SubscribeOutcome {
        state: "subscribed",
        locator: target.locator,
        description: target.description,
        revision_id: local.desired_revision_id,
        release_version: (!local.live_private).then_some(
            u64::try_from(local.release_version).map_err(|_| {
                RuntimeError::new(CliErrorCode::LocalState, "release version is invalid")
            })?,
        ),
        live_private: local.live_private,
        deprecation: target.deprecation,
        harness_name,
        sync,
    })
}

pub async fn unsubscribe(locator: &str) -> Result<UnsubscribeOutcome, RuntimeError> {
    let context = installed_context(true).await?;
    let local = context
        .db
        .subscriptions()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|record| record.locator == locator)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::NotFound,
                format!("not subscribed to {locator}"),
            )
        })?;
    let generation = u64::try_from(local.resource_generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "stored resource generation is invalid",
        )
    })?;
    mutate_subscription(
        &context.client,
        SubscriptionMutationKind::Unsubscribe,
        &local.resource_id,
        generation,
        None,
        false,
    )
    .await?;
    let (sync, blockers) = sync_with_context(context).await?;
    if let Some(blocker) = blockers.into_iter().next() {
        return Err(blocker);
    }
    Ok(UnsubscribeOutcome {
        state: "unsubscribed",
        locator: locator.to_owned(),
        sync,
    })
}

pub(crate) async fn sync_once() -> Result<SyncOutcome, RuntimeError> {
    // Capture owned edits before the first network request. This is what makes explicit
    // `denju sync` useful while offline even when the background service is unavailable:
    // a valid save becomes a durable queued local revision before registry connectivity is
    // allowed to fail the command.
    let mut blockers = Vec::new();
    if let Ok(paths) = LocalPaths::discover()
        && paths.state_db.is_file()
    {
        let db = LocalDatabase::open(&paths.state_db)
            .await
            .map_err(local_error)?;
        let recorded = db.harness_config().await.map_err(local_error)?;
        let roots = resolve_harness_roots(&paths, recorded.as_ref()).map_err(local_error)?;
        prepare_harness_roots(&roots).map_err(local_error)?;
        recover_local_lifecycle(&paths, &db, &roots)
            .await
            .map_err(local_error)?;
        let (_forked, fork_blockers) =
            crate::forks::protect_subscription_edits(&paths, &db, &roots).await?;
        blockers.extend(fork_blockers);
        let (_workspace_pass, local_blockers) =
            crate::workspace::capture_local_edits(&paths, &db, false).await?;
        blockers.extend(local_blockers);
    }
    let context = installed_context(true).await?;
    // An upgraded Phase-5 identity may only learn its user author-principal from the registry
    // above, so run the idempotent capture once more after authenticated context hydration.
    let (_workspace_pass, hydrated_blockers) =
        crate::workspace::capture_local_edits(&context.paths, &context.db, false).await?;
    blockers.extend(hydrated_blockers);
    crate::fork_ops::promote_local_forks(&context).await?;
    let (_uploaded, upload_blockers) = crate::workspace::drain_queued_revisions(&context).await?;
    blockers.extend(upload_blockers);
    let (outcome, remote_blockers) = sync_with_context(context).await?;
    blockers.extend(remote_blockers);
    if let Some(blocker) = blockers.into_iter().next() {
        Err(blocker)
    } else {
        Ok(outcome)
    }
}

pub(crate) async fn wait_for_remote_hint() -> Result<(), RuntimeError> {
    let context = installed_context(true).await?;
    context
        .client
        .wait_for_sync_hint()
        .await
        .map_err(client_error)?;
    Ok(())
}

pub(crate) async fn clear_local_managed_state() -> Result<usize, RuntimeError> {
    let paths = LocalPaths::discover().map_err(local_error)?;
    let db = LocalDatabase::open(&paths.state_db)
        .await
        .map_err(local_error)?;
    let recorded = db.harness_config().await.map_err(local_error)?;
    let roots = resolve_harness_roots(&paths, recorded.as_ref()).map_err(local_error)?;
    recover_local_lifecycle(&paths, &db, &roots)
        .await
        .map_err(local_error)?;
    let managed = db.managed_skills().await.map_err(local_error)?;
    let owned = db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .map(|record| record.resource_id)
        .collect::<BTreeSet<_>>();
    for record in &managed {
        let kind = if owned.contains(&record.resource_id) {
            ManagedDesiredKind::Owned
        } else {
            ManagedDesiredKind::Subscription
        };
        journaled_remove_managed_skill(&paths, &db, &roots, record, kind)
            .await
            .map_err(local_error)?;
    }
    for directory in [&paths.generations, &paths.derived, &paths.objects] {
        if directory.exists() {
            fs::remove_dir_all(directory).map_err(local_error)?;
        }
        fs::create_dir_all(directory).map_err(local_error)?;
    }
    Ok(managed.len())
}

async fn sync_with_context(
    context: InstalledContext,
) -> Result<(SyncOutcome, Vec<RuntimeError>), RuntimeError> {
    let mut blockers = Vec::new();
    recover_materializations(&context.paths, &context.db)
        .await
        .map_err(local_error)?;
    recover_local_lifecycle(&context.paths, &context.db, &context.roots)
        .await
        .map_err(local_error)?;
    crate::pack_sync::recover_incomplete_apply(&context).await?;
    let existing = context.db.subscriptions().await.map_err(local_error)?;
    let mut known = Vec::with_capacity(existing.len());
    for record in &existing {
        known.push(SyncKnownResource {
            resource_id: record.resource_id.clone(),
            generation: u64::try_from(record.resource_generation).map_err(|_| {
                RuntimeError::new(
                    CliErrorCode::LocalState,
                    "stored subscription generation is invalid",
                )
                .recovery("denju doctor")
            })?,
            // Reconcile what is actually visible on this device, not merely the remote
            // revision we last recorded as desired. If a download/materialization failed
            // after desired-state persistence, advertising that desired revision here would
            // make the registry omit it as unchanged and strand the local retry forever.
            revision_id: record.materialized_revision_id.clone().unwrap_or_default(),
        });
    }
    let reconcile = context
        .client
        .reconcile_subscriptions(&SyncReconcileRequest { known })
        .await
        .map_err(client_error)?;
    let mut suppressed_subscription_ids = context
        .db
        .local_forks()
        .await
        .map_err(local_error)?
        .into_iter()
        .map(|fork| fork.upstream_resource_id)
        .collect::<BTreeSet<_>>();
    suppressed_subscription_ids.extend(
        context
            .db
            .subscription_edit_locks()
            .await
            .map_err(local_error)?,
    );
    let removed_ids = reconcile
        .removed_resource_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut removed: usize = 0;
    for record in &existing {
        if !removed_ids.contains(record.resource_id.as_str()) {
            continue;
        }
        journaled_remove_managed_skill(
            &context.paths,
            &context.db,
            &context.roots,
            &ManagedSkillRecord {
                resource_id: record.resource_id.clone(),
                locator: record.locator.clone(),
                owner: record.owner.clone(),
                skill_name: record.skill_name.clone(),
                harness_name: record.harness_name.clone(),
                materialized_revision_id: record.materialized_revision_id.clone(),
            },
            ManagedDesiredKind::Subscription,
        )
        .await
        .map_err(local_error)?;
        removed += 1;
    }

    let mut materialized = 0;
    for remote in &reconcile.skills {
        if suppressed_subscription_ids.contains(&remote.resource_id) {
            continue;
        }
        let mut existing = context
            .db
            .subscription(remote.resource_id.clone())
            .await
            .map_err(local_error)?;
        if let Some(local) = existing.as_ref()
            && (local.owner != remote.owner || local.skill_name != remote.name)
        {
            journaled_remove_managed_skill(
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
                ManagedDesiredKind::Subscription,
            )
            .await
            .map_err(local_error)?;
            existing = None;
        }
        upsert_desired(&context.db, remote).await?;
        let already_current = existing.as_ref().is_some_and(|record| {
            record.owner == remote.owner
                && record.skill_name == remote.name
                && record.materialized_revision_id.as_deref() == Some(remote.revision_id.as_str())
                && context
                    .paths
                    .skills
                    .join(&remote.owner)
                    .join(&remote.name)
                    .exists()
        });
        if already_current {
            continue;
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
        materialize_skill_snapshot(&context.paths, &context.db, &desired, &bytes)
            .await
            .map_err(|error| {
                RuntimeError::new(CliErrorCode::ContentVerification, error.to_string())
                    .recovery("denju sync")
            })?;
        materialized += 1;
    }

    let mut owned_desired = 0;
    if context
        .db
        .identity()
        .await
        .map_err(local_error)?
        .is_some_and(|identity| identity.session_backend.is_some())
    {
        let owned = context
            .client
            .private_skills()
            .await
            .map_err(client_error)?;
        owned_desired = owned.skills.len();
        let remote_ids = owned
            .skills
            .iter()
            .map(|skill| skill.resource_id.as_str())
            .collect::<BTreeSet<_>>();
        let local_fork_ids = context
            .db
            .local_forks()
            .await
            .map_err(local_error)?
            .into_iter()
            .map(|fork| fork.resource_id)
            .collect::<BTreeSet<_>>();
        let existing_owned = context.db.owned_skills().await.map_err(local_error)?;
        for record in &existing_owned {
            if remote_ids.contains(record.resource_id.as_str())
                || local_fork_ids.contains(record.resource_id.as_str())
            {
                continue;
            }
            let managed = context
                .db
                .managed_skills()
                .await
                .map_err(local_error)?
                .into_iter()
                .find(|managed| managed.resource_id == record.resource_id)
                .ok_or_else(|| {
                    RuntimeError::new(
                        CliErrorCode::LocalState,
                        format!("owned resource {} lost managed state", record.resource_id),
                    )
                })?;
            journaled_remove_managed_skill(
                &context.paths,
                &context.db,
                &context.roots,
                &managed,
                ManagedDesiredKind::Owned,
            )
            .await
            .map_err(local_error)?;
            removed += 1;
        }
        for remote in &owned.skills {
            match sync_owned_skill(&context, remote).await {
                Ok(count) => materialized += count,
                Err(error) => {
                    let persisted_conflict = error.code == CliErrorCode::LocalState
                        && context
                            .db
                            .workspace_content_conflict(remote.resource_id.clone())
                            .await
                            .map_err(local_error)?
                            .is_some();
                    if persisted_conflict {
                        blockers.push(error);
                    } else {
                        return Err(error);
                    }
                }
            }
        }
    }

    let pack_state = crate::pack_sync::refresh_catalog(&context).await?;
    let pack_apply = crate::pack_sync::apply_pack_only_state(&context, &pack_state).await?;
    materialized = materialized.saturating_add(pack_apply.materialized);
    removed = removed.saturating_add(pack_apply.removed);
    let pack_desired = context
        .db
        .pack_materialized_skills()
        .await
        .map_err(local_error)?
        .len();
    for conflict in context
        .db
        .pack_source_conflicts()
        .await
        .map_err(local_error)?
    {
        blockers.push(
            RuntimeError::new(
                CliErrorCode::LocalState,
                format!(
                    "{} (pack sources: {}; revisions: {})",
                    conflict.message,
                    conflict.source_pack_ids.join(", "),
                    conflict.revision_ids.join(", ")
                ),
            )
            .recovery("denju status"),
        );
    }
    let mut unavailable = BTreeSet::new();
    for source in context.db.pack_skill_sources().await.map_err(local_error)? {
        if let Some(reason) = source.unavailable_reason
            && unavailable.insert((source.pack_resource_id.clone(), source.resource_id.clone()))
        {
            blockers.push(
                RuntimeError::new(
                    CliErrorCode::NotFound,
                    format!(
                        "pack {} cannot currently satisfy {}: {reason}",
                        source.pack_resource_id, source.locator
                    ),
                )
                .recovery("denju show <pack-locator>"),
            );
        }
    }

    reconcile_canonical_links(&context.paths, &context.db)
        .await
        .map_err(local_error)?;
    let projections = reconcile_harness_projections(&context.paths, &context.db, &context.roots)
        .await
        .map_err(local_error)?
        .into_iter()
        .map(|(locator, harness_name)| ProjectionOutcome {
            locator,
            harness_name,
        })
        .collect();
    Ok((
        SyncOutcome {
            desired: existing.len().saturating_sub(removed)
                + reconcile
                    .skills
                    .iter()
                    .filter(|remote| {
                        !existing
                            .iter()
                            .any(|local| local.resource_id == remote.resource_id)
                    })
                    .count()
                + owned_desired
                + pack_desired,
            materialized,
            removed,
            projections,
        },
        blockers,
    ))
}

async fn sync_owned_skill(
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
                generation: resource_generation,
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
            && context
                .paths
                .skills
                .join(&remote.owner)
                .join(&remote.name)
                .exists()
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
                resource_generation,
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
                resource_generation,
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
            resource_generation,
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
            resource_generation,
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

async fn upsert_desired(db: &LocalDatabase, remote: &SubscribedSkill) -> Result<(), RuntimeError> {
    let resource_generation = i64::try_from(remote.generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "resource generation exceeds local storage",
        )
    })?;
    let (release_version, live_private) = match remote.content {
        SubscriptionContent::Release { version, .. } => (
            i64::try_from(version).map_err(|_| {
                RuntimeError::new(
                    CliErrorCode::LocalState,
                    "release version exceeds local storage",
                )
            })?,
            false,
        ),
        SubscriptionContent::PrivateWorkspace => (0, true),
    };
    let desired_root_tree_id = remote.manifest.root_tree_id.clone();
    db.upsert_subscription_desired(
        SubscriptionRecord {
            resource_id: remote.resource_id.clone(),
            locator: remote.locator.clone(),
            owner: remote.owner.clone(),
            skill_name: remote.name.clone(),
            resource_generation,
            release_version,
            desired_revision_id: remote.revision_id.clone(),
            harness_name: None,
            materialized_revision_id: None,
            retain_on_delete: remote.retain_on_delete,
            retained_after_delete: remote.retained_after_delete,
            live_private,
            desired_root_tree_id,
        },
        now_unix_ms(),
    )
    .await
    .map_err(local_error)
}

pub(crate) async fn mutate_subscription(
    client: &RegistryClient,
    kind: SubscriptionMutationKind,
    resource_id: &str,
    generation: u64,
    release_version: Option<u64>,
    retain_on_delete: bool,
) -> Result<(), RuntimeError> {
    let operation_id = OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let request_hash = subscription_request_hash(
        kind,
        &operation_id.to_string(),
        resource_id,
        generation,
        release_version,
        retain_on_delete,
    )
    .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let request = SubscriptionMutationRequest {
        operation_id: operation_id.to_string(),
        resource_id: resource_id.to_owned(),
        expected_generation: generation,
        release_version,
        retain_on_delete,
        request_hash: request_hash.to_string(),
    };
    match kind {
        SubscriptionMutationKind::Subscribe => client.subscribe(&request).await,
        SubscriptionMutationKind::Unsubscribe => client.unsubscribe(&request).await,
    }
    .map_err(client_error)?;
    Ok(())
}

pub(crate) use crate::context::{
    InstalledContext, client_error, installed_context, local_error, now_unix_ms,
};
