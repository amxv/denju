use std::{collections::BTreeSet, str::FromStr};

use denju_client::{ClientError, RegistryClient};
use denju_core::{OperationId, ResourceId, RevisionId};
use denju_local::{
    CredentialBackend, CredentialManager, DesiredSkillMaterialization, InstallCredential,
    InstallationRecord, LocalDatabase, LocalPaths, ResolvedHarnessRoots, SubscriptionRecord,
    materialize_skill_snapshot, prepare_harness_roots, reconcile_harness_projections,
    recover_materializations, remove_canonical_skill, remove_subscription_projection,
    resolve_harness_roots,
};
use denju_wire::{
    ApiErrorCode, CliErrorCode, PublicSkill, PublicSkillDetail, PublicSkillSearchResponse,
    SubscribedSkill, SubscriptionMutationKind, SubscriptionMutationRequest,
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

pub async fn subscribe(locator: &str) -> Result<SubscribeOutcome, RuntimeError> {
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
    let context = installed_context(true).await?;
    sync_with_context(context).await
}

async fn sync_with_context(context: InstalledContext) -> Result<SyncOutcome, RuntimeError> {
    recover_materializations(&context.paths, &context.db)
        .await
        .map_err(local_error)?;
    let catalog = context.client.subscriptions().await.map_err(client_error)?;
    let remote_ids = catalog
        .skills
        .iter()
        .map(|skill| skill.skill.resource_id.as_str())
        .collect::<BTreeSet<_>>();
    let existing = context.db.subscriptions().await.map_err(local_error)?;
    let mut removed = 0;
    for record in &existing {
        if remote_ids.contains(record.resource_id.as_str()) {
            continue;
        }
        remove_subscription_projection(&context.paths, &context.roots, record)
            .map_err(local_error)?;
        remove_canonical_skill(&context.paths, &record.owner, &record.skill_name)
            .map_err(local_error)?;
        context
            .db
            .remove_subscription(record.resource_id.clone())
            .await
            .map_err(local_error)?;
        removed += 1;
    }

    let mut materialized = 0;
    for remote in &catalog.skills {
        let existing = context
            .db
            .subscription(remote.skill.resource_id.clone())
            .await
            .map_err(local_error)?;
        upsert_desired(&context.db, remote).await?;
        let already_current = existing.as_ref().is_some_and(|record| {
            record.materialized_revision_id.as_deref() == Some(remote.skill.revision_id.as_str())
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
        if remote.snapshot.size_bytes > context.max_transfer_bytes {
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
        desired: catalog.skills.len(),
        materialized,
        removed,
        projections,
    })
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
) -> Result<(), RuntimeError> {
    let operation_id = OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let request_hash =
        subscription_request_hash(kind, &operation_id.to_string(), resource_id, generation)
            .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let request = SubscriptionMutationRequest {
        operation_id: operation_id.to_string(),
        resource_id: resource_id.to_owned(),
        expected_generation: generation,
        request_hash: request_hash.to_string(),
    };
    match kind {
        SubscriptionMutationKind::Subscribe => client.subscribe(&request).await,
        SubscriptionMutationKind::Unsubscribe => client.unsubscribe(&request).await,
    }
    .map_err(client_error)?;
    Ok(())
}

struct InstalledContext {
    paths: LocalPaths,
    db: LocalDatabase,
    roots: ResolvedHarnessRoots,
    client: RegistryClient,
    max_transfer_bytes: u64,
}

async fn installed_context(authenticated: bool) -> Result<InstalledContext, RuntimeError> {
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
        let credential = load_credential(&paths, &installation)?;
        RegistryClient::authenticated(origin, credential.bearer_token()).map_err(client_error)?
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
    let recorded = db.harness_config().await.map_err(local_error)?;
    let roots = resolve_harness_roots(&paths, recorded.as_ref()).map_err(local_error)?;
    prepare_harness_roots(&roots).map_err(local_error)?;
    Ok(InstalledContext {
        paths,
        db,
        roots,
        client,
        max_transfer_bytes: capabilities.limits.max_transfer_bytes,
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

fn client_error(error: ClientError) -> RuntimeError {
    match &error {
        ClientError::Registry(api) if api.code == ApiErrorCode::NotFound => {
            RuntimeError::new(CliErrorCode::NotFound, api.message.clone())
        }
        ClientError::ContentMismatch(_) => {
            RuntimeError::new(CliErrorCode::ContentVerification, error.to_string())
                .recovery("denju sync")
        }
        _ => RuntimeError::new(CliErrorCode::RegistryUnavailable, error.to_string())
            .recovery("denju doctor"),
    }
}

fn local_error(error: impl std::fmt::Display) -> RuntimeError {
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
