use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
};

use denju_core::{
    BlobId, OperationId, ResourceId, ResourceLocator, RevisionId, SkillManifest,
    build_deterministic_skill_snapshot, build_skill_manifest, validate_skill_snapshot,
};
use denju_local::{
    DesiredSkillMaterialization, ImportJournal, ImportJournalPayload, JournalState,
    OwnedSkillRecord, materialize_skill_snapshot, read_skill_source, reconcile_harness_projections,
};
use denju_wire::{
    CliErrorCode, PrivateSkillImportCommitRequest, PrivateSkillImportRequest, PublicSkillManifest,
    private_skill_import_request_hash,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    public::{client_error, installed_context, local_error},
    setup::RuntimeError,
};

#[derive(Debug, Clone, Serialize)]
pub struct ImportOutcome {
    pub state: &'static str,
    pub resource_id: String,
    pub locator: String,
    pub revision_id: String,
    pub harness_name: String,
}

pub async fn import(source: &Path) -> Result<ImportOutcome, RuntimeError> {
    let context = installed_context(true).await?;
    let identity = context
        .db
        .identity()
        .await
        .map_err(local_error)?
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::CredentialUnavailable,
                "import requires a claimed Denju identity",
            )
            .recovery("denju claim <username>")
        })?;
    if identity.session_backend.is_none() {
        return Err(RuntimeError::new(
            CliErrorCode::CredentialUnavailable,
            format!("{} is not logged in on this device", identity.username),
        )
        .recovery(format!("denju login {}", identity.username)));
    }

    fs::create_dir_all(&context.paths.imports).map_err(local_error)?;
    let absolute_candidate = absolute_candidate(source).map_err(local_error)?;
    let candidate_text = absolute_candidate.display().to_string();
    let mut journal = context
        .db
        .import_journal_for_source(candidate_text)
        .await
        .map_err(local_error)?;

    if journal.is_none() && absolute_candidate.exists() {
        // Reject a symlink root before canonicalization changes what the user actually named.
        read_skill_source(&absolute_candidate).map_err(source_error)?;
        let canonical = fs::canonicalize(&absolute_candidate).map_err(local_error)?;
        let canonical_text = canonical.display().to_string();
        journal = context
            .db
            .import_journal_for_source(canonical_text.clone())
            .await
            .map_err(local_error)?;
        if journal.is_none() {
            if canonical.starts_with(&context.paths.root) {
                return Err(RuntimeError::new(
                    CliErrorCode::InvalidArguments,
                    "cannot import a path already inside Denju managed state",
                ));
            }
            journal = Some(create_import_journal(&context, &canonical, canonical_text).await?);
        }
    }
    let mut journal = journal.ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::InvalidArguments,
            format!(
                "skill source does not exist: {}",
                absolute_candidate.display()
            ),
        )
    })?;

    let (manifest_wire, manifest, snapshot_bytes, entries) = load_staged_import(&journal.payload)?;
    let request = import_request(&journal, &manifest_wire)?;

    if matches!(journal.state, JournalState::Planned | JournalState::Staged) {
        let prepared = context
            .client
            .prepare_private_skill_import(&request)
            .await
            .map_err(client_error)?;
        verify_or_record(
            &mut journal.payload.resource_id,
            &prepared.resource_id,
            "resource ID",
        )?;
        verify_or_record(&mut journal.payload.locator, &prepared.locator, "locator")?;
        verify_or_record(
            &mut journal.payload.revision_id,
            &prepared.revision_id,
            "revision ID",
        )?;

        let blob_bytes = file_bytes_by_blob(&entries);
        for upload in &prepared.uploads {
            let blob = BlobId::from_str(&upload.blob_id)
                .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
            let bytes = blob_bytes.get(&blob).ok_or_else(|| {
                RuntimeError::new(
                    CliErrorCode::LocalState,
                    format!("registry requested undeclared blob {blob}"),
                )
            })?;
            context
                .client
                .upload_staged_blob(upload, bytes)
                .await
                .map_err(client_error)?;
        }
        if journal.state == JournalState::Planned {
            context
                .db
                .update_import_journal(
                    journal.operation_id,
                    JournalState::Planned,
                    JournalState::Staged,
                    journal.payload.clone(),
                    now_unix_ms(),
                )
                .await
                .map_err(local_error)?;
            journal.state = JournalState::Staged;
        }

        let committed = context
            .client
            .commit_private_skill_import(&PrivateSkillImportCommitRequest {
                operation_id: journal.operation_id.to_string(),
                request_hash: journal.payload.request_hash.clone(),
            })
            .await
            .map_err(client_error)?;
        verify_committed_identity(
            &journal.payload,
            &committed.resource_id,
            &committed.locator,
            &committed.revision_id,
        )?;
        context
            .db
            .update_import_journal(
                journal.operation_id,
                JournalState::Staged,
                JournalState::Verified,
                journal.payload.clone(),
                now_unix_ms(),
            )
            .await
            .map_err(local_error)?;
        journal.state = JournalState::Verified;
    }

    let resource_id_text =
        required_payload(&journal.payload.resource_id, "resource ID")?.to_owned();
    let locator_text = required_payload(&journal.payload.locator, "locator")?.to_owned();
    let revision_id_text =
        required_payload(&journal.payload.revision_id, "revision ID")?.to_owned();
    let locator = ResourceLocator::from_str(&locator_text)
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;

    let mut harness_name = None;
    if journal.state == JournalState::Verified {
        context
            .db
            .upsert_owned_skill_desired(
                OwnedSkillRecord {
                    resource_id: resource_id_text.clone(),
                    locator: locator_text.clone(),
                    owner: locator.owner().to_owned(),
                    skill_name: journal.payload.skill_name.clone(),
                    resource_generation: 1,
                    desired_revision_id: revision_id_text.clone(),
                    harness_name: None,
                    materialized_revision_id: None,
                },
                now_unix_ms(),
            )
            .await
            .map_err(local_error)?;
        let desired = DesiredSkillMaterialization {
            resource_id: ResourceId::from_str(&resource_id_text).map_err(local_error)?,
            owner: locator.owner().to_owned(),
            skill_name: journal.payload.skill_name.clone(),
            revision_id: RevisionId::from_str(&revision_id_text).map_err(local_error)?,
            manifest: manifest.clone(),
        };
        materialize_skill_snapshot(&context.paths, &context.db, &desired, &snapshot_bytes)
            .await
            .map_err(|error| {
                RuntimeError::new(CliErrorCode::ContentVerification, error.to_string())
                    .recovery(format!("denju import {}", journal.payload.source_path))
            })?;
        let projections =
            reconcile_harness_projections(&context.paths, &context.db, &context.roots)
                .await
                .map_err(local_error)?;
        harness_name = projection_name(&projections, &locator_text);
        verify_source_unchanged(&journal.payload, &manifest)?;
        context
            .db
            .update_import_journal(
                journal.operation_id,
                JournalState::Verified,
                JournalState::Switched,
                journal.payload.clone(),
                now_unix_ms(),
            )
            .await
            .map_err(local_error)?;
        journal.state = JournalState::Switched;
    }

    if journal.state == JournalState::Switched {
        let source = PathBuf::from(&journal.payload.source_path);
        if source.exists() {
            verify_source_unchanged(&journal.payload, &manifest)?;
            fs::remove_dir_all(&source).map_err(|error| {
                RuntimeError::new(
                    CliErrorCode::LocalState,
                    format!("managed import is safe, but failed to remove source: {error}"),
                )
                .recovery(format!("denju import {}", journal.payload.source_path))
            })?;
        }
        let projections =
            reconcile_harness_projections(&context.paths, &context.db, &context.roots)
                .await
                .map_err(local_error)?;
        harness_name = projection_name(&projections, &locator_text).or(harness_name);
        context
            .db
            .update_import_journal(
                journal.operation_id,
                JournalState::Switched,
                JournalState::Complete,
                journal.payload.clone(),
                now_unix_ms(),
            )
            .await
            .map_err(local_error)?;
        let _ = fs::remove_file(&journal.payload.snapshot_path);
    }

    let harness_name = match harness_name {
        Some(name) => name,
        None => context
            .db
            .managed_skills()
            .await
            .map_err(local_error)?
            .into_iter()
            .find(|record| record.resource_id == resource_id_text)
            .and_then(|record| record.harness_name)
            .ok_or_else(|| {
                RuntimeError::new(
                    CliErrorCode::LocalState,
                    "import completed without a harness projection",
                )
                .recovery("denju sync")
            })?,
    };
    Ok(ImportOutcome {
        state: "imported",
        resource_id: resource_id_text,
        locator: locator_text,
        revision_id: revision_id_text,
        harness_name,
    })
}

