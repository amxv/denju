use std::{collections::BTreeMap, str::FromStr};

use denju_client::ClientError;
use denju_core::{
    AuthorPrincipalId, BlobId, OperationId, ResourceId, ResourceLocator, Revision, RevisionId,
    build_deterministic_skill_snapshot, rewrite_skill_document_name, skill_document_declared_name,
    validate_skill_snapshot,
};
use denju_local::{
    DesiredSkillMaterialization, LocalForkRecord, LocalRevisionRecord, ManagedDesiredKind,
    ManagedSkillRecord, OwnedSkillRecord, journaled_remove_managed_skill,
    materialize_skill_snapshot, reconcile_harness_projections, workspace_entries_from_manifest,
};
use denju_wire::{
    ApiErrorCode, CliErrorCode, ForkImportIntent, PrivateRevisionCommitRequest,
    PrivateRevisionCommitResponse, PrivateRevisionRequest, PrivateSkill,
    PrivateSkillImportCommitRequest, PrivateSkillImportRequest, PrivateSkillImportResponse,
    PublicSkillManifest, private_revision_request_hash, private_skill_import_request_hash,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    context::{InstalledContext, client_error, local_error, now_unix_ms},
    public::installed_context,
    setup::RuntimeError,
};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ForkOutcome {
    pub(crate) state: &'static str,
    pub(crate) resource_id: String,
    pub(crate) locator: String,
    pub(crate) revision_id: String,
    pub(crate) upstream_locator: String,
    pub(crate) sync_base_revision_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ForkSyncOutcome {
    pub(crate) state: &'static str,
    pub(crate) locator: String,
    pub(crate) revision_id: String,
    pub(crate) upstream_locator: String,
    pub(crate) upstream_revision_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ForkResolveOutcome {
    pub(crate) state: &'static str,
    pub(crate) upstream_locator: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) locator: Option<String>,
}

struct PromotionRename {
    operation: OperationId,
    author: String,
    revision_id: String,
    manifest: PublicSkillManifest,
    entries: Vec<denju_core::OwnedSkillEntry>,
    snapshot: denju_core::DeterministicSkillSnapshot,
}

struct ForkImportSpec<'a> {
    owner: &'a str,
    name: &'a str,
    operation: OperationId,
    author: &'a str,
    manifest: &'a PublicSkillManifest,
    snapshot: &'a [u8],
    entries: &'a [denju_core::OwnedSkillEntry],
    fork: ForkImportIntent,
    expected_revision_id: &'a str,
}

