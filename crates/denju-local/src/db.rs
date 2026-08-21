use std::{
    path::Path,
    sync::mpsc::{self, Sender},
    thread,
};

use denju_core::OperationId;
use rusqlite::{Connection, OptionalExtension, params};
use tokio::sync::oneshot;

mod schema;
mod types;

use schema::*;
pub use types::*;

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

    pub async fn clear_installation(&self) -> Result<(), LocalDbError> {
        self.call(|connection| {
            connection.execute("DELETE FROM installation WHERE singleton=1", [])?;
            Ok(())
        })
        .await
    }

    pub async fn identity(&self) -> Result<Option<IdentityRecord>, LocalDbError> {
        self.call(|connection| {
            Ok(connection
                .query_row(
                    "SELECT user_id, namespace_id, username, session_id, session_backend, author_principal_id \
                     FROM identity_state WHERE singleton=1",
                    [],
                    |row| {
                        Ok(IdentityRecord {
                            user_id: row.get(0)?,
                            namespace_id: row.get(1)?,
                            username: row.get(2)?,
                            session_id: row.get(3)?,
                            session_backend: row.get(4)?,
                            author_principal_id: row.get(5)?,
                        })
                    },
                )
                .optional()?)
        })
        .await
    }

    pub async fn save_identity(
        &self,
        identity: IdentityRecord,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO identity_state \
                 (singleton,user_id,namespace_id,username,session_id,session_backend,author_principal_id,updated_at_unix_ms) \
                 VALUES (1,?1,?2,?3,?4,?5,?6,?7) \
                 ON CONFLICT(singleton) DO UPDATE SET \
                   user_id=excluded.user_id, namespace_id=excluded.namespace_id, username=excluded.username, \
                   session_id=excluded.session_id, session_backend=excluded.session_backend, author_principal_id=excluded.author_principal_id, \
                   updated_at_unix_ms=excluded.updated_at_unix_ms",
                params![
                    identity.user_id,
                    identity.namespace_id,
                    identity.username,
                    identity.session_id,
                    identity.session_backend,
                    identity.author_principal_id,
                    now_unix_ms,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn clear_identity(&self) -> Result<(), LocalDbError> {
        self.call(|connection| {
            connection.execute("DELETE FROM identity_state WHERE singleton=1", [])?;
            Ok(())
        })
        .await
    }

    pub async fn clear_identity_session(&self, now_unix_ms: i64) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "UPDATE identity_state SET session_id=NULL,session_backend=NULL,updated_at_unix_ms=?1 WHERE singleton=1",
                params![now_unix_ms],
            )?;
            Ok(())
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

    pub async fn account_delete_journal(
        &self,
    ) -> Result<Option<AccountDeleteJournal>, LocalDbError> {
        self.call(|connection| {
            let row = connection
                .query_row(
                    "SELECT operation_id,state,payload_json FROM operation_journal \
                     WHERE kind='account_delete_local' ORDER BY created_at_unix_ms DESC LIMIT 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            row.map(|(operation_id, state, payload)| {
                Ok(AccountDeleteJournal {
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

    pub async fn create_account_delete_journal(
        &self,
        operation_id: OperationId,
        payload: AccountDeleteJournalPayload,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        let payload = serde_json::to_string(&payload)?;
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO operation_journal \
                 (operation_id,kind,state,payload_json,created_at_unix_ms,updated_at_unix_ms) \
                 VALUES (?1,'account_delete_local','planned',?2,?3,?3)",
                params![operation_id.to_string(), payload, now_unix_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn advance_account_delete_journal(
        &self,
        operation_id: OperationId,
        expected: JournalState,
        next: JournalState,
        payload: AccountDeleteJournalPayload,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        if expected.next() != Some(next) {
            return Err(LocalDbError::InvalidJournalTransition { expected, next });
        }
        let payload = serde_json::to_string(&payload)?;
        self.call(move |connection| {
            let changed = connection.execute(
                "UPDATE operation_journal SET state=?1,payload_json=?2,updated_at_unix_ms=?3 \
                 WHERE operation_id=?4 AND kind='account_delete_local' AND state=?5",
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

    pub async fn finish_account_delete_journal(
        &self,
        operation_id: OperationId,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            let changed = connection.execute(
                "DELETE FROM operation_journal WHERE operation_id=?1 AND kind='account_delete_local' AND state='switched'",
                params![operation_id.to_string()],
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

    pub async fn subscriptions(&self) -> Result<Vec<SubscriptionRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT resource_id, locator, owner, skill_name, resource_generation, release_version, \
                        desired_revision_id, harness_name, materialized_revision_id, retain_on_delete, retained_after_delete \
                 FROM subscriptions ORDER BY owner, skill_name, resource_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(SubscriptionRecord {
                    resource_id: row.get(0)?,
                    locator: row.get(1)?,
                    owner: row.get(2)?,
                    skill_name: row.get(3)?,
                    resource_generation: row.get::<_, i64>(4)?,
                    release_version: row.get::<_, i64>(5)?,
                    desired_revision_id: row.get(6)?,
                    harness_name: row.get(7)?,
                    materialized_revision_id: row.get(8)?,
                    retain_on_delete: row.get::<_, i64>(9)? != 0,
                    retained_after_delete: row.get::<_, i64>(10)? != 0,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn owned_skills(&self) -> Result<Vec<OwnedSkillRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT resource_id, locator, owner, skill_name, resource_generation, desired_revision_id, \
                        harness_name, materialized_revision_id \
                 FROM owned_skills ORDER BY owner, skill_name, resource_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(OwnedSkillRecord {
                    resource_id: row.get(0)?,
                    locator: row.get(1)?,
                    owner: row.get(2)?,
                    skill_name: row.get(3)?,
                    resource_generation: row.get(4)?,
                    desired_revision_id: row.get(5)?,
                    harness_name: row.get(6)?,
                    materialized_revision_id: row.get(7)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn managed_skills(&self) -> Result<Vec<ManagedSkillRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT resource_id, locator, owner, skill_name, harness_name, materialized_revision_id \
                 FROM subscriptions \
                 UNION ALL \
                 SELECT resource_id, locator, owner, skill_name, harness_name, materialized_revision_id \
                 FROM owned_skills \
                 ORDER BY owner, skill_name, resource_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(ManagedSkillRecord {
                    resource_id: row.get(0)?,
                    locator: row.get(1)?,
                    owner: row.get(2)?,
                    skill_name: row.get(3)?,
                    harness_name: row.get(4)?,
                    materialized_revision_id: row.get(5)?,
                })
            })?;
            let records = rows.collect::<Result<Vec<_>, _>>()?;
            for pair in records.windows(2) {
                if pair[0].resource_id == pair[1].resource_id {
                    return Err(LocalDbError::Corrupt(format!(
                        "resource {} has multiple local desired-state owners",
                        pair[0].resource_id
                    )));
                }
            }
            Ok(records)
        })
        .await
    }

    pub async fn upsert_owned_skill_desired(
        &self,
        record: OwnedSkillRecord,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO owned_skills \
                 (resource_id, locator, owner, skill_name, resource_generation, desired_revision_id, updated_at_unix_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(resource_id) DO UPDATE SET \
                   locator=excluded.locator, owner=excluded.owner, skill_name=excluded.skill_name, \
                   resource_generation=excluded.resource_generation, desired_revision_id=excluded.desired_revision_id, \
                   updated_at_unix_ms=excluded.updated_at_unix_ms",
                params![
                    record.resource_id,
                    record.locator,
                    record.owner,
                    record.skill_name,
                    record.resource_generation,
                    record.desired_revision_id,
                    now_unix_ms,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn remove_owned_skill(&self, resource_id: String) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM owned_skills WHERE resource_id=?1",
                params![resource_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn subscription(
        &self,
        resource_id: String,
    ) -> Result<Option<SubscriptionRecord>, LocalDbError> {
        self.call(move |connection| {
            Ok(connection
                .query_row(
                    "SELECT resource_id, locator, owner, skill_name, resource_generation, release_version, \
                            desired_revision_id, harness_name, materialized_revision_id, retain_on_delete, retained_after_delete \
                     FROM subscriptions WHERE resource_id=?1",
                    params![resource_id],
                    |row| {
                        Ok(SubscriptionRecord {
                            resource_id: row.get(0)?,
                            locator: row.get(1)?,
                            owner: row.get(2)?,
                            skill_name: row.get(3)?,
                            resource_generation: row.get(4)?,
                            release_version: row.get(5)?,
                            desired_revision_id: row.get(6)?,
                            harness_name: row.get(7)?,
                            materialized_revision_id: row.get(8)?,
                            retain_on_delete: row.get::<_, i64>(9)? != 0,
                            retained_after_delete: row.get::<_, i64>(10)? != 0,
                        })
                    },
                )
                .optional()?)
        })
        .await
    }

    pub async fn upsert_subscription_desired(
        &self,
        record: SubscriptionRecord,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO subscriptions \
                 (resource_id, locator, owner, skill_name, resource_generation, release_version, desired_revision_id, retain_on_delete, retained_after_delete, updated_at_unix_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
                 ON CONFLICT(resource_id) DO UPDATE SET \
                   locator=excluded.locator, owner=excluded.owner, skill_name=excluded.skill_name, \
                   resource_generation=excluded.resource_generation, release_version=excluded.release_version, \
                   desired_revision_id=excluded.desired_revision_id, retain_on_delete=excluded.retain_on_delete, \
                   retained_after_delete=excluded.retained_after_delete, updated_at_unix_ms=excluded.updated_at_unix_ms",
                params![
                    record.resource_id,
                    record.locator,
                    record.owner,
                    record.skill_name,
                    record.resource_generation,
                    record.release_version,
                    record.desired_revision_id,
                    i64::from(record.retain_on_delete),
                    i64::from(record.retained_after_delete),
                    now_unix_ms,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn mark_skill_materialized(
        &self,
        resource_id: String,
        revision_id: String,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            let subscription_changed = connection.execute(
                "UPDATE subscriptions SET materialized_revision_id=?1, updated_at_unix_ms=?2 WHERE resource_id=?3",
                params![revision_id, now_unix_ms, resource_id],
            )?;
            let owned_changed = connection.execute(
                "UPDATE owned_skills SET materialized_revision_id=?1, updated_at_unix_ms=?2 WHERE resource_id=?3",
                params![revision_id, now_unix_ms, resource_id],
            )?;
            if subscription_changed + owned_changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows.into());
            }
            Ok(())
        })
        .await
    }

    pub async fn set_managed_harness_name(
        &self,
        resource_id: String,
        harness_name: String,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            let subscription_changed = connection.execute(
                "UPDATE subscriptions SET harness_name=?1, updated_at_unix_ms=?2 WHERE resource_id=?3",
                params![harness_name, now_unix_ms, resource_id],
            )?;
            let owned_changed = connection.execute(
                "UPDATE owned_skills SET harness_name=?1, updated_at_unix_ms=?2 WHERE resource_id=?3",
                params![harness_name, now_unix_ms, resource_id],
            )?;
            if subscription_changed + owned_changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows.into());
            }
            Ok(())
        })
        .await
    }

    pub async fn import_journal_for_source(
        &self,
        source_path: String,
    ) -> Result<Option<ImportJournal>, LocalDbError> {
        self.call(move |connection| {
            let mut statement = connection.prepare(
                "SELECT operation_id, state, payload_json FROM operation_journal \
                 WHERE kind='import_skill' AND state != 'complete' ORDER BY created_at_unix_ms",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (operation_id, state, payload_json) = row?;
                let payload: ImportJournalPayload = serde_json::from_str(&payload_json)?;
                if payload.source_path == source_path {
                    return Ok(Some(ImportJournal {
                        operation_id: operation_id.parse().map_err(
                            |error: denju_core::IdError| LocalDbError::Corrupt(error.to_string()),
                        )?,
                        state: state.parse()?,
                        payload,
                    }));
                }
            }
            Ok(None)
        })
        .await
    }

    pub async fn create_import_journal(
        &self,
        operation_id: OperationId,
        payload: ImportJournalPayload,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        let payload = serde_json::to_string(&payload)?;
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO operation_journal \
                 (operation_id, kind, state, payload_json, created_at_unix_ms, updated_at_unix_ms) \
                 VALUES (?1, 'import_skill', 'planned', ?2, ?3, ?3)",
                params![operation_id.to_string(), payload, now_unix_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn update_import_journal(
        &self,
        operation_id: OperationId,
        expected: JournalState,
        next: JournalState,
        payload: ImportJournalPayload,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        if expected.next() != Some(next) {
            return Err(LocalDbError::InvalidJournalTransition { expected, next });
        }
        let payload = serde_json::to_string(&payload)?;
        self.call(move |connection| {
            let changed = connection.execute(
                "UPDATE operation_journal SET state=?1,payload_json=?2,updated_at_unix_ms=?3 \
                 WHERE operation_id=?4 AND kind='import_skill' AND state=?5",
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

    pub async fn remove_subscription(&self, resource_id: String) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM subscriptions WHERE resource_id=?1",
                params![resource_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn clear_subscriptions(&self) -> Result<(), LocalDbError> {
        self.call(|connection| {
            connection.execute("DELETE FROM subscriptions", [])?;
            Ok(())
        })
        .await
    }

    pub async fn materialization_journals(
        &self,
    ) -> Result<Vec<MaterializationJournal>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT operation_id, state, payload_json FROM operation_journal \
                 WHERE kind='materialize_skill' AND state != 'complete' ORDER BY created_at_unix_ms",
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
                journals.push(MaterializationJournal {
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

    pub async fn create_materialization_journal(
        &self,
        operation_id: OperationId,
        payload: MaterializationJournalPayload,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        let payload = serde_json::to_string(&payload)?;
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO operation_journal \
                 (operation_id, kind, state, payload_json, created_at_unix_ms, updated_at_unix_ms) \
                 VALUES (?1, 'materialize_skill', 'planned', ?2, ?3, ?3)",
                params![operation_id.to_string(), payload, now_unix_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn update_materialization_journal(
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
                "UPDATE operation_journal SET state=?1, updated_at_unix_ms=?2 \
                 WHERE operation_id=?3 AND kind='materialize_skill' AND state=?4",
                params![
                    next.as_str(),
                    now_unix_ms,
                    operation_id.to_string(),
                    expected.as_str()
                ],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows.into());
            }
            Ok(())
        })
        .await
    }

    pub async fn discard_materialization_journal(
        &self,
        operation_id: OperationId,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM operation_journal WHERE operation_id=?1 AND kind='materialize_skill' AND state IN ('planned','staged')",
                params![operation_id.to_string()],
            )?;
            Ok(())
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

    pub(crate) async fn call<R, F>(&self, operation: F) -> Result<R, LocalDbError>
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
    if version > 7 {
        return Err(LocalDbError::UnsupportedSchema(version));
    }
    if version == 0 {
        connection.execute_batch(MIGRATION_V1)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 1 {
        connection.execute_batch(MIGRATION_V2)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 2 {
        connection.execute_batch(MIGRATION_V3)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 3 {
        connection.execute_batch(MIGRATION_V4)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 4 {
        connection.execute_batch(MIGRATION_V5)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 5 {
        connection.execute_batch(MIGRATION_V6)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 6 {
        connection.execute_batch(MIGRATION_V7)?;
    }
    Ok(connection)
}

#[cfg(test)]
mod tests;