async fn create_import_journal(
    context: &crate::public::InstalledContext,
    source: &Path,
    source_text: String,
) -> Result<ImportJournal, RuntimeError> {
    let entries = read_skill_source(source).map_err(source_error)?;
    let skill_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            RuntimeError::new(
                CliErrorCode::InvalidArguments,
                "skill source directory name must be UTF-8",
            )
        })?
        .to_owned();
    let snapshot = build_deterministic_skill_snapshot(&skill_name, &entries)
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error.to_string()))?;
    let snapshot_size = u64::try_from(snapshot.bytes().len()).map_err(|_| {
        RuntimeError::new(
            CliErrorCode::ContentVerification,
            "skill snapshot is too large",
        )
    })?;
    if snapshot_size > context.limits.max_release_bytes {
        return Err(RuntimeError::new(
            CliErrorCode::ContentVerification,
            "skill exceeds the registry release-size limit",
        ));
    }
    if snapshot_size > context.limits.max_transfer_bytes {
        return Err(RuntimeError::new(
            CliErrorCode::ContentVerification,
            "skill exceeds the registry transfer limit",
        ));
    }
    for entry in &entries {
        if let denju_core::OwnedSkillEntry::File { bytes, .. } = entry
            && u64::try_from(bytes.len()).unwrap_or(u64::MAX) > context.limits.max_object_bytes
        {
            return Err(RuntimeError::new(
                CliErrorCode::ContentVerification,
                "skill contains a file above the registry object-size limit",
            ));
        }
    }

    let operation_id = OperationId::from_uuid(Uuid::now_v7())
        .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let manifest = PublicSkillManifest::from_core(snapshot.manifest());
    let snapshot_sha256 = BlobId::hash(snapshot.bytes()).to_string();
    let request_hash = private_skill_import_request_hash(
        &operation_id.to_string(),
        0,
        &skill_name,
        &manifest,
        &snapshot_sha256,
        snapshot_size,
    )
    .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    let snapshot_path = context
        .paths
        .imports
        .join(format!("{operation_id}.tar.zst"));
    persist_staging_snapshot(&snapshot_path, snapshot.bytes()).map_err(local_error)?;
    let payload = ImportJournalPayload {
        source_path: source_text,
        skill_name,
        request_hash: request_hash.to_string(),
        manifest_json: serde_json::to_string(&manifest)
            .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?,
        snapshot_sha256,
        snapshot_size_bytes: snapshot_size,
        snapshot_path: snapshot_path.display().to_string(),
        resource_id: None,
        locator: None,
        revision_id: None,
    };
    context
        .db
        .create_import_journal(operation_id, payload.clone(), now_unix_ms())
        .await
        .map_err(local_error)?;
    Ok(ImportJournal {
        operation_id,
        state: JournalState::Planned,
        payload,
    })
}

