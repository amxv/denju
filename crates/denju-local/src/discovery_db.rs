use rusqlite::{OptionalExtension, params};

use crate::{IdentityRecord, InstallationRecord, LocalDatabase, LocalDbError, OwnedSkillRecord};

pub struct LocalCatalogState {
    pub installation: Option<InstallationRecord>,
    pub identity: Option<IdentityRecord>,
    pub owned_skills: Vec<OwnedSkillRecord>,
    pub discovery_records: Vec<LocalDiscoveryRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDiscoveryRecord {
    pub resource_id: String,
    pub locator: String,
    pub owner: String,
    pub skill_name: String,
    pub resource_generation: i64,
    pub description: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub fork_upstream_locator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnonymousFollowRecord {
    pub user_id: String,
    pub username: String,
}

impl LocalDatabase {
    /// Reads the local catalog bootstrap state in one worker round trip. Search is a short-lived
    /// CLI hot path, so opening SQLite and then separately asking for installation, identity,
    /// owned resources, and cached discovery rows adds measurable scheduler overhead even when
    /// the installation owns no skills. Callers that discover owned working trees still refresh
    /// their filesystem-derived metadata before presenting results.
    pub async fn catalog_state(&self) -> Result<LocalCatalogState, LocalDbError> {
        self.call(|connection| {
            let installation = connection
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
                .optional()?;
            let identity = connection
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
                .optional()?;

            let owned_skills = {
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
                rows.collect::<Result<Vec<_>, _>>()?
            };
            let discovery_records = if owned_skills.is_empty() {
                Vec::new()
            } else {
                read_local_discovery_records(connection)?
            };
            Ok(LocalCatalogState {
                installation,
                identity,
                owned_skills,
                discovery_records,
            })
        })
        .await
    }

    pub async fn upsert_skill_discovery_metadata(
        &self,
        resource_id: String,
        description: String,
        license: Option<String>,
        compatibility: Option<String>,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO skill_discovery_metadata \
                 (resource_id,description,license,compatibility,updated_at_unix_ms) VALUES (?1,?2,?3,?4,?5) \
                 ON CONFLICT(resource_id) DO UPDATE SET description=excluded.description,license=excluded.license, \
                   compatibility=excluded.compatibility,updated_at_unix_ms=excluded.updated_at_unix_ms",
                params![resource_id, description, license, compatibility, now_unix_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn local_discovery_records(&self) -> Result<Vec<LocalDiscoveryRecord>, LocalDbError> {
        self.call(read_local_discovery_records).await
    }

    pub async fn upsert_anonymous_follow(
        &self,
        record: AnonymousFollowRecord,
        now_unix_ms: i64,
    ) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "INSERT INTO anonymous_follows (user_id,username,updated_at_unix_ms) VALUES (?1,?2,?3) \
                 ON CONFLICT(user_id) DO UPDATE SET username=excluded.username,updated_at_unix_ms=excluded.updated_at_unix_ms",
                params![record.user_id, record.username, now_unix_ms],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn anonymous_follows(&self) -> Result<Vec<AnonymousFollowRecord>, LocalDbError> {
        self.call(|connection| {
            let mut statement = connection.prepare(
                "SELECT user_id,username FROM anonymous_follows ORDER BY username,user_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(AnonymousFollowRecord {
                    user_id: row.get(0)?,
                    username: row.get(1)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(LocalDbError::from)
        })
        .await
    }

    pub async fn remove_anonymous_follow(&self, user_id: String) -> Result<(), LocalDbError> {
        self.call(move |connection| {
            connection.execute(
                "DELETE FROM anonymous_follows WHERE user_id=?1",
                params![user_id],
            )?;
            Ok(())
        })
        .await
    }
}

fn read_local_discovery_records(
    connection: &mut rusqlite::Connection,
) -> Result<Vec<LocalDiscoveryRecord>, LocalDbError> {
    let mut statement = connection.prepare(
        "SELECT o.resource_id,o.locator,o.owner,o.skill_name,o.resource_generation,m.description,m.license,m.compatibility,f.upstream_locator \
         FROM owned_skills o JOIN skill_discovery_metadata m ON m.resource_id=o.resource_id \
         LEFT JOIN local_forks f ON f.resource_id=o.resource_id \
         ORDER BY o.owner,o.skill_name,o.resource_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(LocalDiscoveryRecord {
            resource_id: row.get(0)?,
            locator: row.get(1)?,
            owner: row.get(2)?,
            skill_name: row.get(3)?,
            resource_generation: row.get(4)?,
            description: row.get(5)?,
            license: row.get(6)?,
            compatibility: row.get(7)?,
            fork_upstream_locator: row.get(8)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(LocalDbError::from)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::OwnedSkillRecord;

    #[tokio::test]
    async fn discovery_metadata_and_anonymous_follows_are_local_only_state() {
        let dir = tempdir().unwrap();
        let db = LocalDatabase::open(dir.path().join("state.db"))
            .await
            .unwrap();
        db.upsert_owned_skill_desired(
            OwnedSkillRecord {
                resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".into(),
                locator: "@alice/review".into(),
                owner: "alice".into(),
                skill_name: "review".into(),
                resource_generation: 4,
                workspace_generation: 3,
                desired_revision_id: "11".repeat(32),
                harness_name: None,
                materialized_revision_id: None,
            },
            1,
        )
        .await
        .unwrap();
        db.upsert_skill_discovery_metadata(
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".into(),
            "Reviews Rust code".into(),
            Some("MIT".into()),
            Some("Rust projects".into()),
            2,
        )
        .await
        .unwrap();
        let records = db.local_discovery_records().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].resource_generation, 4);
        assert_eq!(records[0].description, "Reviews Rust code");

        db.upsert_anonymous_follow(
            AnonymousFollowRecord {
                user_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f003".into(),
                username: "@bob".into(),
            },
            3,
        )
        .await
        .unwrap();
        assert_eq!(db.anonymous_follows().await.unwrap().len(), 1);
        db.remove_anonymous_follow("01890f47-6a1d-7ad0-8f43-9a4d8c29f003".into())
            .await
            .unwrap();
        assert!(db.anonymous_follows().await.unwrap().is_empty());
    }
}