pub(crate) async fn promote_local_forks(context: &InstalledContext) -> Result<(), RuntimeError> {
    let identity = match context.db.identity().await.map_err(local_error)? {
        Some(identity) if identity.session_backend.is_some() => identity,
        _ => return Ok(()),
    };
    let installation = context
        .db
        .installation()
        .await
        .map_err(local_error)?
        .ok_or_else(|| RuntimeError::new(CliErrorCode::SetupRequired, "Denju is not set up"))?;
    let user_author = identity.author_principal_id.as_deref();
    for fork in context.db.local_forks().await.map_err(local_error)? {
        if fork.state != "local" {
            continue;
        }
        match promote_one(
            context,
            &fork,
            &installation.author_principal_id,
            user_author,
            &identity.username,
        )
        .await
        {
            Ok(()) => {}
            Err(PromoteError::Client(ClientError::Registry(api)))
                if api.code == ApiErrorCode::GenerationConflict =>
            {
                let mut blocked = fork.clone();
                blocked.state = "name_conflict".to_owned();
                context
                    .db
                    .save_local_fork(blocked, now_unix_ms())
                    .await
                    .map_err(local_error)?;
            }
            Err(PromoteError::Client(error)) => return Err(client_error(error)),
            Err(PromoteError::Runtime(error)) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) async fn create(locator: &str) -> Result<ForkOutcome, RuntimeError> {
    let context = installed_context(true).await?;
    let identity = require_identity(&context).await?;
    let target = context
        .client
        .subscription_target(locator)
        .await
        .map_err(client_error)?;
    let history = context
        .client
        .skill_history(&target.locator)
        .await
        .map_err(client_error)?;
    let upstream_revision = history.workspace_revision_id;
    let detail = context
        .client
        .skill_revision(&target.locator, &upstream_revision)
        .await
        .map_err(client_error)?;
    let bytes = context
        .client
        .download_snapshot(&detail.snapshot)
        .await
        .map_err(client_error)?;
    let manifest = detail
        .manifest
        .to_core()
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error))?;
    let entries = validate_skill_snapshot(&target.name, &manifest, &bytes).map_err(local_error)?;
    let operation = new_operation()?;
    let author = identity.author_principal_id.as_deref().ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::CredentialUnavailable,
            "fork requires a hydrated user author principal",
        )
    })?;
    let revision = Revision::new(
        manifest.root_tree(),
        vec![RevisionId::from_str(&upstream_revision).map_err(local_error)?],
        AuthorPrincipalId::from_str(author).map_err(local_error)?,
        operation,
    )
    .map_err(local_error)?;
    let expected_revision_id = revision.id().to_string();
    let response = import_fork(
        &context,
        ForkImportSpec {
            owner: &identity.username,
            name: &target.name,
            operation,
            author,
            manifest: &detail.manifest,
            snapshot: &bytes,
            entries: &entries,
            fork: ForkImportIntent {
                upstream_resource_id: target.resource_id,
                upstream_revision_id: upstream_revision.clone(),
                replace_subscription: false,
                promotion_head_revision_id: None,
                historical_skill_name: None,
            },
            expected_revision_id: &expected_revision_id,
        },
    )
    .await
    .map_err(map_promote_error)?;
    install_server_fork(
        &context,
        None,
        &response,
        response.generation,
        &response.revision_id,
        &detail.manifest,
        &bytes,
    )
    .await?;
    Ok(ForkOutcome {
        state: "created",
        resource_id: response.resource_id,
        locator: response.locator,
        revision_id: response.revision_id,
        upstream_locator: target.locator,
        sync_base_revision_id: upstream_revision,
    })
}