fn load_staged_import(
    payload: &ImportJournalPayload,
) -> Result<
    (
        PublicSkillManifest,
        SkillManifest,
        Vec<u8>,
        Vec<denju_core::OwnedSkillEntry>,
    ),
    RuntimeError,
> {
    let manifest_wire: PublicSkillManifest = serde_json::from_str(&payload.manifest_json)
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error.to_string()))?;
    let manifest = manifest_wire
        .to_core()
        .map_err(|error| RuntimeError::new(CliErrorCode::LocalState, error))?;
    let snapshot = fs::read(&payload.snapshot_path).map_err(|error| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            format!("import staging snapshot is unavailable: {error}"),
        )
    })?;
    if u64::try_from(snapshot.len()).ok() != Some(payload.snapshot_size_bytes)
        || BlobId::hash(&snapshot).to_string() != payload.snapshot_sha256
    {
        return Err(RuntimeError::new(
            CliErrorCode::ContentVerification,
            "import staging snapshot failed integrity verification",
        ));
    }
    let entries = validate_skill_snapshot(&payload.skill_name, &manifest, &snapshot)
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error.to_string()))?;
    Ok((manifest_wire, manifest, snapshot, entries))
}

fn import_request(
    journal: &ImportJournal,
    manifest: &PublicSkillManifest,
) -> Result<PrivateSkillImportRequest, RuntimeError> {
    let expected_hash = private_skill_import_request_hash(
        &journal.operation_id.to_string(),
        0,
        &journal.payload.skill_name,
        manifest,
        &journal.payload.snapshot_sha256,
        journal.payload.snapshot_size_bytes,
    )
    .map_err(|error| RuntimeError::new(CliErrorCode::Internal, error.to_string()))?;
    if expected_hash.to_string() != journal.payload.request_hash {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            "import journal request hash no longer matches its immutable intent",
        ));
    }
    Ok(PrivateSkillImportRequest {
        operation_id: journal.operation_id.to_string(),
        expected_generation: 0,
        name: journal.payload.skill_name.clone(),
        manifest: manifest.clone(),
        snapshot_sha256: journal.payload.snapshot_sha256.clone(),
        snapshot_size_bytes: journal.payload.snapshot_size_bytes,
        request_hash: journal.payload.request_hash.clone(),
    })
}

