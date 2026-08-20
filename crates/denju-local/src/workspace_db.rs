use denju_core::OperationId;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{JournalState, LocalDatabase, LocalDbError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Clean,
    Queued,
    PausedValidation,
    PendingRename,
    Conflict,
    Quota,
}

impl WorkspaceStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Queued => "queued",
            Self::PausedValidation => "paused_validation",
            Self::PendingRename => "pending_rename",
            Self::Conflict => "conflict",
            Self::Quota => "quota",
        }
    }
}

impl std::str::FromStr for WorkspaceStatus {
    type Err = LocalDbError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "clean" => Ok(Self::Clean),
            "queued" => Ok(Self::Queued),
            "paused_validation" => Ok(Self::PausedValidation),
            "pending_rename" => Ok(Self::PendingRename),
            "conflict" => Ok(Self::Conflict),
            "quota" => Ok(Self::Quota),
            other => Err(LocalDbError::Corrupt(format!(
                "unknown workspace status {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceStateRecord {
    pub resource_id: String,
    pub base_generation: i64,
    pub base_revision_id: String,
    pub local_head_revision_id: String,
    pub valid_root_tree_id: String,
    pub working_generation_path: String,
    pub status: WorkspaceStatus,
    pub error_message: Option<String>,
    pub pending_rename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedProjectionStateRecord {
    pub resource_id: String,
    pub harness_name: String,
    pub baseline_root_tree_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWritebackJournal {
    pub operation_id: OperationId,
    pub state: JournalState,
    pub payload: WorkspaceWritebackJournalPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceWritebackJournalPayload {
    pub resource_id: String,
    pub skill_name: String,
    pub harness_name: String,
    pub target_root_tree_id: String,
    pub stage_dir: String,
    pub generation_dir: String,
    pub canonical_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFileRecord {
    pub resource_id: String,
    pub path: String,
    pub kind: String,
    pub size_bytes: Option<i64>,
    pub mtime_ns: Option<i64>,
    pub executable: Option<bool>,
    pub blob_id: Option<String>,
    pub symlink_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRevisionRecord {
    pub operation_id: String,
    pub resource_id: String,
    pub revision_id: String,
    pub parent_revision_id: String,
    pub expected_generation: i64,
    pub root_tree_id: String,
    pub manifest_json: String,
    pub state: String,
}

impl LocalDatabase {
    pub async fn workspace_writeback_journals(
        &self,
    ) -> Result<Vec<WorkspaceWritebackJournal>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT operation_id,state,payload_json FROM operation_journal \
                 WHERE kind='workspace_writeback' AND state != 'complete' ORDER BY created_at_unix_ms",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            let mut journals = Vec::new();
            for row in rows {
                let (operation_id, state, payload) = row?;
                journals.push(WorkspaceWritebackJournal {
                    operation_id: operation_id.parse().map_err(|error: denju_core::IdError| {
                        LocalDbError::Corrupt(error.to_string())
                    })?,
                    state: state.parse()?,
                    payload: serde_json::from_str(&payload)?,
                });
            }
            Ok(journals)
        })
        .await
    }

    pub async fn create_workspace_writeback_journal(
        &self,
        operation_id: OperationId,
        payload: WorkspaceWritebackJournalPayload,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        let payload = serde_json::to_string(&payload)?;
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO operation_journal \
                 (operation_id,kind,state,payload_json,created_at_unix_ms,updated_at_unix_ms) \
                 VALUES (?1,'workspace_writeback','planned',?2,?3,?3)",
                params![operation_id.to_string(), payload, now_unix_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn update_workspace_writeback_journal(
        &self,
        operation_id: OperationId,
        expected: JournalState,
        next: JournalState,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        if expected.next() != Some(next) {
            return Err(LocalDbError::InvalidJournalTransition { expected, next });
        }
        self.call(move |connection| {
            let changed = connection.execute(
                "UPDATE operation_journal SET state=?1,updated_at_unix_ms=?2 \
                 WHERE operation_id=?3 AND kind='workspace_writeback' AND state=?4",
                params![
                    next.as_str(),
                    now_unix_ms,
                    operation_id.to_string(),
                    expected.as_str(),
                ],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows.into());
            }
            Ok(())
        })
        .await
    }

    pub async fn discard_workspace_writeback_journal(
        &self,
        operation_id: OperationId,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM operation_journal WHERE operation_id=?1 AND kind='workspace_writeback' AND state IN ('planned','staged')",
                params![operation_id.to_string()],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn workspace_state(
        &self,
        resource_id: String,
    ) -> Result<Option<WorkspaceStateRecord>, LocalDbError> {
        self.call(move |connection| {
            let row = connection
                .query_row(
                    "SELECT resource_id,base_generation,base_revision_id,local_head_revision_id,valid_root_tree_id,working_generation_path,status,error_message,pending_rename \
                     FROM workspace_state WHERE resource_id=?1",
                    params![resource_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                        ))
                    },
                )
                .optional()?;
            row.map(|row| {
                Ok(WorkspaceStateRecord {
                    resource_id: row.0,
                    base_generation: row.1,
                    base_revision_id: row.2,
                    local_head_revision_id: row.3,
                    valid_root_tree_id: row.4,
                    working_generation_path: row.5,
                    status: row.6.parse()?,
                    error_message: row.7,
                    pending_rename: row.8,
                })
            })
            .transpose()
        })
        .await
    }

    pub async fn workspace_states(&self) -> Result<Vec<WorkspaceStateRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT resource_id,base_generation,base_revision_id,local_head_revision_id,valid_root_tree_id,working_generation_path,status,error_message,pending_rename \
                 FROM workspace_state ORDER BY resource_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })?;
            let mut result = Vec::new();
            for row in rows {
                let row = row?;
                result.push(WorkspaceStateRecord {
                    resource_id: row.0,
                    base_generation: row.1,
                    base_revision_id: row.2,
                    local_head_revision_id: row.3,
                    valid_root_tree_id: row.4,
                    working_generation_path: row.5,
                    status: row.6.parse()?,
                    error_message: row.7,
                    pending_rename: row.8,
                });
            }
            Ok(result)
        })
        .await
    }

