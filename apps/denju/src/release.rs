use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use denju_client::ClientError;
use denju_core::OperationId;
use denju_local::{OwnedSkillRecord, export_skill_snapshot};
use denju_wire::{
    ApiErrorCode, CliErrorCode, PrivateRevisionResponse, PublicSkillManifestEntry,
    PublishSkillRequest, PublishSkillResponse, RestoreSkillRequest, SkillHistoryResponse,
    SkillRevisionDetail, publish_skill_request_hash, restore_skill_request_hash,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    public::{client_error, installed_context, local_error, sync_once},
    setup::RuntimeError,
};

#[derive(Debug, Clone, Serialize)]
pub struct DiffOutcome {
    pub locator: String,
    pub from_revision: String,
    pub to_revision: String,
    pub changes: Vec<DiffEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffEntry {
    pub path: String,
    pub change: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestoreOutcome {
    #[serde(flatten)]
    pub revision: PrivateRevisionResponse,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportOutcome {
    pub locator: String,
    pub revision_id: String,
    pub destination: PathBuf,
}

pub async fn publish(
    locator: &str,
    public: bool,
    message: Option<String>,
    tags: Vec<String>,
) -> Result<PublishSkillResponse, RuntimeError> {
    // Settle local edits first so publish always means "the bytes I currently have", not a
    // stale registry workspace head.
    sync_once().await?;
    let context = installed_context(true).await?;
    let owned = context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|skill| skill.locator == locator)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::NotFound,
                format!("{locator} is not an owned skill on this identity"),
            )
        })?;
    let request = build_publish_request(&owned, public, &message, &tags)?;
    let outcome = match context.client.publish_skill(&request).await {
        Ok(outcome) => outcome,
        Err(ClientError::Registry(api)) if api.code == ApiErrorCode::GenerationConflict => {
            // A team maintainer may have published from another private workspace after this
            // workspace last synced. Reconcile once: clean three-way merges are committed by
            // the existing conflict path, while real content conflicts return a precise blocker.
            sync_once().await?;
            let refreshed = installed_context(true).await?;
            let owned = refreshed
                .db
                .owned_skills()
                .await
                .map_err(local_error)?
                .into_iter()
                .find(|skill| skill.locator == locator)
                .ok_or_else(|| {
                    RuntimeError::new(
                        CliErrorCode::NotFound,
                        format!("{locator} is no longer an owned skill on this identity"),
                    )
                })?;
            let request = build_publish_request(&owned, public, &message, &tags)?;
            refreshed
                .client
                .publish_skill(&request)
                .await
                .map_err(client_error)?
        }
        Err(error) => return Err(client_error(error)),
    };
    // Publishing advances the resource generation without changing the private workspace
    // revision. Refresh local authority immediately so the next save uses the new CAS token.
    sync_once().await?;
    Ok(outcome)
}

fn build_publish_request(
    owned: &OwnedSkillRecord,
    public: bool,
    message: &Option<String>,
    tags: &[String],
) -> Result<PublishSkillRequest, RuntimeError> {
    let generation = u64::try_from(owned.workspace_generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "stored workspace generation is invalid",
        )
    })?;
    let operation_id = OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let operation = operation_id.to_string();
    let request_hash = publish_skill_request_hash(
        &operation,
        &owned.resource_id,
        generation,
        public,
        message.as_deref(),
        tags,
    )
    .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    Ok(PublishSkillRequest {
        operation_id: operation,
        resource_id: owned.resource_id.clone(),
        expected_generation: generation,
        public,
        message: message.clone(),
        tags: tags.to_vec(),
        request_hash: request_hash.to_string(),
    })
}

pub async fn history(locator: &str) -> Result<SkillHistoryResponse, RuntimeError> {
    let context = installed_context(true).await?;
    context
        .client
        .skill_history(locator)
        .await
        .map_err(client_error)
}

pub async fn diff(
    locator: &str,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<DiffOutcome, RuntimeError> {
    let context = installed_context(true).await?;
    let history = context
        .client
        .skill_history(locator)
        .await
        .map_err(client_error)?;
    let to_revision = match to {
        Some(value) => resolve_revision(&history, value)?,
        None => history.workspace_revision_id.clone(),
    };
    let from_revision = match from {
        Some(value) => resolve_revision(&history, value)?,
        None => history
            .revisions
            .iter()
            .find(|revision| revision.revision_id == to_revision)
            .and_then(|revision| revision.parent_revision_ids.first())
            .cloned()
            .ok_or_else(|| {
                RuntimeError::new(
                    CliErrorCode::InvalidArguments,
                    "the selected revision has no parent; provide both revisions explicitly",
                )
            })?,
    };
    let left = context
        .client
        .skill_revision(locator, &from_revision)
        .await
        .map_err(client_error)?;
    let right = context
        .client
        .skill_revision(locator, &to_revision)
        .await
        .map_err(client_error)?;
    Ok(diff_details(locator, &left, &right))
}

pub async fn restore(locator: &str, revision: &str) -> Result<RestoreOutcome, RuntimeError> {
    let context = installed_context(true).await?;
    if let Some(owned) = context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|skill| skill.locator == locator)
        && context
            .db
            .workspace_content_conflict(owned.resource_id)
            .await
            .map_err(local_error)?
            .is_some()
    {
        let history = context
            .client
            .skill_history(locator)
            .await
            .map_err(client_error)?;
        let target_revision_id = resolve_revision(&history, revision)?;
        let revision = crate::workspace_merge::resolve_workspace_conflict_with_revision(
            &context,
            locator,
            &target_revision_id,
        )
        .await?;
        return Ok(RestoreOutcome { revision });
    }
    drop(context);
    sync_once().await?;
    let context = installed_context(true).await?;
    let history = context
        .client
        .skill_history(locator)
        .await
        .map_err(client_error)?;
    let target_revision_id = resolve_revision(&history, revision)?;
    let owned = context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|skill| skill.resource_id == history.resource_id)
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::NotFound,
                format!("{locator} is not an owned skill on this identity"),
            )
        })?;
    let generation = u64::try_from(owned.workspace_generation).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            "stored workspace generation is invalid",
        )
    })?;
    let operation_id = OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let request_hash = restore_skill_request_hash(
        &operation_id.to_string(),
        &owned.resource_id,
        generation,
        &target_revision_id,
    )
    .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let request = RestoreSkillRequest {
        operation_id: operation_id.to_string(),
        resource_id: owned.resource_id,
        expected_generation: generation,
        target_revision_id,
        request_hash: request_hash.to_string(),
    };
    let revision = context
        .client
        .restore_skill(&request)
        .await
        .map_err(client_error)?;
    sync_once().await?;
    Ok(RestoreOutcome { revision })
}

