use std::{fs, str::FromStr};

use denju_core::{
    AuthorPrincipalId, OperationId, ResourceId, Revision, RevisionId,
    build_deterministic_skill_snapshot, skill_document_declared_name,
};
use denju_local::{
    DesiredSkillMaterialization, LocalDatabase, LocalForkRecord, LocalPaths, LocalRevisionRecord,
    ManagedDesiredKind, ManagedSkillRecord, OwnedSkillRecord, ResolvedHarnessRoots,
    journaled_remove_managed_skill, materialize_skill_snapshot, read_skill_source,
    reconcile_harness_projections,
};
use denju_wire::{CliErrorCode, PublicSkillManifest};
use uuid::Uuid;

use crate::{context::now_unix_ms, setup::RuntimeError};

pub(crate) async fn protect_subscription_edits(
    paths: &LocalPaths,
    db: &LocalDatabase,
    roots: &ResolvedHarnessRoots,
) -> Result<(usize, Vec<RuntimeError>), RuntimeError> {
    let mut forked = 0;
    let mut blockers = Vec::new();
    for subscription in db.subscriptions().await.map_err(local_error)? {
        let canonical = paths
            .skills
            .join(&subscription.owner)
            .join(&subscription.skill_name);
        if !canonical.exists() {
            continue;
        }
        let working = fs::canonicalize(&canonical).map_err(local_error)?;
        let entries = match read_skill_source(&working) {
            Ok(entries) => entries,
            Err(error) => {
                blockers.push(
                    lock_invalid_edit(db, &subscription.resource_id, error.to_string()).await?,
                );
                continue;
            }
        };
        let skill_md = entries.iter().find_map(|entry| match entry {
            denju_core::OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                Some(bytes.as_slice())
            }
            _ => None,
        });
        let desired_name = match skill_md
            .ok_or_else(|| "edited subscription is missing SKILL.md".to_owned())
            .and_then(|bytes| {
                skill_document_declared_name(bytes).map_err(|error| error.to_string())
            }) {
            Ok(name) => name,
            Err(error) => {
                blockers.push(lock_invalid_edit(db, &subscription.resource_id, error).await?);
                continue;
            }
        };
        let snapshot = match build_deterministic_skill_snapshot(&desired_name, &entries) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                blockers.push(
                    lock_invalid_edit(db, &subscription.resource_id, error.to_string()).await?,
                );
                continue;
            }
        };
        if snapshot.manifest().root_tree().to_string() == subscription.desired_root_tree_id {
            db.clear_subscription_edit_lock(subscription.resource_id)
                .await
                .map_err(local_error)?;
            continue;
        }

        create_local_fork(paths, db, roots, &subscription, &desired_name, &snapshot).await?;
        forked += 1;
    }
    Ok((forked, blockers))
}

