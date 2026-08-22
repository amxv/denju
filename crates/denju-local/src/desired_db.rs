use rusqlite::{OptionalExtension, params};

use crate::{
    LocalDatabase, LocalDbError, ManagedSkillRecord, OwnedSkillRecord, SubscriptionRecord,
};

impl LocalDatabase {
    pub async fn source_suppressions(
        &self,
        source_kind: &'static str,
    ) -> Result<Vec<String>, LocalDbError> {
        self.call(move |connection| {
            let mut statement = connection.prepare(
                "SELECT resource_id FROM desired_source_suppressions WHERE source_kind=?1 ORDER BY resource_id",
            )?;
            let rows = statement.query_map(params![source_kind], |row| row.get(0))?;
            rows.collect::<Result<Vec<String>, _>>()
                .map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn reconcile_source_suppressions(
        &self,
        subscriptions: Vec<(String, String)>,
        owned: Vec<(String, String)>,
        preserve_resources: Vec<String>,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            let tx = connection.transaction()?;
            let preserve = preserve_resources.into_iter().collect::<std::collections::BTreeSet<_>>();
            let existing = {
                let mut statement = tx.prepare(
                    "SELECT source_kind,resource_id FROM desired_source_suppressions ORDER BY source_kind,resource_id",
                )?;
                let rows = statement.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            for (kind, resource_id) in existing {
                if !preserve.contains(&resource_id) {
                    tx.execute(
                        "DELETE FROM desired_source_suppressions WHERE source_kind=?1 AND resource_id=?2",
                        params![kind, resource_id],
                    )?;
                }
            }
            for (kind, values) in [("subscription", subscriptions), ("owned", owned)] {
                for (resource_id, enforcing_source_id) in values {
                    if preserve.contains(&resource_id) {
                        continue;
                    }
                    tx.execute(
                        "INSERT INTO desired_source_suppressions \
                         (source_kind,resource_id,enforcing_source_id,updated_at_unix_ms) VALUES (?1,?2,?3,?4) \
                         ON CONFLICT(source_kind,resource_id) DO UPDATE SET \
                         enforcing_source_id=excluded.enforcing_source_id,updated_at_unix_ms=excluded.updated_at_unix_ms",
                        params![kind, resource_id, enforcing_source_id, now_unix_ms],
                    )?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn subscription_edit_locks(&self) -> Result<Vec<String>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection
                .prepare("SELECT resource_id FROM subscription_edit_locks ORDER BY resource_id")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<Result<Vec<String>, _>>()
                .map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn save_subscription_edit_lock(
        &self,
        resource_id: String,
        message: String,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO subscription_edit_locks (resource_id,message,updated_at_unix_ms) \
                 VALUES (?1,?2,?3) ON CONFLICT(resource_id) DO UPDATE SET \
                 message=excluded.message,updated_at_unix_ms=excluded.updated_at_unix_ms",
                params![resource_id, message, now_unix_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn clear_subscription_edit_lock(
        &self,
        resource_id: String,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM subscription_edit_locks WHERE resource_id=?1",
                params![resource_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn subscriptions(&self) -> Result<Vec<SubscriptionRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT resource_id, locator, owner, skill_name, resource_generation, release_version, \
                        desired_revision_id, harness_name, materialized_revision_id, retain_on_delete, retained_after_delete, \
                        live_private,desired_root_tree_id \
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
                    live_private: row.get::<_, i64>(11)? != 0,
                    desired_root_tree_id: row.get(12)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn owned_skills(&self) -> Result<Vec<OwnedSkillRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT resource_id, locator, owner, skill_name, resource_generation,workspace_generation,desired_revision_id, \
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
                    workspace_generation: row.get(5)?,
                    desired_revision_id: row.get(6)?,
                    harness_name: row.get(7)?,
                    materialized_revision_id: row.get(8)?,
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
                 FROM subscriptions s WHERE NOT EXISTS ( \
                   SELECT 1 FROM desired_source_suppressions x WHERE x.source_kind='subscription' AND x.resource_id=s.resource_id \
                 ) \
                 UNION ALL \
                 SELECT resource_id, locator, owner, skill_name, harness_name, materialized_revision_id \
                 FROM owned_skills o WHERE NOT EXISTS ( \
                   SELECT 1 FROM desired_source_suppressions x WHERE x.source_kind='owned' AND x.resource_id=o.resource_id \
                 ) \
                 UNION ALL \
                 SELECT resource_id, locator, owner, skill_name, harness_name, materialized_revision_id \
                 FROM pack_materialized_skills \
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
                 (resource_id, locator, owner, skill_name, resource_generation,workspace_generation,desired_revision_id, updated_at_unix_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(resource_id) DO UPDATE SET \
                   locator=excluded.locator, owner=excluded.owner, skill_name=excluded.skill_name, \
                   resource_generation=excluded.resource_generation,workspace_generation=excluded.workspace_generation, desired_revision_id=excluded.desired_revision_id, \
                   updated_at_unix_ms=excluded.updated_at_unix_ms",
                params![
                    record.resource_id,
                    record.locator,
                    record.owner,
                    record.skill_name,
                    record.resource_generation,
                    record.workspace_generation,
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
                            desired_revision_id, harness_name, materialized_revision_id, retain_on_delete, retained_after_delete, \
                            live_private,desired_root_tree_id \
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
                            live_private: row.get::<_, i64>(11)? != 0,
                            desired_root_tree_id: row.get(12)?,
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
                 (resource_id, locator, owner, skill_name, resource_generation, release_version, desired_revision_id, retain_on_delete, retained_after_delete, live_private, desired_root_tree_id, updated_at_unix_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
                 ON CONFLICT(resource_id) DO UPDATE SET \
                   locator=excluded.locator, owner=excluded.owner, skill_name=excluded.skill_name, \
                   resource_generation=excluded.resource_generation, release_version=excluded.release_version, \
                   desired_revision_id=excluded.desired_revision_id, retain_on_delete=excluded.retain_on_delete, \
                   retained_after_delete=excluded.retained_after_delete, live_private=excluded.live_private, \
                   desired_root_tree_id=excluded.desired_root_tree_id, updated_at_unix_ms=excluded.updated_at_unix_ms",
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
                    i64::from(record.live_private),
                    record.desired_root_tree_id,
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
            let tx = connection.transaction()?;
            // Overlapping durable relationships can briefly coexist before the next resolver
            // pass (for example a just-created direct subscription while team policy is active).
            // A successful materialization only proves the source(s) whose desired revision is
            // exactly the bytes we switched, so never stamp a different desired source current.
            let subscription_changed = tx.execute(
                "UPDATE subscriptions SET materialized_revision_id=?1, updated_at_unix_ms=?2 \
                 WHERE resource_id=?3 AND desired_revision_id=?1 AND NOT EXISTS (SELECT 1 FROM desired_source_suppressions x \
                   WHERE x.source_kind='subscription' AND x.resource_id=subscriptions.resource_id)",
                params![revision_id, now_unix_ms, resource_id],
            )?;
            let owned_changed = tx.execute(
                "UPDATE owned_skills SET materialized_revision_id=?1, updated_at_unix_ms=?2 \
                 WHERE resource_id=?3 AND desired_revision_id=?1 AND NOT EXISTS (SELECT 1 FROM desired_source_suppressions x \
                   WHERE x.source_kind='owned' AND x.resource_id=owned_skills.resource_id)",
                params![revision_id, now_unix_ms, resource_id],
            )?;
            let pack_changed = tx.execute(
                "UPDATE pack_materialized_skills SET materialized_revision_id=?1, updated_at_unix_ms=?2 \
                 WHERE resource_id=?3 AND desired_revision_id=?1",
                params![revision_id, now_unix_ms, resource_id],
            )?;
            if subscription_changed + owned_changed + pack_changed == 0 {
                return Err(rusqlite::Error::QueryReturnedNoRows.into());
            }
            tx.commit()?;
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
            let tx = connection.transaction()?;
            let subscription_changed = tx.execute(
                "UPDATE subscriptions SET harness_name=?1, updated_at_unix_ms=?2 \
                 WHERE resource_id=?3 AND NOT EXISTS (SELECT 1 FROM desired_source_suppressions x \
                   WHERE x.source_kind='subscription' AND x.resource_id=subscriptions.resource_id)",
                params![harness_name, now_unix_ms, resource_id],
            )?;
            let owned_changed = tx.execute(
                "UPDATE owned_skills SET harness_name=?1, updated_at_unix_ms=?2 \
                 WHERE resource_id=?3 AND NOT EXISTS (SELECT 1 FROM desired_source_suppressions x \
                   WHERE x.source_kind='owned' AND x.resource_id=owned_skills.resource_id)",
                params![harness_name, now_unix_ms, resource_id],
            )?;
            let pack_changed = tx.execute(
                "UPDATE pack_materialized_skills SET harness_name=?1, updated_at_unix_ms=?2 WHERE resource_id=?3",
                params![harness_name, now_unix_ms, resource_id],
            )?;
            if subscription_changed + owned_changed + pack_changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows.into());
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
