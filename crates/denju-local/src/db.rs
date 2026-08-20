use std::{
    path::Path,
    sync::mpsc::{self, Sender},
    thread,
};

use denju_core::OperationId;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::oneshot;

const MIGRATION_V1: &str = r#"
CREATE TABLE IF NOT EXISTS installation (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    registry_origin TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    author_principal_id TEXT NOT NULL,
    credential_backend TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS harness_config (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    codex_root TEXT NOT NULL,
    claude_root TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS operation_journal (
    operation_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('planned', 'staged', 'verified', 'switched', 'complete')),
    payload_json TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS service_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    kind TEXT NOT NULL,
    persistent INTEGER NOT NULL CHECK (persistent IN (0, 1)),
    running INTEGER NOT NULL CHECK (running IN (0, 1)),
    detail TEXT
);

CREATE TABLE IF NOT EXISTS work_leases (
    resource_key TEXT PRIMARY KEY,
    holder TEXT NOT NULL,
    expires_at_unix_ms INTEGER NOT NULL
);

PRAGMA user_version = 1;
"#;

type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

#[derive(Clone)]
pub struct LocalDatabase {
    sender: Sender<Job>,
}

impl LocalDatabase {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, LocalDbError> {
        let path = path.as_ref().to_owned();
        let (sender, receiver) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = oneshot::channel();
        thread::Builder::new()
            .name("denju-sqlite".to_owned())
            .spawn(move || {
                let connection = open_connection(&path);
                match connection {
                    Ok(mut connection) => {
                        let _ = ready_tx.send(Ok(()));
                        while let Ok(job) = receiver.recv() {
                            job(&mut connection);
                        }
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                    }
                }
            })
            .map_err(LocalDbError::WorkerStart)?;
        ready_rx.await.map_err(|_| LocalDbError::WorkerStopped)??;
        Ok(Self { sender })
    }

    pub async fn installation(&self) -> Result<Option<InstallationRecord>, LocalDbError> {
        self.call(|connection| {
            Ok(connection
                .query_row(
                    "SELECT registry_origin, installation_id, author_principal_id, \
                     credential_backend, created_at_unix_ms FROM installation WHERE singleton = 1",
                    [],
                    |row| {
                        Ok(InstallationRecord {
                            registry_origin: row.get(0)?,
                            installation_id: row.get(1)?,
                            author_principal_id: row.get(2)?,
                            credential_backend: row.get(3)?,
                            created_at_unix_ms: row.get(4)?,
                        })
                    },
                )
                .optional()?)
        })
        .await
    }

    pub async fn save_installation(&self, record: InstallationRecord) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO installation \
                 (singleton, registry_origin, installation_id, author_principal_id, credential_backend, created_at_unix_ms) \
                 VALUES (1, ?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(singleton) DO UPDATE SET \
                 registry_origin=excluded.registry_origin, installation_id=excluded.installation_id, \
                 author_principal_id=excluded.author_principal_id, credential_backend=excluded.credential_backend",
                params![
                    record.registry_origin,
                    record.installation_id,
                    record.author_principal_id,
                    record.credential_backend,
                    record.created_at_unix_ms,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn harness_config(&self) -> Result<Option<HarnessConfig>, LocalDbError> {
        self.call(|connection| {
            Ok(connection
                .query_row(
                    "SELECT codex_root, claude_root FROM harness_config WHERE singleton = 1",
                    [],
                    |row| {
                        Ok(HarnessConfig {
                            codex_root: row.get(0)?,
                            claude_root: row.get(1)?,
                        })
                    },
                )
                .optional()?)
        })
        .await
    }

    pub async fn save_harness_config(&self, config: HarnessConfig) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO harness_config (singleton, codex_root, claude_root) VALUES (1, ?1, ?2) \
                 ON CONFLICT(singleton) DO UPDATE SET codex_root=excluded.codex_root, claude_root=excluded.claude_root",
                params![config.codex_root, config.claude_root],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn bootstrap_journal(&self) -> Result<Option<BootstrapJournal>, LocalDbError> {
        self.call(|connection| {
            let row = connection
                .query_row(
                    "SELECT operation_id, state, payload_json FROM operation_journal \
                     WHERE kind = 'setup_bootstrap' AND state != 'complete' \
                     ORDER BY created_at_unix_ms DESC LIMIT 1",
                    [],
                    |row| {
                        let operation_id: String = row.get(0)?;
                        let state: String = row.get(1)?;
                        let payload: String = row.get(2)?;
                        Ok((operation_id, state, payload))
                    },
                )
                .optional()?;
            row.map(|(operation_id, state, payload)| {
                Ok(BootstrapJournal {
                    operation_id: operation_id.parse().map_err(|error: denju_core::IdError| {
                        LocalDbError::Corrupt(error.to_string())
                    })?,
                    state: state.parse()?,
                    payload: serde_json::from_str(&payload)?,
                })
            })
            .transpose()
        })
        .await
    }

    pub async fn create_bootstrap_journal(
        &self,
        operation_id: OperationId,
        payload: BootstrapJournalPayload,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        let payload = serde_json::to_string(&payload)?;
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO operation_journal \
                 (operation_id, kind, state, payload_json, created_at_unix_ms, updated_at_unix_ms) \
                 VALUES (?1, 'setup_bootstrap', 'planned', ?2, ?3, ?3)",
                params![operation_id.to_string(), payload, now_unix_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn discard_planned_bootstrap(
        &self,
        operation_id: OperationId,
    ) -> Result<bool, LocalDbError> {
        self.call(move |connection| {
            let changed = connection.execute(
                "DELETE FROM operation_journal \
                 WHERE operation_id=?1 AND kind='setup_bootstrap' AND state='planned'",
                params![operation_id.to_string()],
            )?;
            Ok(changed == 1)
        })
        .await
    }

    pub async fn update_bootstrap(
        &self,
        operation_id: OperationId,
        expected: JournalState,
        next: JournalState,
        payload: BootstrapJournalPayload,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        if expected.next() != Some(next) {
            return Err(LocalDbError::InvalidJournalTransition { expected, next });
        }
        let payload = serde_json::to_string(&payload)?;
        self.call(move |connection| {
            let changed = connection.execute(
                "UPDATE operation_journal SET state=?1, payload_json=?2, updated_at_unix_ms=?3 \
                 WHERE operation_id=?4 AND state=?5",
                params![
                    next.as_str(),
                    payload,
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

    pub async fn save_service(&self, service: ServiceRecord) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO service_state (singleton, kind, persistent, running, detail) \
                 VALUES (1, ?1, ?2, ?3, ?4) ON CONFLICT(singleton) DO UPDATE SET \
                 kind=excluded.kind, persistent=excluded.persistent, running=excluded.running, detail=excluded.detail",
                params![
                    service.kind,
                    if service.persistent { 1_i64 } else { 0_i64 },
                    if service.running { 1_i64 } else { 0_i64 },
                    service.detail,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn service(&self) -> Result<Option<ServiceRecord>, LocalDbError> {
        self.call(|connection| {
            Ok(connection
                .query_row(
                    "SELECT kind, persistent, running, detail FROM service_state WHERE singleton=1",
                    [],
                    |row| {
                        Ok(ServiceRecord {
                            kind: row.get(0)?,
                            persistent: row.get::<_, i64>(1)? != 0,
                            running: row.get::<_, i64>(2)? != 0,
                            detail: row.get(3)?,
                        })
                    },
                )
                .optional()?)
        })
        .await
    }

    pub async fn incomplete_operations(&self) -> Result<Vec<(String, JournalState)>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT operation_id, state FROM operation_journal WHERE state != 'complete' ORDER BY created_at_unix_ms",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut result = Vec::new();
            for row in rows {
                let (operation_id, state) = row?;
                result.push((operation_id, state.parse()?));
            }
            Ok(result)
        })
        .await
    }

    pub async fn quick_check(&self) -> Result<(), LocalDbError> {
        self.call(|connection| {
            let result: String =
                connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
            if result == "ok" {
                Ok(())
            } else {
                Err(rusqlite::Error::InvalidQuery.into())
            }
        })
        .await
    }

    pub async fn claim_lease(
        &self,
        resource_key: String,
        holder: String,
        now_unix_ms: i64,
        ttl_ms: i64,
    ) -> Result<bool, LocalDbError> {
        if ttl_ms <= 0 {
            return Err(LocalDbError::InvalidLeaseTtl(ttl_ms));
        }
        self.call(move |connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "DELETE FROM work_leases WHERE resource_key=?1 AND expires_at_unix_ms <= ?2",
                params![resource_key, now_unix_ms],
            )?;
            let changed = tx.execute(
                "INSERT INTO work_leases (resource_key, holder, expires_at_unix_ms) \
                 VALUES (?1, ?2, ?3) ON CONFLICT(resource_key) DO NOTHING",
                params![resource_key, holder, now_unix_ms.saturating_add(ttl_ms)],
            )?;
            tx.commit()?;
            Ok(changed == 1)
        })
        .await
    }

    pub async fn release_lease(
        &self,
        resource_key: String,
        holder: String,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM work_leases WHERE resource_key=?1 AND holder=?2",
                params![resource_key, holder],
            )?;
            Ok(())
        })
        .await
    }

    async fn call<R, F>(&self, operation: F) -> Result<R, LocalDbError>
    where
        R: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<R, LocalDbError> + Send + 'static,
    {
        let (result_tx, result_rx) = oneshot::channel();
        self.sender
            .send(Box::new(move |connection| {
                let _ = result_tx.send(operation(connection));
            }))
            .map_err(|_| LocalDbError::WorkerStopped)?;
        result_rx.await.map_err(|_| LocalDbError::WorkerStopped)?
    }
}

fn open_connection(path: &Path) -> Result<Connection, LocalDbError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
    )?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > 1 {
        return Err(LocalDbError::UnsupportedSchema(version));
    }
    if version == 0 {
        connection.execute_batch(MIGRATION_V1)?;
    }
    Ok(connection)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationRecord {
    pub registry_origin: String,
    pub installation_id: String,
    pub author_principal_id: String,
    pub credential_backend: String,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessConfig {
    pub codex_root: String,
    pub claude_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRecord {
    pub kind: String,
    pub persistent: bool,
    pub running: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapJournal {
    pub operation_id: OperationId,
    pub state: JournalState,
    pub payload: BootstrapJournalPayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapJournalPayload {
    pub registry_origin: String,
    pub credential_hash: String,
    pub credential_backend: Option<String>,
    pub installation_id: Option<String>,
    pub author_principal_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalState {
    Planned,
    Staged,
    Verified,
    Switched,
    Complete,
}

impl JournalState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Staged => "staged",
            Self::Verified => "verified",
            Self::Switched => "switched",
            Self::Complete => "complete",
        }
    }

    pub const fn next(self) -> Option<Self> {
        match self {
            Self::Planned => Some(Self::Staged),
            Self::Staged => Some(Self::Verified),
            Self::Verified => Some(Self::Switched),
            Self::Switched => Some(Self::Complete),
            Self::Complete => None,
        }
    }
}

impl std::str::FromStr for JournalState {
    type Err = LocalDbError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "planned" => Ok(Self::Planned),
            "staged" => Ok(Self::Staged),
            "verified" => Ok(Self::Verified),
            "switched" => Ok(Self::Switched),
            "complete" => Ok(Self::Complete),
            other => Err(LocalDbError::Corrupt(format!(
                "unknown journal state {other}"
            ))),
        }
    }
}

#[derive(Debug, Error)]
pub enum LocalDbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("local state serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("failed to start SQLite worker: {0}")]
    WorkerStart(std::io::Error),
    #[error("SQLite worker stopped unexpectedly")]
    WorkerStopped,
    #[error("local database schema {0} is newer than this Denju binary")]
    UnsupportedSchema(i64),
    #[error("corrupt local state: {0}")]
    Corrupt(String),
    #[error("invalid journal transition {expected:?} -> {next:?}")]
    InvalidJournalTransition {
        expected: JournalState,
        next: JournalState,
    },
    #[error("lease TTL must be positive, got {0}ms")]
    InvalidLeaseTtl(i64),
}

#[cfg(test)]
mod tests {
    use denju_core::OperationId;
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::*;

    #[tokio::test]
    async fn sqlite_worker_uses_wal_and_persists_journal() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let db = LocalDatabase::open(&path).await.unwrap();
        let operation_id = OperationId::from_uuid(Uuid::now_v7()).unwrap();
        db.create_bootstrap_journal(
            operation_id,
            BootstrapJournalPayload {
                registry_origin: "http://127.0.0.1:7788".to_owned(),
                credential_hash: "00".repeat(32),
                credential_backend: None,
                installation_id: None,
                author_principal_id: None,
            },
            1,
        )
        .await
        .unwrap();
        assert_eq!(
            db.bootstrap_journal().await.unwrap().unwrap().state,
            JournalState::Planned
        );
        db.quick_check().await.unwrap();

        let mode: String = db
            .call(|connection| {
                connection
                    .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                    .map_err(LocalDbError::from)
            })
            .await
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
    }

    #[tokio::test]
    async fn leases_expire_and_are_holder_scoped() {
        let dir = tempdir().unwrap();
        let db = LocalDatabase::open(dir.path().join("state.db"))
            .await
            .unwrap();
        assert!(
            db.claim_lease("skill:a".into(), "cli".into(), 100, 50)
                .await
                .unwrap()
        );
        assert!(
            !db.claim_lease("skill:a".into(), "daemon".into(), 120, 50)
                .await
                .unwrap()
        );
        db.release_lease("skill:a".into(), "daemon".into())
            .await
            .unwrap();
        assert!(
            !db.claim_lease("skill:a".into(), "daemon".into(), 120, 50)
                .await
                .unwrap()
        );
        assert!(
            db.claim_lease("skill:a".into(), "daemon".into(), 151, 50)
                .await
                .unwrap()
        );
    }
}