async fn promote_one(
    context: &InstalledContext,
    fork: &LocalForkRecord,
    installation_author: &str,
    user_author: Option<&str>,
    username: &str,
) -> Result<(), PromoteError> {
    let history = ordered_history(
        &context
            .db
            .local_revision_history(fork.resource_id.clone())
            .await
            .map_err(|error| PromoteError::Runtime(local_error(error)))?,
        &fork.created_from_revision_id,
    )
    .map_err(PromoteError::Runtime)?;
    let first = history.first().ok_or_else(|| {
        PromoteError::Runtime(RuntimeError::new(
            CliErrorCode::LocalState,
            "local fork has no revision history",
        ))
    })?;
    let (first_manifest, first_entries, first_snapshot) =
        local_revision_snapshot(context, first).map_err(PromoteError::Runtime)?;
    let first_name = revision_skill_name(&first_entries).map_err(PromoteError::Runtime)?;
    let last_record = history.last().ok_or_else(|| {
        PromoteError::Runtime(RuntimeError::new(
            CliErrorCode::LocalState,
            "local fork has no final revision",
        ))
    })?;
    let (last_manifest, last_entries, last_snapshot) =
        local_revision_snapshot(context, last_record).map_err(PromoteError::Runtime)?;
    let last_name = revision_skill_name(&last_entries).map_err(PromoteError::Runtime)?;
    let rename = if last_name == fork.desired_name {
        None
    } else {
        let user_author = user_author.ok_or_else(|| {
            PromoteError::Runtime(RuntimeError::new(
                CliErrorCode::CredentialUnavailable,
                "resolving a local fork name requires a claimed user author principal",
            ))
        })?;
        Some(
            build_promotion_rename(fork, last_record, &last_entries, &last_name, user_author)
                .map_err(PromoteError::Runtime)?,
        )
    };
    let promotion_head_revision_id = rename
        .as_ref()
        .map(|rename| rename.revision_id.clone())
        .unwrap_or_else(|| last_record.revision_id.clone());
    let first_author =
        infer_revision_author(first, &first_manifest, installation_author, user_author)
            .map_err(PromoteError::Runtime)?;
    let operation = OperationId::from_str(&first.operation_id)
        .map_err(|error| PromoteError::Runtime(local_error(error)))?;
    let imported = import_fork(
        context,
        ForkImportSpec {
            owner: username,
            name: &fork.desired_name,
            operation,
            author: &first_author,
            manifest: &first_manifest,
            snapshot: first_snapshot.bytes(),
            entries: &first_entries,
            fork: ForkImportIntent {
                upstream_resource_id: fork.upstream_resource_id.clone(),
                upstream_revision_id: fork.created_from_revision_id.clone(),
                replace_subscription: fork.replace_subscription,
                promotion_head_revision_id: Some(promotion_head_revision_id),
                historical_skill_name: (first_name != fork.desired_name).then_some(first_name),
            },
            expected_revision_id: &first.revision_id,
        },
    )
    .await?;

    let mut generation = imported.generation;
    let mut revision_id = imported.revision_id.clone();
    for revision in history.iter().skip(1) {
        let (manifest, entries, _) =
            local_revision_snapshot(context, revision).map_err(PromoteError::Runtime)?;
        let historical_name = revision_skill_name(&entries).map_err(PromoteError::Runtime)?;
        let author = infer_revision_author(revision, &manifest, installation_author, user_author)
            .map_err(PromoteError::Runtime)?;
        let operation = OperationId::from_str(&revision.operation_id)
            .map_err(|error| PromoteError::Runtime(local_error(error)))?;
        if revision.expected_head_revision_id != revision_id {
            return Err(PromoteError::Runtime(RuntimeError::new(
                CliErrorCode::LocalState,
                "local fork revision chain does not match the promoted server head",
            )));
        }
        let operation_id = operation.to_string();
        let request_hash =
            private_revision_request_hash(&denju_wire::PrivateRevisionRequestHashInput {
                operation_id: &operation_id,
                resource_id: &imported.resource_id,
                expected_generation: generation,
                expected_head_revision_id: &revision.expected_head_revision_id,
                parent_revision_ids: &revision.parent_revision_ids,
                manifest: &manifest,
                revision_author_principal_id: Some(&author),
                fork_sync: None,
                historical_skill_name: (historical_name != fork.desired_name)
                    .then_some(historical_name.as_str()),
            })
            .map_err(|error| PromoteError::Runtime(internal_error(error)))?;
        let request = PrivateRevisionRequest {
            operation_id: operation_id.clone(),
            resource_id: imported.resource_id.clone(),
            expected_generation: generation,
            expected_head_revision_id: revision.expected_head_revision_id.clone(),
            parent_revision_ids: revision.parent_revision_ids.clone(),
            manifest: manifest.clone(),
            revision_author_principal_id: Some(author),
            fork_sync: None,
            historical_skill_name: (historical_name != fork.desired_name)
                .then_some(historical_name),
            request_hash: request_hash.to_string(),
        };
        let prepared = context
            .client
            .prepare_private_revision(&request)
            .await
            .map_err(PromoteError::Client)?;
        if prepared.revision_id != revision.revision_id {
            return Err(PromoteError::Runtime(RuntimeError::new(
                CliErrorCode::LocalState,
                "registry rewrote an anonymous fork revision identity during claim",
            )));
        }
        upload_entries(context, &prepared.uploads, &entries)
            .await
            .map_err(PromoteError::Runtime)?;
        let committed = context
            .client
            .commit_private_revision(&PrivateRevisionCommitRequest {
                operation_id: operation.to_string(),
                request_hash: request_hash.to_string(),
            })
            .await
            .map_err(PromoteError::Client)?;
        let advanced = match committed {
            PrivateRevisionCommitResponse::Advanced { revision } => revision,
            PrivateRevisionCommitResponse::Diverged { .. } => {
                return Err(PromoteError::Runtime(RuntimeError::new(
                    CliErrorCode::LocalState,
                    "newly promoted fork diverged while replaying local history",
                )));
            }
        };
        if advanced.revision_id != revision.revision_id {
            return Err(PromoteError::Runtime(RuntimeError::new(
                CliErrorCode::LocalState,
                "registry committed a different revision while promoting local fork history",
            )));
        }
        generation = advanced.generation;
        revision_id = advanced.revision_id;
    }

    let (final_manifest, final_snapshot) = if let Some(rename) = rename {
        if revision_id != last_record.revision_id {
            return Err(PromoteError::Runtime(RuntimeError::new(
                CliErrorCode::LocalState,
                "local fork promotion did not finish at its preserved history head",
            )));
        }
        let rename_operation_id = rename.operation.to_string();
        let request_hash =
            private_revision_request_hash(&denju_wire::PrivateRevisionRequestHashInput {
                operation_id: &rename_operation_id,
                resource_id: &imported.resource_id,
                expected_generation: generation,
                expected_head_revision_id: &revision_id,
                parent_revision_ids: std::slice::from_ref(&revision_id),
                manifest: &rename.manifest,
                revision_author_principal_id: Some(&rename.author),
                fork_sync: None,
                historical_skill_name: None,
            })
            .map_err(|error| PromoteError::Runtime(internal_error(error)))?;
        let request = PrivateRevisionRequest {
            operation_id: rename_operation_id.clone(),
            resource_id: imported.resource_id.clone(),
            expected_generation: generation,
            expected_head_revision_id: revision_id.clone(),
            parent_revision_ids: vec![revision_id.clone()],
            manifest: rename.manifest.clone(),
            revision_author_principal_id: Some(rename.author.clone()),
            fork_sync: None,
            historical_skill_name: None,
            request_hash: request_hash.to_string(),
        };
        let prepared = context
            .client
            .prepare_private_revision(&request)
            .await
            .map_err(PromoteError::Client)?;
        if prepared.revision_id != rename.revision_id {
            return Err(PromoteError::Runtime(RuntimeError::new(
                CliErrorCode::LocalState,
                "registry rewrote the deterministic fork collision rename revision",
            )));
        }
        upload_entries(context, &prepared.uploads, &rename.entries)
            .await
            .map_err(PromoteError::Runtime)?;
        let committed = context
            .client
            .commit_private_revision(&PrivateRevisionCommitRequest {
                operation_id: rename.operation.to_string(),
                request_hash: request_hash.to_string(),
            })
            .await
            .map_err(PromoteError::Client)?;
        let advanced = match committed {
            PrivateRevisionCommitResponse::Advanced { revision } => revision,
            PrivateRevisionCommitResponse::Diverged { .. } => {
                return Err(PromoteError::Runtime(RuntimeError::new(
                    CliErrorCode::LocalState,
                    "fork collision rename diverged while finishing promotion",
                )));
            }
        };
        if advanced.revision_id != rename.revision_id {
            return Err(PromoteError::Runtime(RuntimeError::new(
                CliErrorCode::LocalState,
                "registry committed a different fork collision rename revision",
            )));
        }
        generation = advanced.generation;
        revision_id = advanced.revision_id;
        (rename.manifest, rename.snapshot)
    } else {
        (last_manifest, last_snapshot)
    };

    install_server_fork(
        context,
        Some(fork),
        &imported,
        generation,
        &revision_id,
        &final_manifest,
        final_snapshot.bytes(),
    )
    .await
    .map_err(PromoteError::Runtime)?;
    let expected_locator = format!(
        "@{}/{}",
        username.strip_prefix('@').unwrap_or(username),
        fork.desired_name
    );
    if imported.locator != expected_locator {
        return Err(PromoteError::Runtime(RuntimeError::new(
            CliErrorCode::LocalState,
            "promoted fork locator did not match the claimed identity",
        )));
    }
    Ok(())
}