    pub async fn ensure_workspace_baseline(
        &self,
        resource_id: String,
        generation: i64,
        revision_id: String,
        root_tree_id: String,
        working_generation_path: String,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO workspace_state \
                 (resource_id,base_generation,base_revision_id,local_head_revision_id,valid_root_tree_id,working_generation_path,status,updated_at_unix_ms) \
                 VALUES (?1,?2,?3,?3,?4,?5,'clean',?6) ON CONFLICT(resource_id) DO NOTHING",
                params![resource_id, generation, revision_id, root_tree_id, working_generation_path, now_unix_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn advance_clean_workspace_baseline(
        &self,
        resource_id: String,
        generation: i64,
        revision_id: String,
        root_tree_id: String,
        working_generation_path: String,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "UPDATE workspace_state SET base_generation=?1,base_revision_id=?2,local_head_revision_id=?2,valid_root_tree_id=?3,working_generation_path=?4, \
                 status='clean',error_message=NULL,pending_rename=NULL,updated_at_unix_ms=?5 \
                 WHERE resource_id=?6 AND status='clean'",
                params![generation, revision_id, root_tree_id, working_generation_path, now_unix_ms, resource_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn pause_workspace(
        &self,
        resource_id: String,
        status: WorkspaceStatus,
        message: String,
        pending_rename: Option<String>,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        if !matches!(
            status,
            WorkspaceStatus::PausedValidation
                | WorkspaceStatus::PendingRename
                | WorkspaceStatus::Conflict
                | WorkspaceStatus::Quota
        ) {
            return Err(LocalDbError::Corrupt(
                "pause_workspace requires a paused status".to_owned(),
            ));
        }
        self.call(move |connection| {
            let changed = connection.execute(
                "UPDATE workspace_state SET status=?1,error_message=?2,pending_rename=?3,updated_at_unix_ms=?4 WHERE resource_id=?5",
                params![status.as_str(), message, pending_rename, now_unix_ms, resource_id],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows.into());
            }
            Ok(())
        })
        .await
    }

    pub async fn resume_workspace(
        &self,
        resource_id: String,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            let queued: i64 = connection.query_row(
                "SELECT count(*) FROM local_revisions WHERE resource_id=?1 AND state='queued'",
                params![resource_id],
                |row| row.get(0),
            )?;
            let status = if queued == 0 { "clean" } else { "queued" };
            let changed = connection.execute(
                "UPDATE workspace_state SET status=?1,error_message=NULL,pending_rename=NULL,updated_at_unix_ms=?2 WHERE resource_id=?3",
                params![status, now_unix_ms, resource_id],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows.into());
            }
            Ok(())
        })
        .await
    }

