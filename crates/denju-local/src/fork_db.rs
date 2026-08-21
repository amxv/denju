use rusqlite::{OptionalExtension, params};

use crate::{
    ForkSyncConflictRecord, LocalDatabase, LocalDbError, LocalForkRecord, LocalRevisionRecord,
};

impl LocalDatabase {
    pub async fn fork_sync_conflict(
        &self,
        resource_id: String,
    ) -> Result<Option<ForkSyncConflictRecord>, LocalDbError> {
        self.call(move |connection| {
            let row = connection
                .query_row(
                    "SELECT resource_id,sync_base_revision_id,fork_revision_id,upstream_revision_id,conflict_paths_json \
                     FROM fork_sync_conflicts WHERE resource_id=?1",
                    params![resource_id],
                    |row| {
                        let paths: String = row.get(4)?;
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            paths,
                        ))
                    },
                )
                .optional()?;
            row.map(|(resource_id, sync_base_revision_id, fork_revision_id, upstream_revision_id, paths)| {
                Ok(ForkSyncConflictRecord {
                    resource_id,
                    sync_base_revision_id,
                    fork_revision_id,
                    upstream_revision_id,
                    conflict_paths: serde_json::from_str(&paths)?,
                })
            })
            .transpose()
        })
        .await
    }

    pub async fn save_fork_sync_conflict(
        &self,
        conflict: ForkSyncConflictRecord,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        if conflict.conflict_paths.is_empty() {
            return Err(LocalDbError::Corrupt(
                "fork sync conflict must contain at least one path".to_owned(),
            ));
        }
        let paths = serde_json::to_string(&conflict.conflict_paths)?;
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO fork_sync_conflicts \
                 (resource_id,sync_base_revision_id,fork_revision_id,upstream_revision_id,conflict_paths_json,created_at_unix_ms,updated_at_unix_ms) \
                 VALUES (?1,?2,?3,?4,?5,?6,?6) \
                 ON CONFLICT(resource_id) DO UPDATE SET \
                   sync_base_revision_id=excluded.sync_base_revision_id, \
                   fork_revision_id=excluded.fork_revision_id, \
                   upstream_revision_id=excluded.upstream_revision_id, \
                   conflict_paths_json=excluded.conflict_paths_json, \
                   updated_at_unix_ms=excluded.updated_at_unix_ms",
                params![
                    conflict.resource_id,
                    conflict.sync_base_revision_id,
                    conflict.fork_revision_id,
                    conflict.upstream_revision_id,
                    paths,
                    now_unix_ms,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn clear_fork_sync_conflict(&self, resource_id: String) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM fork_sync_conflicts WHERE resource_id=?1",
                params![resource_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn local_forks(&self) -> Result<Vec<LocalForkRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT resource_id,upstream_resource_id,upstream_locator,created_from_revision_id, \
                        sync_base_revision_id,desired_name,state FROM local_forks ORDER BY resource_id",
            )?;
            let rows = statement.query_map([], local_fork_from_row)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn local_fork_for_upstream(
        &self,
        upstream_resource_id: String,
    ) -> Result<Option<LocalForkRecord>, LocalDbError> {
        self.call(move |connection| {
            connection
                .query_row(
                    "SELECT resource_id,upstream_resource_id,upstream_locator,created_from_revision_id, \
                            sync_base_revision_id,desired_name,state FROM local_forks WHERE upstream_resource_id=?1",
                    params![upstream_resource_id],
                    local_fork_from_row,
                )
                .optional()
                .map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn save_local_fork(
        &self,
        fork: LocalForkRecord,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO local_forks \
                 (resource_id,upstream_resource_id,upstream_locator,created_from_revision_id,sync_base_revision_id,desired_name,state,created_at_unix_ms,updated_at_unix_ms) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8) \
                 ON CONFLICT(resource_id) DO UPDATE SET sync_base_revision_id=excluded.sync_base_revision_id, \
                 desired_name=excluded.desired_name,state=excluded.state,updated_at_unix_ms=excluded.updated_at_unix_ms",
                params![
                    fork.resource_id,
                    fork.upstream_resource_id,
                    fork.upstream_locator,
                    fork.created_from_revision_id,
                    fork.sync_base_revision_id,
                    fork.desired_name,
                    fork.state,
                    now_unix_ms,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn remove_local_fork(&self, resource_id: String) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM local_forks WHERE resource_id=?1",
                params![resource_id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn local_revision_history(
        &self,
        resource_id: String,
    ) -> Result<Vec<LocalRevisionRecord>, LocalDbError> {
        self.call(move |connection| {
            let mut statement = connection.prepare(
                "SELECT operation_id,resource_id,revision_id,parent_revision_id,merge_parent_revision_id, \
                        expected_generation,root_tree_id,manifest_json,state \
                 FROM local_revisions WHERE resource_id=?1 ORDER BY created_at_unix_ms,operation_id",
            )?;
            let rows = statement.query_map(params![resource_id], |row| {
                let expected_head_revision_id = row.get::<_, String>(3)?;
                let merge_parent_revision_id = row.get::<_, Option<String>>(4)?;
                let mut parent_revision_ids = vec![expected_head_revision_id.clone()];
                if let Some(parent) = merge_parent_revision_id {
                    parent_revision_ids.push(parent);
                }
                parent_revision_ids.sort();
                Ok(LocalRevisionRecord {
                    operation_id: row.get(0)?,
                    resource_id: row.get(1)?,
                    revision_id: row.get(2)?,
                    expected_head_revision_id,
                    parent_revision_ids,
                    expected_generation: row.get(5)?,
                    root_tree_id: row.get(6)?,
                    manifest_json: row.get(7)?,
                    state: row.get(8)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn commit_local_only_revision(
        &self,
        revision: LocalRevisionRecord,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        let mut parents = revision.parent_revision_ids.clone();
        parents.sort();
        if parents.len() != 1 || parents[0] != revision.expected_head_revision_id {
            return Err(LocalDbError::Corrupt(
                "local-only fork revisions require exactly one expected-head parent".to_owned(),
            ));
        }
        let next_generation = revision
            .expected_generation
            .checked_add(1)
            .ok_or_else(|| LocalDbError::Corrupt("local fork generation overflow".to_owned()))?;
        self.call(move |connection| {
            let tx = connection.transaction()?;
            tx.execute(
                "INSERT INTO local_revisions \
                 (operation_id,resource_id,revision_id,parent_revision_id,merge_parent_revision_id,expected_generation,root_tree_id,manifest_json,state,created_at_unix_ms,updated_at_unix_ms) \
                 VALUES (?1,?2,?3,?4,NULL,?5,?6,?7,'synced',?8,?8)",
                params![
                    revision.operation_id,
                    revision.resource_id,
                    revision.revision_id,
                    revision.expected_head_revision_id,
                    revision.expected_generation,
                    revision.root_tree_id,
                    revision.manifest_json,
                    now_unix_ms,
                ],
            )?;
            tx.execute(
                "UPDATE owned_skills SET resource_generation=?1,desired_revision_id=?2,materialized_revision_id=?2,updated_at_unix_ms=?3 WHERE resource_id=?4",
                params![
                    next_generation,
                    revision.revision_id,
                    now_unix_ms,
                    revision.resource_id,
                ],
            )?;
            tx.execute(
                "UPDATE workspace_state SET base_generation=?1,base_revision_id=?2,local_head_revision_id=?2, \
                 valid_root_tree_id=?3,status='clean',error_message=NULL,pending_rename=NULL,updated_at_unix_ms=?4 WHERE resource_id=?5",
                params![
                    next_generation,
                    revision.revision_id,
                    revision.root_tree_id,
                    now_unix_ms,
                    revision.resource_id,
                ],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn record_local_fork_revision(
        &self,
        revision: LocalRevisionRecord,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        if revision.parent_revision_ids.len() != 1
            || revision.parent_revision_ids[0] != revision.expected_head_revision_id
        {
            return Err(LocalDbError::Corrupt(
                "initial local fork revision requires one upstream parent".to_owned(),
            ));
        }
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO local_revisions \
                 (operation_id,resource_id,revision_id,parent_revision_id,merge_parent_revision_id,expected_generation,root_tree_id,manifest_json,state,created_at_unix_ms,updated_at_unix_ms) \
                 VALUES (?1,?2,?3,?4,NULL,?5,?6,?7,'synced',?8,?8)",
                params![
                    revision.operation_id,
                    revision.resource_id,
                    revision.revision_id,
                    revision.expected_head_revision_id,
                    revision.expected_generation,
                    revision.root_tree_id,
                    revision.manifest_json,
                    now_unix_ms,
                ],
            )?;
            Ok(())
        })
        .await
    }
}

fn local_fork_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalForkRecord> {
    Ok(LocalForkRecord {
        resource_id: row.get(0)?,
        upstream_resource_id: row.get(1)?,
        upstream_locator: row.get(2)?,
        created_from_revision_id: row.get(3)?,
        sync_base_revision_id: row.get(4)?,
        desired_name: row.get(5)?,
        state: row.get(6)?,
    })
}