async fn import_fork(
    context: &InstalledContext,
    spec: ForkImportSpec<'_>,
) -> Result<PrivateSkillImportResponse, PromoteError> {
    let snapshot_sha = BlobId::hash(spec.snapshot).to_string();
    let snapshot_size = u64::try_from(spec.snapshot.len()).map_err(|_| {
        PromoteError::Runtime(RuntimeError::new(
            CliErrorCode::ContentVerification,
            "fork snapshot is too large",
        ))
    })?;
    let operation_id = spec.operation.to_string();
    let request_hash =
        private_skill_import_request_hash(&denju_wire::PrivateSkillImportRequestHashInput {
            operation_id: &operation_id,
            expected_generation: 0,
            owner: spec.owner,
            name: spec.name,
            manifest: spec.manifest,
            snapshot_sha256: &snapshot_sha,
            snapshot_size_bytes: snapshot_size,
            revision_author_principal_id: Some(spec.author),
            fork: Some(&spec.fork),
        })
        .map_err(|error| PromoteError::Runtime(internal_error(error)))?;
    let prepared = context
        .client
        .prepare_private_skill_import(&PrivateSkillImportRequest {
            operation_id: operation_id.clone(),
            expected_generation: 0,
            owner: spec.owner.to_owned(),
            name: spec.name.to_owned(),
            manifest: spec.manifest.clone(),
            snapshot_sha256: snapshot_sha,
            snapshot_size_bytes: snapshot_size,
            revision_author_principal_id: Some(spec.author.to_owned()),
            fork: Some(spec.fork),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(PromoteError::Client)?;
    if prepared.revision_id != spec.expected_revision_id {
        return Err(PromoteError::Runtime(RuntimeError::new(
            CliErrorCode::LocalState,
            "registry computed a different fork revision identity",
        )));
    }
    upload_entries(context, &prepared.uploads, spec.entries)
        .await
        .map_err(PromoteError::Runtime)?;
    let committed = context
        .client
        .commit_private_skill_import(&PrivateSkillImportCommitRequest {
            operation_id,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(PromoteError::Client)?;
    if committed.revision_id != spec.expected_revision_id {
        return Err(PromoteError::Runtime(RuntimeError::new(
            CliErrorCode::LocalState,
            "registry committed a different fork revision identity",
        )));
    }
    Ok(committed)
}

pub(crate) async fn upload_entries(
    context: &InstalledContext,
    uploads: &[denju_wire::StagedBlobUpload],
    entries: &[denju_core::OwnedSkillEntry],
) -> Result<(), RuntimeError> {
    let by_blob = entries
        .iter()
        .filter_map(|entry| match entry {
            denju_core::OwnedSkillEntry::File { bytes, .. } => Some((BlobId::hash(bytes), bytes)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    for upload in uploads {
        let blob = BlobId::from_str(&upload.blob_id).map_err(local_error)?;
        let bytes = by_blob.get(&blob).ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                format!("registry requested undeclared fork blob {blob}"),
            )
        })?;
        context
            .client
            .upload_staged_blob(upload, bytes)
            .await
            .map_err(client_error)?;
    }
    Ok(())
}

pub(crate) fn local_revision_snapshot(
    context: &InstalledContext,
    revision: &LocalRevisionRecord,
) -> Result<
    (
        PublicSkillManifest,
        Vec<denju_core::OwnedSkillEntry>,
        denju_core::DeterministicSkillSnapshot,
    ),
    RuntimeError,
> {
    let manifest: PublicSkillManifest = serde_json::from_str(&revision.manifest_json)
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
    let core = manifest
        .to_core()
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error))?;
    let entries = workspace_entries_from_manifest(&context.paths, &core).map_err(local_error)?;
    let skill_md = entries
        .iter()
        .find_map(|entry| match entry {
            denju_core::OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                Some(bytes.as_slice())
            }
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeError::new(CliErrorCode::LocalState, "local fork is missing SKILL.md")
        })?;
    let name = skill_document_declared_name(skill_md).map_err(local_error)?;
    let snapshot = build_deterministic_skill_snapshot(&name, &entries).map_err(local_error)?;
    if snapshot.manifest() != &core {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            "local fork CAS does not match its immutable revision manifest",
        ));
    }
    Ok((manifest, entries, snapshot))
}

