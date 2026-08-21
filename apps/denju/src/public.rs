use std::{collections::BTreeSet, fs, str::FromStr};

use denju_client::{ClientError, RegistryClient};
use denju_core::{OperationId, ResourceId, RevisionId};
use denju_local::{
    CredentialBackend, CredentialManager, DesiredSkillMaterialization, IdentityRecord,
    InstallCredential, InstallationRecord, LocalDatabase, LocalPaths, ManagedDesiredKind,
    ManagedSkillRecord, OwnedSkillRecord, RegistryRenameState, ResolvedHarnessRoots,
    SessionCredential, SubscriptionRecord, WorkspaceStatus, apply_registry_rename,
    journaled_remove_managed_skill, materialize_skill_snapshot, prepare_harness_roots,
    reconcile_canonical_links, reconcile_harness_projections, recover_local_lifecycle,
    recover_materializations, resolve_harness_roots,
};
use denju_wire::{
    ApiErrorCode, CliErrorCode, PrivateSkill, PublicSkill, PublicSkillDetail,
    PublicSkillSearchResponse, RegistryLimits, SubscribedSkill, SubscriptionMutationKind,
    SubscriptionMutationRequest, SyncKnownResource, SyncReconcileRequest,
    subscription_request_hash,
};
use serde::Serialize;
use url::Url;
use uuid::Uuid;

use crate::setup::RuntimeError;

#[derive(Debug, Clone, Serialize)]
pub struct SubscribeOutcome {
    pub state: &'static str,
    pub skill: PublicSkill,
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
    let context = installed_context(false).await?;
    context
        .client
        .search_public_skills(query, 20, None)
        .await
        .map_err(client_error)
}

pub async fn show(locator: &str) -> Result<PublicSkillDetail, RuntimeError> {
    let context = installed_context(false).await?;
    context
        .client
        .show_public_skill(locator)
        .await
        .map_err(client_error)
}

pub async fn subscribe(
    locator: &str,
    release_version: Option<u64>,
    retain_on_delete: bool,
) -> Result<SubscribeOutcome, RuntimeError> {
    let context = installed_context(true).await?;
    let detail = context
        .client
        .show_public_skill(locator)
        .await
        .map_err(client_error)?;
    mutate_subscription(
        &context.client,
        SubscriptionMutationKind::Subscribe,
        &detail.skill.resource_id,
        detail.skill.generation,
        release_version,
        retain_on_delete,
    )
    .await?;
    let sync = sync_with_context(context).await?;
    let harness_name = sync
        .projections
        .iter()
        .find(|projection| projection.locator == detail.skill.locator)
        .map(|projection| projection.harness_name.clone())
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "subscription synchronized without a harness projection",
            )
            .recovery("denju sync")
        })?;
    Ok(SubscribeOutcome {
        state: "subscribed",
        skill: detail.skill,
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
    let sync = sync_with_context(context).await?;
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
    let (_uploaded, upload_blockers) = crate::workspace::drain_queued_revisions(&context).await?;
    blockers.extend(upload_blockers);
    let outcome = sync_with_context(context).await?;
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

async fn sync_with_context(context: InstalledContext) -> Result<SyncOutcome, RuntimeError> {
    recover_materializations(&context.paths, &context.db)
        .await
        .map_err(local_error)?;
    recover_local_lifecycle(&context.paths, &context.db, &context.roots)
        .await
        .map_err(local_error)?;
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
            revision_id: record.desired_revision_id.clone(),
        });
    }
    let reconcile = context
        .client
        .reconcile_subscriptions(&SyncReconcileRequest { known })
        .await
        .map_err(client_error)?;
    let removed_ids = reconcile
        .removed_resource_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut removed = 0;
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
        let existing = context
            .db
            .subscription(remote.skill.resource_id.clone())
            .await
            .map_err(local_error)?;
        upsert_desired(&context.db, remote).await?;
        let already_current = existing.as_ref().is_some_and(|record| {
            record.owner == remote.skill.owner
                && record.skill_name == remote.skill.name
                && record.materialized_revision_id.as_deref()
                    == Some(remote.skill.revision_id.as_str())
                && context
                    .paths
                    .skills
                    .join(&remote.skill.owner)
                    .join(&remote.skill.name)
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
                    remote.skill.locator
                ),
            ));
        }
        let bytes = context
            .client
            .download_snapshot(&remote.snapshot)
            .await
            .map_err(client_error)?;
        let desired = DesiredSkillMaterialization {
            resource_id: ResourceId::from_str(&remote.skill.resource_id).map_err(local_error)?,
            owner: remote.skill.owner.clone(),
            skill_name: remote.skill.name.clone(),
            revision_id: RevisionId::from_str(&remote.skill.revision_id).map_err(local_error)?,
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
        let existing_owned = context.db.owned_skills().await.map_err(local_error)?;
        for record in &existing_owned {
            if remote_ids.contains(record.resource_id.as_str()) {
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
            materialized += sync_owned_skill(&context, remote).await?;
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
    Ok(SyncOutcome {
        desired: existing.len().saturating_sub(removed)
            + reconcile
                .skills
                .iter()
                .filter(|remote| {
                    !existing
                        .iter()
                        .any(|local| local.resource_id == remote.skill.resource_id)
                })
                .count()
            + owned_desired,
        materialized,
        removed,
        projections,
    })
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
    Ok(1)
}

async fn upsert_desired(db: &LocalDatabase, remote: &SubscribedSkill) -> Result<(), RuntimeError> {
    let resource_generation = i64::try_from(remote.skill.generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "resource generation exceeds local storage",
        )
    })?;
    let release_version = i64::try_from(remote.skill.version).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "release version exceeds local storage",
        )
    })?;
    db.upsert_subscription_desired(
        SubscriptionRecord {
            resource_id: remote.skill.resource_id.clone(),
            locator: remote.skill.locator.clone(),
            owner: remote.skill.owner.clone(),
            skill_name: remote.skill.name.clone(),
            resource_generation,
            release_version,
            desired_revision_id: remote.skill.revision_id.clone(),
            harness_name: None,
            materialized_revision_id: None,
            retain_on_delete: remote.retain_on_delete,
            retained_after_delete: remote.retained_after_delete,
        },
        now_unix_ms(),
    )
    .await
    .map_err(local_error)
}