async fn create_local_fork(
    paths: &LocalPaths,
    db: &LocalDatabase,
    roots: &ResolvedHarnessRoots,
    subscription: &denju_local::SubscriptionRecord,
    desired_name: &str,
    snapshot: &denju_core::DeterministicSkillSnapshot,
) -> Result<(), RuntimeError> {
    let author = current_local_author(db).await?;
    let upstream = RevisionId::from_str(&subscription.desired_revision_id).map_err(local_error)?;
    let operation = OperationId::from_uuid(Uuid::now_v7()).map_err(local_error)?;
    let revision = Revision::new(
        snapshot.manifest().root_tree(),
        vec![upstream],
        author,
        operation,
    )
    .map_err(local_error)?;
    let resource_id = ResourceId::from_uuid(Uuid::now_v7()).map_err(local_error)?;
    let short = resource_id
        .to_string()
        .chars()
        .filter(|character| *character != '-')
        .take(8)
        .collect::<String>();
    let owner = format!("local-{short}");
    let locator = format!("@{owner}/{desired_name}");
    let now = now_unix_ms();
    db.upsert_owned_skill_desired(
        OwnedSkillRecord {
            resource_id: resource_id.to_string(),
            locator: locator.clone(),
            owner: owner.clone(),
            skill_name: desired_name.to_owned(),
            resource_generation: 1,
            workspace_generation: 1,
            desired_revision_id: revision.id().to_string(),
            harness_name: None,
            materialized_revision_id: None,
        },
        now,
    )
    .await
    .map_err(local_error)?;
    let desired = DesiredSkillMaterialization {
        resource_id,
        owner: owner.clone(),
        skill_name: desired_name.to_owned(),
        revision_id: revision.id(),
        manifest: snapshot.manifest().clone(),
    };
    let generation = materialize_skill_snapshot(paths, db, &desired, snapshot.bytes())
        .await
        .map_err(local_error)?;
    db.ensure_workspace_baseline(
        resource_id.to_string(),
        1,
        revision.id().to_string(),
        snapshot.manifest().root_tree().to_string(),
        generation.display().to_string(),
        now,
    )
    .await
    .map_err(local_error)?;
    db.record_local_fork_revision(
        LocalRevisionRecord {
            operation_id: operation.to_string(),
            resource_id: resource_id.to_string(),
            revision_id: revision.id().to_string(),
            expected_head_revision_id: subscription.desired_revision_id.clone(),
            parent_revision_ids: vec![subscription.desired_revision_id.clone()],
            expected_generation: 1,
            root_tree_id: snapshot.manifest().root_tree().to_string(),
            manifest_json: serde_json::to_string(&PublicSkillManifest::from_core(
                snapshot.manifest(),
            ))
            .map_err(internal_error)?,
            state: "synced".to_owned(),
        },
        now,
    )
    .await
    .map_err(local_error)?;
    db.save_local_fork(
        LocalForkRecord {
            resource_id: resource_id.to_string(),
            upstream_resource_id: subscription.resource_id.clone(),
            upstream_locator: subscription.locator.clone(),
            created_from_revision_id: subscription.desired_revision_id.clone(),
            sync_base_revision_id: subscription.desired_revision_id.clone(),
            desired_name: desired_name.to_owned(),
            state: "local".to_owned(),
        },
        now,
    )
    .await
    .map_err(local_error)?;

    journaled_remove_managed_skill(
        paths,
        db,
        roots,
        &ManagedSkillRecord {
            resource_id: subscription.resource_id.clone(),
            locator: subscription.locator.clone(),
            owner: subscription.owner.clone(),
            skill_name: subscription.skill_name.clone(),
            harness_name: subscription.harness_name.clone(),
            materialized_revision_id: subscription.materialized_revision_id.clone(),
        },
        ManagedDesiredKind::Subscription,
    )
    .await
    .map_err(local_error)?;
    db.clear_subscription_edit_lock(subscription.resource_id.clone())
        .await
        .map_err(local_error)?;
    reconcile_harness_projections(paths, db, roots)
        .await
        .map_err(local_error)?;
    Ok(())
}

async fn current_local_author(db: &LocalDatabase) -> Result<AuthorPrincipalId, RuntimeError> {
    if let Some(identity) = db.identity().await.map_err(local_error)?
        && let Some(author) = identity.author_principal_id
    {
        return AuthorPrincipalId::from_str(&author).map_err(local_error);
    }
    let installation = db
        .installation()
        .await
        .map_err(local_error)?
        .ok_or_else(|| RuntimeError::new(CliErrorCode::SetupRequired, "Denju is not set up"))?;
    AuthorPrincipalId::from_str(&installation.author_principal_id).map_err(local_error)
}

async fn lock_invalid_edit(
    db: &LocalDatabase,
    resource_id: &str,
    detail: String,
) -> Result<RuntimeError, RuntimeError> {
    let message = format!(
        "subscribed skill {resource_id} was edited but is not a valid Agent Skill yet: {detail}; local bytes were preserved"
    );
    db.save_subscription_edit_lock(resource_id.to_owned(), message.clone(), now_unix_ms())
        .await
        .map_err(local_error)?;
    Ok(RuntimeError::new(CliErrorCode::ContentVerification, message).recovery("denju sync"))
}

fn local_error(error: impl std::fmt::Display) -> RuntimeError {
    crate::context::local_error(error)
}

fn internal_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::Internal, error.to_string())
}