fn revision_skill_name(entries: &[denju_core::OwnedSkillEntry]) -> Result<String, RuntimeError> {
    let skill_md = entries
        .iter()
        .find_map(|entry| match entry {
            denju_core::OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                Some(bytes.as_slice())
            }
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "fork revision is missing SKILL.md",
            )
        })?;
    skill_document_declared_name(skill_md).map_err(local_error)
}

fn build_promotion_rename(
    fork: &LocalForkRecord,
    last: &LocalRevisionRecord,
    entries: &[denju_core::OwnedSkillEntry],
    old_name: &str,
    user_author: &str,
) -> Result<PromotionRename, RuntimeError> {
    let mut renamed = entries.to_vec();
    let skill_md = renamed
        .iter_mut()
        .find_map(|entry| match entry {
            denju_core::OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                Some(bytes)
            }
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::LocalState,
                "fork revision is missing SKILL.md",
            )
        })?;
    *skill_md =
        rewrite_skill_document_name(old_name, skill_md, &fork.desired_name).map_err(local_error)?;
    let snapshot =
        build_deterministic_skill_snapshot(&fork.desired_name, &renamed).map_err(local_error)?;
    let manifest = PublicSkillManifest::from_core(snapshot.manifest());
    let resource = ResourceId::from_str(&fork.resource_id).map_err(local_error)?;
    let operation = OperationId::from_uuid(resource.as_uuid()).map_err(local_error)?;
    let author = AuthorPrincipalId::from_str(user_author).map_err(local_error)?;
    let parent = RevisionId::from_str(&last.revision_id).map_err(local_error)?;
    let revision = Revision::new(
        snapshot.manifest().root_tree(),
        vec![parent],
        author,
        operation,
    )
    .map_err(local_error)?;
    Ok(PromotionRename {
        operation,
        author: user_author.to_owned(),
        revision_id: revision.id().to_string(),
        manifest,
        entries: renamed,
        snapshot,
    })
}

