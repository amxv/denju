use std::{collections::BTreeSet, str::FromStr};

use denju_core::{ResourceLocator, RevisionId};
use denju_wire::{
    ApiError, ApiErrorCode, PublicSkillManifest, SkillHistoryResponse, SkillRelease,
    SkillRevisionDetail, SkillRevisionSummary, SnapshotDownload,
};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    Registry,
    access::{skill_access_for_user, user_can_read_revision},
    admin::active_quarantine,
    ingest::decode_32,
    internal_api_error,
    lifecycle::resolve_active_skill_locator_tx,
    revision_graph::{revision_parents, revision_parents_tx},
};

impl Registry {
    pub async fn skill_history(
        &self,
        bearer: Option<&str>,
        locator: &str,
    ) -> Result<SkillHistoryResponse, ApiError> {
        let parsed = ResourceLocator::from_str(locator)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let authority = match bearer {
            Some(token) => self.user_authority(token, "skills:read").await.ok(),
            None => None,
        };
        let mut actor_tx = if let Some(authority) = authority.as_ref() {
            Some(self.begin_actor_tx(authority.user_id).await?)
        } else {
            None
        };
        let resolved = if let Some(tx) = actor_tx.as_mut() {
            resolve_active_skill_locator_tx(tx, &parsed).await?
        } else {
            self.resolve_active_skill_locator(&parsed).await?
        };
        let row = if let Some(tx) = actor_tx.as_mut() {
            sqlx::query_as::<_, (String, i64, Option<i64>)>(
                "SELECT visibility,generation,latest_release_version FROM resources \
                 WHERE id=$1 AND kind='skill' AND deleted_at IS NULL",
            )
            .bind(resolved.resource_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal_api_error)?
        } else {
            sqlx::query_as::<_, (String, i64, Option<i64>)>(
                "SELECT visibility,generation,latest_release_version FROM resources \
                 WHERE id=$1 AND kind='skill' AND deleted_at IS NULL",
            )
            .bind(resolved.resource_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal_api_error)?
        }
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "skill not found"))?;
        let resource_id = resolved.resource_id;
        if active_quarantine(&self.pool, resource_id, None)
            .await?
            .is_some()
        {
            return Err(ApiError::new(ApiErrorCode::NotFound, "skill not found"));
        }
        let latest_release_quarantined = match row.2 {
            Some(version) => active_quarantine(&self.pool, resource_id, Some(version))
                .await?
                .is_some(),
            None => false,
        };
        let access = if let (Some(authority), Some(tx)) = (authority.as_ref(), actor_tx.as_mut()) {
            Some(
                skill_access_for_user(tx, authority.user_id, authority.namespace_id, resource_id)
                    .await?,
            )
        } else {
            None
        };
        let private_read = access
            .as_ref()
            .is_some_and(|access| access.can_read_private());
        let workspace_read = access
            .as_ref()
            .is_some_and(|access| access.workspace_access);
        let release_read = row.2.is_some()
            && !latest_release_quarantined
            && match access.as_ref() {
                Some(access) => access.can_read_released(),
                None => row.0 == "public",
            };
        if !private_read && !workspace_read && !release_read {
            return Err(ApiError::new(ApiErrorCode::NotFound, "skill not found"));
        }

        let releases = if let Some(tx) = actor_tx.as_mut() {
            history_release_rows_tx(tx, resource_id).await?
        } else {
            self.history_release_rows(resource_id).await?
        };
        let released_ids = releases
            .iter()
            .map(|release| release.revision_id.clone())
            .collect::<BTreeSet<_>>();
        let rows = if let Some(tx) = actor_tx.as_mut() {
            sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT revision_id FROM resource_revision_snapshots \
                 WHERE resource_id=$1 ORDER BY created_at,revision_id",
            )
            .bind(resource_id)
            .fetch_all(&mut **tx)
            .await
            .map_err(internal_api_error)?
        } else {
            sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT revision_id FROM resource_revision_snapshots \
                 WHERE resource_id=$1 ORDER BY created_at,revision_id",
            )
            .bind(resource_id)
            .fetch_all(&self.pool)
            .await
            .map_err(internal_api_error)?
        };
        let mut revisions = Vec::new();
        for bytes in rows {
            let revision_bytes = decode_32(&bytes, "stored revision ID")?;
            let revision = hex::encode(revision_bytes);
            let readable = if private_read {
                true
            } else if let (Some(access), Some(tx)) = (access.as_ref(), actor_tx.as_mut()) {
                user_can_read_revision(tx, access, resource_id, &revision_bytes).await?
            } else {
                released_ids.contains(&revision)
            };
            if !readable {
                continue;
            }
            let released_versions = releases
                .iter()
                .filter(|release| release.revision_id == revision)
                .map(|release| release.version)
                .collect();
            let parent_revision_ids = if let Some(tx) = actor_tx.as_mut() {
                revision_parents_tx(tx, &revision).await?
            } else {
                revision_parents(&self.pool, &revision).await?
            };
            revisions.push(SkillRevisionSummary {
                parent_revision_ids,
                revision_id: revision,
                released_versions,
            });
        }
        let workspace_revision_id = if let Some(access) = access.as_ref()
            && access.workspace_access
        {
            workspace_head_for_user(
                actor_tx.as_mut().ok_or_else(|| {
                    ApiError::new(ApiErrorCode::Internal, "actor transaction missing")
                })?,
                resource_id,
                access.user_id,
            )
            .await?
        } else if private_read {
            match owner_workspace_head(
                actor_tx.as_mut().ok_or_else(|| {
                    ApiError::new(ApiErrorCode::Internal, "actor transaction missing")
                })?,
                resource_id,
            )
            .await?
            {
                Some(head) => head,
                None => latest_release_head(&releases)?,
            }
        } else {
            latest_release_head(&releases)?
        };
        if let Some(tx) = actor_tx {
            tx.commit().await.map_err(internal_api_error)?;
        }
        Ok(SkillHistoryResponse {
            resource_id: resource_id.to_string(),
            locator: format!("@{}/{}", resolved.owner, resolved.name),
            generation: u64::try_from(row.1).map_err(|_| {
                ApiError::new(ApiErrorCode::Internal, "stored generation is invalid")
            })?,
            workspace_revision_id,
            revisions,
            releases,
        })
    }

    pub async fn skill_revision_detail(
        &self,
        bearer: Option<&str>,
        locator: &str,
        revision_id: &str,
    ) -> Result<SkillRevisionDetail, ApiError> {
        let parsed = ResourceLocator::from_str(locator)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let revision = RevisionId::from_str(revision_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let authority = match bearer {
            Some(token) => self.user_authority(token, "skills:read").await.ok(),
            None => None,
        };
        let mut actor_tx = if let Some(authority) = authority.as_ref() {
            Some(self.begin_actor_tx(authority.user_id).await?)
        } else {
            None
        };
        let resolved = if let Some(tx) = actor_tx.as_mut() {
            resolve_active_skill_locator_tx(tx, &parsed).await?
        } else {
            self.resolve_active_skill_locator(&parsed).await?
        };
        let resource_id = resolved.resource_id;
        if active_quarantine(&self.pool, resource_id, None)
            .await?
            .is_some()
        {
            return Err(ApiError::new(ApiErrorCode::NotFound, "revision not found"));
        }
        let row = if let Some(tx) = actor_tx.as_mut() {
            sqlx::query(
                "SELECT r.visibility,rrs.manifest_json,rrs.snapshot_key,rrs.snapshot_sha256,rrs.snapshot_size, \
                        EXISTS(SELECT 1 FROM skill_releases sr WHERE sr.resource_id=r.id AND sr.revision_id=$2) AS released \
                 FROM resources r JOIN resource_revision_snapshots rrs ON rrs.resource_id=r.id AND rrs.revision_id=$2 \
                 WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL",
            )
            .bind(resource_id)
            .bind(revision.as_bytes().as_slice())
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal_api_error)?
        } else {
            sqlx::query(
                "SELECT r.visibility,rrs.manifest_json,rrs.snapshot_key,rrs.snapshot_sha256,rrs.snapshot_size, \
                        EXISTS(SELECT 1 FROM skill_releases sr WHERE sr.resource_id=r.id AND sr.revision_id=$2) AS released \
                 FROM resources r JOIN resource_revision_snapshots rrs ON rrs.resource_id=r.id AND rrs.revision_id=$2 \
                 WHERE r.id=$1 AND r.kind='skill' AND r.deleted_at IS NULL",
            )
            .bind(resource_id)
            .bind(revision.as_bytes().as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(internal_api_error)?
        }
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "revision not found"))?;
        let access = if let (Some(authority), Some(tx)) = (authority.as_ref(), actor_tx.as_mut()) {
            Some(
                skill_access_for_user(tx, authority.user_id, authority.namespace_id, resource_id)
                    .await?,
            )
        } else {
            None
        };
        let released: bool = row.get(5);
        let readable = match access.as_ref() {
            Some(access) => {
                user_can_read_revision(
                    actor_tx.as_mut().ok_or_else(|| {
                        ApiError::new(ApiErrorCode::Internal, "actor transaction missing")
                    })?,
                    access,
                    resource_id,
                    revision.as_bytes(),
                )
                .await?
            }
            None => row.get::<String, _>(0) == "public" && released,
        };
        if !readable {
            return Err(ApiError::new(ApiErrorCode::NotFound, "revision not found"));
        }
        let manifest: PublicSkillManifest = serde_json::from_value(row.get(1))
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let key: String = row.get(2);
        let sha = decode_32(&row.get::<Vec<u8>, _>(3), "stored snapshot SHA-256")?;
        let size: i64 = row.get(4);
        let parent_revision_ids = if let Some(tx) = actor_tx.as_mut() {
            revision_parents_tx(tx, revision_id).await?
        } else {
            revision_parents(&self.pool, revision_id).await?
        };
        if let Some(tx) = actor_tx {
            tx.commit().await.map_err(internal_api_error)?;
        }
        let url = self
            .objects
            .presign_get(&key)
            .await
            .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
        Ok(SkillRevisionDetail {
            resource_id: resource_id.to_string(),
            locator: format!("@{}/{}", resolved.owner, resolved.name),
            revision_id: revision_id.to_owned(),
            parent_revision_ids,
            manifest,
            snapshot: SnapshotDownload {
                sha256: hex::encode(sha),
                size_bytes: u64::try_from(size).map_err(|_| {
                    ApiError::new(ApiErrorCode::Internal, "stored snapshot size is invalid")
                })?,
                url,
            },
        })
    }

    async fn history_release_rows(&self, resource_id: Uuid) -> Result<Vec<SkillRelease>, ApiError> {
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let releases = history_release_rows_tx(&mut tx, resource_id).await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(releases)
    }
}