async fn mutate_subscription(
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

pub(crate) struct InstalledContext {
    pub(crate) paths: LocalPaths,
    pub(crate) db: LocalDatabase,
    pub(crate) roots: ResolvedHarnessRoots,
    pub(crate) client: RegistryClient,
    pub(crate) limits: RegistryLimits,
}

pub(crate) async fn installed_context(
    authenticated: bool,
) -> Result<InstalledContext, RuntimeError> {
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
    let installation = db
        .installation()
        .await
        .map_err(local_error)?
        .ok_or_else(|| {
            RuntimeError::new(CliErrorCode::SetupRequired, "Denju is not set up")
                .recovery("denju setup")
        })?;
    let origin = Url::parse(&installation.registry_origin)
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
    let client = if authenticated {
        let bearer = load_active_bearer(&paths, &db, &installation).await?;
        RegistryClient::authenticated(origin, bearer).map_err(client_error)?
    } else {
        RegistryClient::new(origin).map_err(client_error)?
    };
    client.ready().await.map_err(client_error)?;
    let capabilities = client.capabilities().await.map_err(client_error)?;
    if capabilities.api_version != "v1" || !capabilities.object_store_required {
        return Err(RuntimeError::new(
            CliErrorCode::RegistryUnavailable,
            "registry does not satisfy the Denju v1 capability contract",
        ));
    }
    if authenticated
        && let Some(identity) = db.identity().await.map_err(local_error)?
        && identity.author_principal_id.is_none()
        && identity.session_backend.is_some()
    {
        let remote = client.identity().await.map_err(client_error)?;
        db.save_identity(
            IdentityRecord {
                user_id: remote.user_id,
                namespace_id: remote.namespace_id,
                username: remote.username,
                session_id: identity.session_id,
                session_backend: identity.session_backend,
                author_principal_id: Some(remote.author_principal_id),
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    }
    let recorded = db.harness_config().await.map_err(local_error)?;
    let roots = resolve_harness_roots(&paths, recorded.as_ref()).map_err(local_error)?;
    prepare_harness_roots(&roots).map_err(local_error)?;
    Ok(InstalledContext {
        paths,
        db,
        roots,
        client,
        limits: capabilities.limits,
    })
}

fn load_credential(
    paths: &LocalPaths,
    installation: &InstallationRecord,
) -> Result<InstallCredential, RuntimeError> {
    let backend =
        CredentialBackend::from_str(&installation.credential_backend).map_err(|error| {
            RuntimeError::new(CliErrorCode::CredentialUnavailable, error.to_string())
        })?;
    CredentialManager::load(paths, backend)
        .map_err(|error| RuntimeError::new(CliErrorCode::CredentialUnavailable, error.to_string()))
}

async fn load_active_bearer(
    paths: &LocalPaths,
    db: &LocalDatabase,
    installation: &InstallationRecord,
) -> Result<String, RuntimeError> {
    if let Some(identity) = db.identity().await.map_err(local_error)? {
        let backend_name = identity.session_backend.as_deref().ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::CredentialUnavailable,
                format!("{} is not logged in on this device", identity.username),
            )
            .recovery(format!("denju login {}", identity.username))
        })?;
        let backend = CredentialBackend::from_str(backend_name).map_err(|error| {
            RuntimeError::new(CliErrorCode::CredentialUnavailable, error.to_string())
        })?;
        let session: SessionCredential =
            CredentialManager::load_session(paths, backend).map_err(|error| {
                RuntimeError::new(CliErrorCode::CredentialUnavailable, error.to_string())
                    .recovery("denju login <@username>")
            })?;
        Ok(session.bearer_token())
    } else {
        Ok(load_credential(paths, installation)?.bearer_token())
    }
}

pub(crate) fn client_error(error: ClientError) -> RuntimeError {
    match &error {
        ClientError::Registry(api) if api.code == ApiErrorCode::NotFound => {
            RuntimeError::new(CliErrorCode::NotFound, api.message.clone())
        }
        ClientError::ContentMismatch(_) => {
            RuntimeError::new(CliErrorCode::ContentVerification, error.to_string())
                .recovery("denju sync")
        }
        ClientError::Registry(api) if api.code == ApiErrorCode::QuotaExceeded => {
            RuntimeError::new(CliErrorCode::QuotaExceeded, api.message.clone())
        }
        ClientError::Registry(api)
            if matches!(
                api.code,
                ApiErrorCode::InvalidRequest
                    | ApiErrorCode::InvalidRequestHash
                    | ApiErrorCode::OperationConflict
                    | ApiErrorCode::GenerationConflict
            ) =>
        {
            RuntimeError::new(CliErrorCode::InvalidArguments, api.message.clone())
        }
        ClientError::Registry(api) if api.code == ApiErrorCode::Unauthorized => {
            RuntimeError::new(CliErrorCode::CredentialUnavailable, api.message.clone())
        }
        _ => RuntimeError::new(CliErrorCode::RegistryUnavailable, error.to_string())
            .recovery("denju doctor"),
    }
}

pub(crate) fn local_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::LocalState, error.to_string()).recovery("denju doctor")
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}