fn infer_revision_author(
    record: &LocalRevisionRecord,
    manifest: &PublicSkillManifest,
    installation_author: &str,
    user_author: Option<&str>,
) -> Result<String, RuntimeError> {
    let manifest = manifest
        .to_core()
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error))?;
    let operation = OperationId::from_str(&record.operation_id).map_err(local_error)?;
    let parents = record
        .parent_revision_ids
        .iter()
        .map(|parent| RevisionId::from_str(parent).map_err(local_error))
        .collect::<Result<Vec<_>, _>>()?;
    let mut candidates = vec![installation_author];
    if let Some(user_author) = user_author
        && user_author != installation_author
    {
        candidates.push(user_author);
    }
    for candidate in candidates {
        let author = AuthorPrincipalId::from_str(candidate).map_err(local_error)?;
        let revision = Revision::new(manifest.root_tree(), parents.clone(), author, operation)
            .map_err(local_error)?;
        if revision.id().to_string() == record.revision_id {
            return Ok(candidate.to_owned());
        }
    }
    Err(RuntimeError::new(
        CliErrorCode::LocalState,
        "local fork revision author is not linked to this installation or claimed user",
    ))
}

pub(crate) fn ordered_history(
    records: &[LocalRevisionRecord],
    upstream_revision: &str,
) -> Result<Vec<LocalRevisionRecord>, RuntimeError> {
    let mut remaining = records.to_vec();
    let mut ordered = Vec::with_capacity(remaining.len());
    let mut head = upstream_revision.to_owned();
    while !remaining.is_empty() {
        let position = remaining
            .iter()
            .position(|record| {
                record.parent_revision_ids.len() == 1
                    && record.expected_head_revision_id == head
                    && record.parent_revision_ids[0] == head
            })
            .ok_or_else(|| {
                RuntimeError::new(
                    CliErrorCode::LocalState,
                    "local fork revision history is not a single preserved chain",
                )
            })?;
        let record = remaining.remove(position);
        head = record.revision_id.clone();
        ordered.push(record);
    }
    Ok(ordered)
}

async fn install_server_fork(
    context: &InstalledContext,
    local: Option<&LocalForkRecord>,
    remote: &PrivateSkillImportResponse,
    generation: u64,
    revision_id: &str,
    manifest: &PublicSkillManifest,
    snapshot: &[u8],
) -> Result<(), RuntimeError> {
    if let Some(local) = local {
        let owned = context
            .db
            .owned_skills()
            .await
            .map_err(local_error)?
            .into_iter()
            .find(|record| record.resource_id == local.resource_id)
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
        .map_err(local_error)?;
    }
    let synthetic = PrivateSkill {
        resource_id: remote.resource_id.clone(),
        locator: remote.locator.clone(),
        owner: remote.owner.clone(),
        name: remote.name.clone(),
        description: remote.description.clone(),
        generation,
        workspace_generation: generation,
        revision_id: revision_id.to_owned(),
        manifest: manifest.clone(),
        snapshot: denju_wire::SnapshotDownload {
            sha256: BlobId::hash(snapshot).to_string(),
            size_bytes: u64::try_from(snapshot.len()).unwrap_or(u64::MAX),
            url: String::new(),
        },
        conflicts: Vec::new(),
        fork: remote.fork.clone(),
    };
    install_owned_revision(
        context,
        &synthetic,
        generation,
        revision_id,
        manifest,
        snapshot,
    )
    .await
}