fn file_bytes_by_blob(entries: &[denju_core::OwnedSkillEntry]) -> BTreeMap<BlobId, &[u8]> {
    let mut result = BTreeMap::new();
    for entry in entries {
        if let denju_core::OwnedSkillEntry::File { bytes, .. } = entry {
            result
                .entry(BlobId::hash(bytes))
                .or_insert(bytes.as_slice());
        }
    }
    result
}

fn verify_source_unchanged(
    payload: &ImportJournalPayload,
    expected: &SkillManifest,
) -> Result<(), RuntimeError> {
    let source = Path::new(&payload.source_path);
    if !source.exists() {
        return Ok(());
    }
    let entries = read_skill_source(source).map_err(source_error)?;
    let actual = build_skill_manifest(&payload.skill_name, &entries)
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error.to_string()))?;
    if &actual != expected {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            "source changed after import began; refusing to remove user content",
        )
        .recovery("move or restore the source, then rerun the same denju import command"));
    }
    Ok(())
}

fn verify_or_record(
    slot: &mut Option<String>,
    value: &str,
    label: &str,
) -> Result<(), RuntimeError> {
    match slot {
        Some(existing) if existing != value => Err(RuntimeError::new(
            CliErrorCode::LocalState,
            format!("registry changed the import {label} across an idempotent retry"),
        )),
        Some(_) => Ok(()),
        None => {
            *slot = Some(value.to_owned());
            Ok(())
        }
    }
}

fn verify_committed_identity(
    payload: &ImportJournalPayload,
    resource_id: &str,
    locator: &str,
    revision_id: &str,
) -> Result<(), RuntimeError> {
    for (stored, actual, label) in [
        (payload.resource_id.as_deref(), resource_id, "resource ID"),
        (payload.locator.as_deref(), locator, "locator"),
        (payload.revision_id.as_deref(), revision_id, "revision ID"),
    ] {
        if stored != Some(actual) {
            return Err(RuntimeError::new(
                CliErrorCode::LocalState,
                format!("committed import returned a different {label}"),
            ));
        }
    }
    Ok(())
}

fn required_payload<'a>(slot: &'a Option<String>, label: &str) -> Result<&'a str, RuntimeError> {
    slot.as_deref().ok_or_else(|| {
        RuntimeError::new(
            CliErrorCode::LocalState,
            format!("import journal is missing its {label}"),
        )
    })
}

fn projection_name(projections: &[(String, String)], locator: &str) -> Option<String> {
    projections
        .iter()
        .find(|(projected_locator, _)| projected_locator == locator)
        .map(|(_, name)| name.clone())
}

fn persist_staging_snapshot(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("snapshot path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".import-{}.tmp", Uuid::now_v7()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn absolute_candidate(path: &Path) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn source_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::ContentVerification, error.to_string())
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}