async fn history_release_rows_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
) -> Result<Vec<SkillRelease>, ApiError> {
    let rows = sqlx::query(
            "SELECT sr.version,sr.revision_id,sr.message,COALESCE(array_agg(srt.tag ORDER BY srt.tag) FILTER (WHERE srt.tag IS NOT NULL),'{}'::text[]) \
             FROM skill_releases sr LEFT JOIN skill_release_tags srt ON srt.resource_id=sr.resource_id AND srt.version=sr.version \
             WHERE sr.resource_id=$1 \
               AND NOT EXISTS(SELECT 1 FROM resource_quarantines rq \
                 WHERE rq.resource_id=sr.resource_id AND rq.lifted_at IS NULL \
                   AND (rq.release_version IS NULL OR rq.release_version=sr.version)) \
             GROUP BY sr.version,sr.revision_id,sr.message ORDER BY sr.version",
        )
        .bind(resource_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    rows.into_iter()
        .map(|row| {
            Ok(SkillRelease {
                version: u64::try_from(row.get::<i64, _>(0)).map_err(|_| {
                    ApiError::new(ApiErrorCode::Internal, "stored release version is invalid")
                })?,
                revision_id: hex::encode(decode_32(
                    &row.get::<Vec<u8>, _>(1),
                    "stored revision ID",
                )?),
                message: row.get(2),
                tags: row.get::<Vec<String>, _>(3),
            })
        })
        .collect()
}

async fn workspace_head_for_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    user_id: Uuid,
) -> Result<String, ApiError> {
    let bytes = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT revision_id FROM skill_private_workspaces WHERE resource_id=$1 AND workspace_user_id=$2",
    )
    .bind(resource_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?
    .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "private workspace not found"))?;
    decode_32(&bytes, "stored revision ID").map(hex::encode)
}

async fn owner_workspace_head(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
) -> Result<Option<String>, ApiError> {
    let bytes = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT w.revision_id FROM resources r JOIN users u ON u.namespace_id=r.owner_namespace_id \
         JOIN skill_private_workspaces w ON w.resource_id=r.id AND w.workspace_user_id=u.id WHERE r.id=$1",
    )
    .bind(resource_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    bytes
        .map(|bytes| decode_32(&bytes, "stored revision ID").map(hex::encode))
        .transpose()
}

fn latest_release_head(releases: &[SkillRelease]) -> Result<String, ApiError> {
    releases
        .last()
        .map(|release| release.revision_id.clone())
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "skill has no release"))
}
