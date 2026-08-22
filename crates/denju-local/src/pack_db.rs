use denju_core::OperationId;
use rusqlite::params;

use crate::{
    LocalDatabase, LocalDbError, PackApplyJournal, PackApplyJournalPayload,
    PackMaterializedSkillRecord, PackSkillSourceRecord, PackSourceConflictRecord,
    PackSubscriptionRecord,
};

impl LocalDatabase {
    pub async fn replace_pack_catalog(
        &self,
        packs: Vec<PackSubscriptionRecord>,
        sources: Vec<PackSkillSourceRecord>,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            let tx = connection.transaction()?;
            tx.execute("DELETE FROM pack_skill_sources", [])?;
            tx.execute("DELETE FROM pack_subscriptions", [])?;
            for pack in packs {
                tx.execute(
                    "INSERT INTO pack_subscriptions \
                     (source_id,source_kind,source_label,source_team_id,enforced,pack_resource_id,locator,resource_generation,pack_version,degraded,updated_at_unix_ms) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                    params![
                        pack.source_id,
                        pack.source_kind,
                        pack.source_label,
                        pack.source_team_id,
                        i64::from(pack.enforced),
                        pack.pack_resource_id,
                        pack.locator,
                        pack.resource_generation,
                        pack.pack_version,
                        i64::from(pack.degraded),
                        now_unix_ms,
                    ],
                )?;
            }
            for source in sources {
                tx.execute(
                    "INSERT INTO pack_skill_sources \
                     (source_id,pack_resource_id,resource_id,locator,owner,skill_name,resource_generation,desired_revision_id,unavailable_reason,updated_at_unix_ms) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        source.source_id,
                        source.pack_resource_id,
                        source.resource_id,
                        source.locator,
                        source.owner,
                        source.skill_name,
                        source.resource_generation,
                        source.desired_revision_id,
                        source.unavailable_reason,
                        now_unix_ms,
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn pack_subscriptions(&self) -> Result<Vec<PackSubscriptionRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT source_id,source_kind,source_label,source_team_id,enforced,pack_resource_id,locator,resource_generation,pack_version,degraded \
                 FROM pack_subscriptions ORDER BY locator,source_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(PackSubscriptionRecord {
                    source_id: row.get(0)?,
                    source_kind: row.get(1)?,
                    source_label: row.get(2)?,
                    source_team_id: row.get(3)?,
                    enforced: row.get::<_, i64>(4)? != 0,
                    pack_resource_id: row.get(5)?,
                    locator: row.get(6)?,
                    resource_generation: row.get(7)?,
                    pack_version: row.get(8)?,
                    degraded: row.get::<_, i64>(9)? != 0,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn enforced_pack_resource_ids(&self) -> Result<Vec<String>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT DISTINCT s.resource_id FROM pack_skill_sources s \
                 JOIN pack_subscriptions p ON p.source_id=s.source_id \
                 WHERE p.enforced=1 ORDER BY s.resource_id",
            )?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<Result<Vec<String>, _>>()
                .map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn pack_skill_sources(&self) -> Result<Vec<PackSkillSourceRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT source_id,pack_resource_id,resource_id,locator,owner,skill_name,resource_generation,desired_revision_id,unavailable_reason \
                 FROM pack_skill_sources ORDER BY resource_id,source_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(PackSkillSourceRecord {
                    source_id: row.get(0)?,
                    pack_resource_id: row.get(1)?,
                    resource_id: row.get(2)?,
                    locator: row.get(3)?,
                    owner: row.get(4)?,
                    skill_name: row.get(5)?,
                    resource_generation: row.get(6)?,
                    desired_revision_id: row.get(7)?,
                    unavailable_reason: row.get(8)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn pack_materialized_skills(
        &self,
    ) -> Result<Vec<PackMaterializedSkillRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT resource_id,locator,owner,skill_name,resource_generation,desired_revision_id,desired_root_tree_id,harness_name,materialized_revision_id \
                 FROM pack_materialized_skills ORDER BY owner,skill_name,resource_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(PackMaterializedSkillRecord {
                    resource_id: row.get(0)?,
                    locator: row.get(1)?,
                    owner: row.get(2)?,
                    skill_name: row.get(3)?,
                    resource_generation: row.get(4)?,
                    desired_revision_id: row.get(5)?,
                    desired_root_tree_id: row.get(6)?,
                    harness_name: row.get(7)?,
                    materialized_revision_id: row.get(8)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn remove_pack_materialized_record(
        &self,
        resource_id: String,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM pack_materialized_skills WHERE resource_id=?1",
                params![resource_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn pack_source_conflicts(
        &self,
    ) -> Result<Vec<PackSourceConflictRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT resource_id,source_ids_json,source_labels_json,revision_ids_json,message FROM pack_source_conflicts ORDER BY resource_id",
            )?;
            let rows = statement.query_map([], |row| {
                let source_pack_ids: String = row.get(1)?;
                let source_labels: String = row.get(2)?;
                let revision_ids: String = row.get(3)?;
                Ok((row.get(0)?, source_pack_ids, source_labels, revision_ids, row.get(4)?))
            })?;
            let rows = rows.collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|(resource_id, source_ids, source_labels, revision_ids, message)| {
                    Ok(PackSourceConflictRecord {
                        resource_id,
                        source_ids: serde_json::from_str(&source_ids)?,
                        source_labels: serde_json::from_str(&source_labels)?,
                        revision_ids: serde_json::from_str(&revision_ids)?,
                        message,
                    })
                })
                .collect()
        })
        .await
    }

    pub async fn create_pack_apply_journal(
        &self,
        operation_id: OperationId,
        payload: PackApplyJournalPayload,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        let payload = serde_json::to_string(&payload)?;
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO pack_apply_journal (operation_id,payload_json,state,updated_at_unix_ms) \
                 VALUES (?1,?2,'verified',?3)",
                params![operation_id.to_string(), payload, now_unix_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn incomplete_pack_apply_journals(
        &self,
    ) -> Result<Vec<PackApplyJournal>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT operation_id,payload_json,state FROM pack_apply_journal WHERE state<>'complete' ORDER BY updated_at_unix_ms,operation_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })?;
            let rows = rows.collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|(operation_id, payload, state)| {
                    Ok(PackApplyJournal {
                        operation_id: operation_id.parse().map_err(|error| {
                            LocalDbError::Corrupt(format!("invalid pack apply operation ID: {error}"))
                        })?,
                        payload: serde_json::from_str(&payload)?,
                        complete: state == "complete",
                    })
                })
                .collect()
        })
        .await
    }