pub(crate) async fn install_owned_revision(
    context: &InstalledContext,
    remote: &PrivateSkill,
    generation: u64,
    revision_id: &str,
    manifest: &PublicSkillManifest,
    snapshot: &[u8],
) -> Result<(), RuntimeError> {
    let generation_i64 = i64::try_from(generation)
        .map_err(|_| RuntimeError::new(CliErrorCode::LocalState, "fork generation is too large"))?;
    context
        .db
        .upsert_owned_skill_desired(
            OwnedSkillRecord {
                resource_id: remote.resource_id.clone(),
                locator: remote.locator.clone(),
                owner: remote.owner.clone(),
                skill_name: remote.name.clone(),
                resource_generation: generation_i64,
                workspace_generation: generation_i64,
                desired_revision_id: revision_id.to_owned(),
                harness_name: None,
                materialized_revision_id: None,
            },
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    let core = manifest
        .to_core()
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error))?;
    let desired = DesiredSkillMaterialization {
        resource_id: ResourceId::from_str(&remote.resource_id).map_err(local_error)?,
        owner: remote.owner.clone(),
        skill_name: remote.name.clone(),
        revision_id: RevisionId::from_str(revision_id).map_err(local_error)?,
        manifest: core.clone(),
    };
    let path = materialize_skill_snapshot(&context.paths, &context.db, &desired, snapshot)
        .await
        .map_err(local_error)?;
    context
        .db
        .clear_workspace_file_index(remote.resource_id.clone())
        .await
        .map_err(local_error)?;
    context
        .db
        .ensure_workspace_baseline(
            remote.resource_id.clone(),
            generation_i64,
            revision_id.to_owned(),
            core.root_tree().to_string(),
            path.display().to_string(),
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    context
        .db
        .advance_clean_workspace_baseline(
            remote.resource_id.clone(),
            generation_i64,
            revision_id.to_owned(),
            core.root_tree().to_string(),
            path.display().to_string(),
            now_unix_ms(),
        )
        .await
        .map_err(local_error)?;
    reconcile_harness_projections(&context.paths, &context.db, &context.roots)
        .await
        .map_err(local_error)?;
    Ok(())
}

pub(crate) async fn fetch_revision_entries(
    context: &InstalledContext,
    locator: &str,
    skill_name: &str,
    revision: &str,
) -> Result<Vec<denju_core::OwnedSkillEntry>, RuntimeError> {
    let detail = context
        .client
        .skill_revision(locator, revision)
        .await
        .map_err(client_error)?;
    let bytes = context
        .client
        .download_snapshot(&detail.snapshot)
        .await
        .map_err(client_error)?;
    let manifest = detail
        .manifest
        .to_core()
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error))?;
    validate_skill_snapshot(skill_name, &manifest, &bytes).map_err(local_error)
}

pub(crate) async fn find_owned(
    context: &InstalledContext,
    locator: &str,
) -> Result<PrivateSkill, RuntimeError> {
    let requested = ResourceLocator::from_str(locator)
        .map_err(|error| RuntimeError::new(CliErrorCode::InvalidArguments, error.to_string()))?;
    context
        .client
        .private_skills()
        .await
        .map_err(client_error)?
        .skills
        .into_iter()
        .find(|skill| skill.locator == requested.to_string())
        .ok_or_else(|| {
            RuntimeError::new(CliErrorCode::NotFound, format!("{} not found", requested))
        })
}

pub(crate) async fn require_identity(
    context: &InstalledContext,
) -> Result<denju_local::IdentityRecord, RuntimeError> {
    context
        .db
        .identity()
        .await
        .map_err(local_error)?
        .filter(|identity| identity.session_backend.is_some())
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::CredentialUnavailable,
                "fork requires a claimed Denju identity",
            )
            .recovery("denju claim <username>")
        })
}

pub(crate) fn new_operation() -> Result<OperationId, RuntimeError> {
    OperationId::from_uuid(Uuid::now_v7()).map_err(internal_error)
}

enum PromoteError {
    Client(ClientError),
    Runtime(RuntimeError),
}

fn map_promote_error(error: PromoteError) -> RuntimeError {
    match error {
        PromoteError::Client(error) => client_error(error),
        PromoteError::Runtime(error) => error,
    }
}

pub(crate) fn internal_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::Internal, error.to_string())
}