    pub async fn clear_workspace_file_index(
        &self,
        resource_id: String,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM workspace_file_index WHERE resource_id=?1",
                params![resource_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn set_workspace_working_generation(
        &self,
        resource_id: String,
        working_generation_path: String,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "UPDATE workspace_state SET working_generation_path=?1,updated_at_unix_ms=?2 WHERE resource_id=?3",
                params![working_generation_path, now_unix_ms, resource_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn derived_projection_state(
        &self,
        resource_id: String,
    ) -> Result<Option<DerivedProjectionStateRecord>, LocalDbError> {
        self.call(move |connection| {
            Ok(connection
                .query_row(
                    "SELECT resource_id,harness_name,baseline_root_tree_id FROM derived_projection_state WHERE resource_id=?1",
                    params![resource_id],
                    |row| {
                        Ok(DerivedProjectionStateRecord {
                            resource_id: row.get(0)?,
                            harness_name: row.get(1)?,
                            baseline_root_tree_id: row.get(2)?,
                        })
                    },
                )
                .optional()?)
        })
        .await
    }

    pub async fn save_derived_projection_state(
        &self,
        state: DerivedProjectionStateRecord,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO derived_projection_state (resource_id,harness_name,baseline_root_tree_id,updated_at_unix_ms) \
                 VALUES (?1,?2,?3,?4) ON CONFLICT(resource_id) DO UPDATE SET \
                 harness_name=excluded.harness_name,baseline_root_tree_id=excluded.baseline_root_tree_id,updated_at_unix_ms=excluded.updated_at_unix_ms",
                params![state.resource_id, state.harness_name, state.baseline_root_tree_id, now_unix_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn workspace_file_index(
        &self,
        resource_id: String,
    ) -> Result<Vec<WorkspaceFileRecord>, LocalDbError> {
        self.call(move |connection| {
            let mut statement = connection.prepare(
                "SELECT resource_id,path,kind,size_bytes,mtime_ns,executable,blob_id,symlink_target \
                 FROM workspace_file_index WHERE resource_id=?1 ORDER BY path",
            )?;
            let rows = statement.query_map(params![resource_id], |row| {
                Ok(WorkspaceFileRecord {
                    resource_id: row.get(0)?,
                    path: row.get(1)?,
                    kind: row.get(2)?,
                    size_bytes: row.get(3)?,
                    mtime_ns: row.get(4)?,
                    executable: row.get::<_, Option<i64>>(5)?.map(|value| value != 0),
                    blob_id: row.get(6)?,
                    symlink_target: row.get(7)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn replace_workspace_file_index(
        &self,
        resource_id: String,
        records: Vec<WorkspaceFileRecord>,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "DELETE FROM workspace_file_index WHERE resource_id=?1",
                params![resource_id],
            )?;
            for record in records {
                tx.execute(
                    "INSERT INTO workspace_file_index \
                     (resource_id,path,kind,size_bytes,mtime_ns,executable,blob_id,symlink_target) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        record.resource_id,
                        record.path,
                        record.kind,
                        record.size_bytes,
                        record.mtime_ns,
                        record.executable.map(i64::from),
                        record.blob_id,
                        record.symlink_target,
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn enqueue_local_revision(
        &self,
        revision: LocalRevisionRecord,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "INSERT INTO local_revisions \
                 (operation_id,resource_id,revision_id,parent_revision_id,expected_generation,root_tree_id,manifest_json,state,created_at_unix_ms,updated_at_unix_ms) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,'queued',?8,?8)",
                params![
                    revision.operation_id,
                    revision.resource_id,
                    revision.revision_id,
                    revision.parent_revision_id,
                    revision.expected_generation,
                    revision.root_tree_id,
                    revision.manifest_json,
                    now_unix_ms,
                ],
            )?;
            let changed = tx.execute(
                "UPDATE workspace_state SET local_head_revision_id=?1,valid_root_tree_id=?2,status='queued',error_message=NULL,pending_rename=NULL,updated_at_unix_ms=?3 \
                 WHERE resource_id=?4",
                params![
                    revision.revision_id,
                    revision.root_tree_id,
                    now_unix_ms,
                    revision.resource_id,
                ],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows.into());
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn queued_local_revisions(&self) -> Result<Vec<LocalRevisionRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT operation_id,resource_id,revision_id,parent_revision_id,expected_generation,root_tree_id,manifest_json,state \
                 FROM local_revisions WHERE state='queued' ORDER BY resource_id,expected_generation,created_at_unix_ms",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(LocalRevisionRecord {
                    operation_id: row.get(0)?,
                    resource_id: row.get(1)?,
                    revision_id: row.get(2)?,
                    parent_revision_id: row.get(3)?,
                    expected_generation: row.get(4)?,
                    root_tree_id: row.get(5)?,
                    manifest_json: row.get(6)?,
                    state: row.get(7)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn mark_local_revision_synced(
        &self,
        operation_id: String,
        generation: i64,
        revision_id: String,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            let tx = connection.transaction()?;
            let resource_id: String = tx.query_row(
                "SELECT resource_id FROM local_revisions WHERE operation_id=?1 AND revision_id=?2",
                params![operation_id, revision_id],
                |row| row.get(0),
            )?;
            tx.execute(
                "UPDATE local_revisions SET state='synced',updated_at_unix_ms=?1 WHERE operation_id=?2",
                params![now_unix_ms, operation_id],
            )?;
            tx.execute(
                "UPDATE workspace_state SET base_generation=?1,base_revision_id=?2,updated_at_unix_ms=?3 WHERE resource_id=?4",
                params![generation, revision_id, now_unix_ms, resource_id],
            )?;
            let queued: i64 = tx.query_row(
                "SELECT count(*) FROM local_revisions WHERE resource_id=?1 AND state='queued'",
                params![resource_id],
                |row| row.get(0),
            )?;
            if queued == 0 {
                tx.execute(
                    "UPDATE workspace_state SET status='clean',error_message=NULL,pending_rename=NULL,updated_at_unix_ms=?1 WHERE resource_id=?2",
                    params![now_unix_ms, resource_id],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