    pub async fn discard_pack_apply_journal(
        &self,
        operation_id: OperationId,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM pack_apply_journal WHERE operation_id=?1",
                params![operation_id.to_string()],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn commit_pack_apply(
        &self,
        operation_id: OperationId,
        skills: Vec<PackMaterializedSkillRecord>,
        conflicts: Vec<PackSourceConflictRecord>,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            let tx = connection.transaction()?;
            tx.execute("DELETE FROM pack_materialized_skills", [])?;
            for skill in skills {
                tx.execute(
                    "INSERT INTO pack_materialized_skills \
                     (resource_id,locator,owner,skill_name,resource_generation,desired_revision_id,desired_root_tree_id,harness_name,materialized_revision_id,updated_at_unix_ms) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        skill.resource_id,
                        skill.locator,
                        skill.owner,
                        skill.skill_name,
                        skill.resource_generation,
                        skill.desired_revision_id,
                        skill.desired_root_tree_id,
                        skill.harness_name,
                        skill.materialized_revision_id,
                        now_unix_ms,
                    ],
                )?;
            }
            tx.execute("DELETE FROM pack_source_conflicts", [])?;
            for conflict in conflicts {
                tx.execute(
                    "INSERT INTO pack_source_conflicts \
                     (resource_id,source_ids_json,source_labels_json,revision_ids_json,message,updated_at_unix_ms) VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        conflict.resource_id,
                        serde_json::to_string(&conflict.source_ids)?,
                        serde_json::to_string(&conflict.source_labels)?,
                        serde_json::to_string(&conflict.revision_ids)?,
                        conflict.message,
                        now_unix_ms,
                    ],
                )?;
            }
            let changed = tx.execute(
                "UPDATE pack_apply_journal SET state='complete',updated_at_unix_ms=?1 WHERE operation_id=?2 AND state='verified'",
                params![now_unix_ms, operation_id.to_string()],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows.into());
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