pub async fn export(
    locator_or_version: &str,
    destination: &Path,
) -> Result<ExportOutcome, RuntimeError> {
    let (locator, version) = split_release_selector(locator_or_version)?;
    let context = installed_context(true).await?;
    let history = context
        .client
        .skill_history(&locator)
        .await
        .map_err(client_error)?;
    let revision_id = match version {
        Some(version) => resolve_revision(&history, &format!("v{version}"))?,
        None => history.workspace_revision_id.clone(),
    };
    let detail = context
        .client
        .skill_revision(&locator, &revision_id)
        .await
        .map_err(client_error)?;
    if detail.snapshot.size_bytes > context.limits.max_transfer_bytes {
        return Err(RuntimeError::new(
            CliErrorCode::ContentVerification,
            format!("snapshot for {locator} exceeds registry transfer limit"),
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
    let skill_name = locator
        .rsplit_once('/')
        .map(|(_, name)| name)
        .ok_or_else(|| {
            RuntimeError::new(CliErrorCode::InvalidArguments, "invalid skill locator")
        })?;
    export_skill_snapshot(skill_name, &manifest, &bytes, destination).map_err(local_error)?;
    Ok(ExportOutcome {
        locator,
        revision_id,
        destination: destination.to_path_buf(),
    })
}

fn resolve_revision(
    history: &SkillHistoryResponse,
    selector: &str,
) -> Result<String, RuntimeError> {
    if let Some(version) = selector
        .strip_prefix('v')
        .and_then(|value| value.parse::<u64>().ok())
    {
        return history
            .releases
            .iter()
            .find(|release| release.version == version)
            .map(|release| release.revision_id.clone())
            .ok_or_else(|| {
                RuntimeError::new(
                    CliErrorCode::NotFound,
                    format!("release v{version} not found"),
                )
            });
    }
    let matches = history
        .revisions
        .iter()
        .filter(|revision| revision.revision_id.starts_with(selector))
        .map(|revision| revision.revision_id.clone())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [revision] => Ok(revision.clone()),
        [] => Err(RuntimeError::new(
            CliErrorCode::NotFound,
            format!("revision {selector} not found"),
        )),
        _ => Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            format!("revision prefix {selector} is ambiguous"),
        )),
    }
}

fn diff_details(
    locator: &str,
    left: &SkillRevisionDetail,
    right: &SkillRevisionDetail,
) -> DiffOutcome {
    let left_entries = manifest_entries(&left.manifest.entries);
    let right_entries = manifest_entries(&right.manifest.entries);
    let mut paths = left_entries
        .keys()
        .chain(right_entries.keys())
        .copied()
        .collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();
    let changes = paths
        .into_iter()
        .filter_map(|path| {
            let change = match (left_entries.get(path), right_entries.get(path)) {
                (None, Some(_)) => "added",
                (Some(_), None) => "removed",
                (Some(before), Some(after)) if before != after => "changed",
                _ => return None,
            };
            Some(DiffEntry {
                path: path.to_owned(),
                change,
            })
        })
        .collect();
    DiffOutcome {
        locator: locator.to_owned(),
        from_revision: left.revision_id.clone(),
        to_revision: right.revision_id.clone(),
        changes,
    }
}

fn manifest_entries(
    entries: &[PublicSkillManifestEntry],
) -> BTreeMap<&str, &PublicSkillManifestEntry> {
    entries
        .iter()
        .map(|entry| {
            let path = match entry {
                PublicSkillManifestEntry::File { path, .. }
                | PublicSkillManifestEntry::Directory { path }
                | PublicSkillManifestEntry::Symlink { path, .. } => path.as_str(),
            };
            (path, entry)
        })
        .collect()
}

fn split_release_selector(value: &str) -> Result<(String, Option<u64>), RuntimeError> {
    if let Some((locator, suffix)) = value.rsplit_once("@v")
        && locator.starts_with('@')
    {
        let version = suffix.parse::<u64>().map_err(|_| {
            RuntimeError::new(
                CliErrorCode::InvalidArguments,
                "release selector must look like @owner/skill@v7",
            )
        })?;
        return Ok((locator.to_owned(), Some(version)));
    }
    Ok((value.to_owned(), None))
}
