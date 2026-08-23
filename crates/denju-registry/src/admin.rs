use std::collections::BTreeMap;
use std::str::FromStr;

use denju_core::{OperationId, ResourceKind, ResourceLocator};
use denju_wire::{
    AdminOperatorCredential, AdminOperatorRevokeResponse, AdminQuarantineMutationKind,
    AdminQuarantineRequest, AdminQuarantineResponse, AdminReport, AdminReportList,
    AdminResourceTarget, ApiError, ApiErrorCode, RequestHash, admin_quarantine_request_hash,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Postgres, Row, Transaction, postgres::PgConnection};
use uuid::Uuid;

use crate::{
    Registry, internal_api_error,
    lifecycle::{generation_u64, next_generation, resolve_active_skill_locator_tx},
    outbox::enqueue_resource_wake_with_event,
    pack_storage::load_pack_by_locator,
};

const REPORT_PAGE_MAX: u32 = 100;

#[derive(Debug, Clone)]
pub(crate) struct OperatorAuthority {
    pub(crate) operator_id: Uuid,
    pub(crate) name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReportCursor {
    created_at_unix_micros: i64,
    report_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveQuarantine {
    pub(crate) quarantine_id: Uuid,
    pub(crate) release_version: Option<i64>,
    pub(crate) reason: String,
}

impl Registry {
    pub async fn bootstrap_operator(
        database_url: &str,
        name: &str,
    ) -> Result<AdminOperatorCredential, ApiError> {
        let name = normalize_operator_name(name)?;
        let secret = rand::random::<[u8; 32]>();
        let token = format!("denju_op_{}", hex::encode(secret));
        let token_hash = hash_bearer(&token);
        let operator_id = Uuid::now_v7();
        let mut connection = PgConnection::connect(database_url)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("INSERT INTO operator_tokens (id,name,token_hash) VALUES ($1,$2,$3)")
            .bind(operator_id)
            .bind(&name)
            .bind(token_hash.as_slice())
            .execute(&mut connection)
            .await
            .map_err(|error| match &error {
                sqlx::Error::Database(database) if database.is_unique_violation() => ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "an active or historical operator with that name already exists",
                ),
                _ => internal_api_error(error),
            })?;
        Ok(AdminOperatorCredential {
            operator_id: operator_id.to_string(),
            name,
            token,
        })
    }

    pub async fn revoke_operator(
        database_url: &str,
        operator_id: &str,
    ) -> Result<AdminOperatorRevokeResponse, ApiError> {
        let operator_id = parse_uuid(operator_id, "operator ID")?;
        let mut connection = PgConnection::connect(database_url)
            .await
            .map_err(internal_api_error)?;
        let changed = sqlx::query(
            "UPDATE operator_tokens SET revoked_at=COALESCE(revoked_at,now()) WHERE id=$1",
        )
        .bind(operator_id)
        .execute(&mut connection)
        .await
        .map_err(internal_api_error)?
        .rows_affected();
        if changed == 0 {
            return Err(ApiError::new(ApiErrorCode::NotFound, "operator not found"));
        }
        Ok(AdminOperatorRevokeResponse {
            operator_id: operator_id.to_string(),
            revoked: true,
        })
    }

    pub async fn admin_reports(
        &self,
        bearer: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<AdminReportList, ApiError> {
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let _operator = self.operator_authority_tx(&mut tx, bearer).await?;
        let limit = limit.clamp(1, REPORT_PAGE_MAX);
        let cursor = cursor.map(ReportCursor::decode).transpose()?;
        let cursor_id = cursor
            .as_ref()
            .map(|cursor| parse_uuid(&cursor.report_id, "report cursor"))
            .transpose()?
            .unwrap_or_else(Uuid::nil);
        let cursor_time = cursor
            .as_ref()
            .map_or(i64::MAX, |cursor| cursor.created_at_unix_micros);
        let rows = sqlx::query(
            "SELECT rr.id,rr.resource_id,COALESCE(n.slug,r.deleted_owner_slug),r.slug,r.kind,r.generation,rr.reason, \
                    (extract(epoch FROM rr.created_at)*1000000)::bigint AS created_micros \
             FROM resource_reports rr JOIN resources r ON r.id=rr.resource_id \
             LEFT JOIN namespaces n ON n.id=r.owner_namespace_id \
             WHERE ($1::boolean=FALSE OR (rr.created_at < to_timestamp($2::double precision/1000000.0) \
                    OR (rr.created_at=to_timestamp($2::double precision/1000000.0) AND rr.id<$3))) \
             ORDER BY rr.created_at DESC,rr.id DESC LIMIT $4",
        )
        .bind(cursor.is_some())
        .bind(cursor_time)
        .bind(cursor_id)
        .bind(i64::from(limit) + 1)
        .fetch_all(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        tx.commit().await.map_err(internal_api_error)?;
        let has_more = rows.len() > limit as usize;
        let visible = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
        let reports = visible
            .iter()
            .map(report_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if has_more {
            visible.last().map(|row| {
                ReportCursor {
                    created_at_unix_micros: row.get(7),
                    report_id: row.get::<Uuid, _>(0).to_string(),
                }
                .encode()
            })
        } else {
            None
        };
        Ok(AdminReportList {
            reports,
            next_cursor,
        })
    }

    pub async fn admin_resolve_resource(
        &self,
        bearer: &str,
        locator: &str,
    ) -> Result<AdminResourceTarget, ApiError> {
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let _operator = self.operator_authority_tx(&mut tx, bearer).await?;
        let target = self.resolve_admin_resource(&mut tx, locator).await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(target)
    }

    pub async fn mutate_quarantine(
        &self,
        bearer: &str,
        kind: AdminQuarantineMutationKind,
        request: &AdminQuarantineRequest,
    ) -> Result<AdminQuarantineResponse, ApiError> {
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let resource_id = parse_uuid(&request.resource_id, "resource ID")?;
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let expected_hash = admin_quarantine_request_hash(
            kind,
            &request.operation_id,
            &request.resource_id,
            request.expected_generation,
            request.release_version,
            &request.reason,
        )
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        if supplied_hash != expected_hash {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequestHash,
                "request_hash does not match the canonical admin quarantine payload",
            ));
        }
        let reason = request.reason.trim();
        if kind == AdminQuarantineMutationKind::Quarantine
            && (reason.is_empty() || reason.chars().count() > 500)
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "quarantine reason must contain 1-500 characters",
            ));
        }

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let operator = self.operator_authority_tx(&mut tx, bearer).await?;
        if let Some(outcome) = replay_admin_operation::<AdminQuarantineResponse>(
            &mut tx,
            operator.operator_id,
            operation_id,
            supplied_hash,
            operation_kind(kind),
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }

        let row = sqlx::query(
            "SELECT COALESCE(n.slug,r.deleted_owner_slug),r.slug,r.kind,r.generation,r.latest_release_version \
             FROM resources r LEFT JOIN namespaces n ON n.id=r.owner_namespace_id \
             WHERE r.id=$1 FOR UPDATE OF r",
        )
        .bind(resource_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "resource not found"))?;
        let owner: Option<String> = row.get(0);
        let name: String = row.get(1);
        let resource_kind: String = row.get(2);
        let generation: i64 = row.get(3);
        let latest_release: Option<i64> = row.get(4);
        if generation_u64(generation)? != request.expected_generation {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                "resource generation changed; resolve the resource and retry",
            ));
        }
        if let Some(version) = request.release_version {
            if resource_kind != "skill" {
                return Err(ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "exact-release quarantine applies only to skills",
                ));
            }
            let version = i64::try_from(version).map_err(|_| {
                ApiError::new(ApiErrorCode::InvalidRequest, "release version is invalid")
            })?;
            let exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM skill_releases WHERE resource_id=$1 AND version=$2)",
            )
            .bind(resource_id)
            .bind(version)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if !exists {
                return Err(ApiError::new(ApiErrorCode::NotFound, "release not found"));
            }
        }

        let release_version = request
            .release_version
            .map(i64::try_from)
            .transpose()
            .map_err(|_| {
                ApiError::new(ApiErrorCode::InvalidRequest, "release version is invalid")
            })?;
        let active = active_quarantine_tx(&mut tx, resource_id, release_version).await?;
        let mut changed = false;
        let (quarantine_id, quarantined) = match kind {
            AdminQuarantineMutationKind::Quarantine => {
                if let Some(active) = active.as_ref() {
                    (Some(active.quarantine_id), true)
                } else {
                    let id = Uuid::now_v7();
                    sqlx::query(
                        "INSERT INTO resource_quarantines \
                         (id,resource_id,release_version,reason,created_by_operator_id) VALUES ($1,$2,$3,$4,$5)",
                    )
                    .bind(id)
                    .bind(resource_id)
                    .bind(release_version)
                    .bind(reason)
                    .bind(operator.operator_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(internal_api_error)?;
                    changed = true;
                    (Some(id), true)
                }
            }
            AdminQuarantineMutationKind::Unquarantine => {
                if let Some(active) = active.as_ref() {
                    sqlx::query(
                        "UPDATE resource_quarantines SET lifted_by_operator_id=$1,lifted_at=now() \
                         WHERE id=$2 AND lifted_at IS NULL",
                    )
                    .bind(operator.operator_id)
                    .bind(active.quarantine_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(internal_api_error)?;
                    changed = true;
                    (Some(active.quarantine_id), false)
                } else {
                    (None, false)
                }
            }
        };

        let next = if changed {
            next_generation(generation)?
        } else {
            generation
        };
        if changed {
            sqlx::query("UPDATE resources SET generation=$1 WHERE id=$2")
                .bind(next)
                .bind(resource_id)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            enqueue_resource_wake_with_event(
                &mut tx,
                resource_id,
                generation_u64(next)?,
                if quarantined {
                    "resource_quarantined"
                } else {
                    "resource_unquarantined"
                },
            )
            .await?;
        }
        let locator = match resource_kind.as_str() {
            "skill" => format!("@{}/{}", owner.unwrap_or_else(|| "deleted".into()), name),
            "pack" => format!(
                "@{}/packs/{}",
                owner.unwrap_or_else(|| "deleted".into()),
                name
            ),
            _ => {
                return Err(ApiError::new(
                    ApiErrorCode::Internal,
                    "stored resource kind is invalid",
                ));
            }
        };
        let outcome = AdminQuarantineResponse {
            quarantine_id: quarantine_id.map(|id| id.to_string()),
            resource_id: resource_id.to_string(),
            locator,
            release_version: request.release_version,
            quarantined,
            generation: generation_u64(next)?,
        };
        record_admin_operation(
            &mut tx,
            operator.operator_id,
            operation_id,
            supplied_hash,
            operation_kind(kind),
            &outcome,
        )
        .await?;
        sqlx::query(
            "INSERT INTO operator_audit_log \
             (operator_id,action,resource_id,release_version,detail_json) VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(operator.operator_id)
        .bind(if changed {
            operation_kind(kind)
        } else {
            match kind {
                AdminQuarantineMutationKind::Quarantine => "quarantine_noop",
                AdminQuarantineMutationKind::Unquarantine => "unquarantine_noop",
            }
        })
        .bind(resource_id)
        .bind(release_version)
        .bind(serde_json::json!({
            "operator_name": operator.name,
            "reason": reason,
            "latest_release_version": latest_release,
        }))
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        tx.commit().await.map_err(internal_api_error)?;
        if changed {
            let _ = self.drain_outbox(64).await;
        }
        Ok(outcome)
    }

    async fn operator_authority_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        bearer: &str,
    ) -> Result<OperatorAuthority, ApiError> {
        let hash = hash_bearer(bearer);
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT operator_id,operator_name FROM denju_authenticate_operator($1)",
        )
        .bind(hash.as_slice())
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?
        .map(|(operator_id, name)| OperatorAuthority { operator_id, name })
        .ok_or_else(|| ApiError::new(ApiErrorCode::Unauthorized, "operator credential rejected"))
    }

    async fn resolve_admin_resource(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        locator: &str,
    ) -> Result<AdminResourceTarget, ApiError> {
        if let Ok(resource_id) = Uuid::parse_str(locator) {
            return self.resolve_admin_resource_id(tx, resource_id).await;
        }
        let parsed = ResourceLocator::from_str(locator)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        match parsed.kind() {
            ResourceKind::Skill => {
                let resolved = resolve_active_skill_locator_tx(tx, &parsed).await?;
                let row = sqlx::query_as::<_, (String, i64, Option<i64>)>(
                    "SELECT kind,generation,latest_release_version FROM resources WHERE id=$1 AND deleted_at IS NULL",
                )
                .bind(resolved.resource_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(internal_api_error)?;
                Ok(AdminResourceTarget {
                    resource_id: resolved.resource_id.to_string(),
                    locator: format!("@{}/{}", resolved.owner, resolved.name),
                    kind: row.0,
                    generation: generation_u64(row.1)?,
                    latest_release_version: row.2.map(generation_u64).transpose()?,
                })
            }
            ResourceKind::Pack => {
                let pack = load_pack_by_locator(tx, &parsed).await?;
                Ok(AdminResourceTarget {
                    resource_id: pack.id.to_string(),
                    locator: format!("@{}/packs/{}", pack.owner, pack.name),
                    kind: "pack".to_owned(),
                    generation: generation_u64(pack.generation)?,
                    latest_release_version: None,
                })
            }
        }
    }

    async fn resolve_admin_resource_id(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        resource_id: Uuid,
    ) -> Result<AdminResourceTarget, ApiError> {
        let row = sqlx::query_as::<_, (Option<String>, String, String, i64, Option<i64>)>(
            "SELECT COALESCE(n.slug,r.deleted_owner_slug),r.slug,r.kind,r.generation,r.latest_release_version \
             FROM resources r LEFT JOIN namespaces n ON n.id=r.owner_namespace_id WHERE r.id=$1",
        )
        .bind(resource_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "resource not found"))?;
        let owner = row
            .0
            .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "resource owner is missing"))?;
        let locator = match row.2.as_str() {
            "skill" => format!("@{owner}/{}", row.1),
            "pack" => format!("@{owner}/packs/{}", row.1),
            _ => {
                return Err(ApiError::new(
                    ApiErrorCode::Internal,
                    "stored resource kind is invalid",
                ));
            }
        };
        Ok(AdminResourceTarget {
            resource_id: resource_id.to_string(),
            locator,
            kind: row.2,
            generation: generation_u64(row.3)?,
            latest_release_version: row.4.map(generation_u64).transpose()?,
        })
    }
}

pub(crate) async fn active_quarantine(
    pool: &sqlx::PgPool,
    resource_id: Uuid,
    release_version: Option<i64>,
) -> Result<Option<ActiveQuarantine>, ApiError> {
    let row = sqlx::query_as::<_, (Uuid, Option<i64>, String)>(
        "SELECT id,release_version,reason FROM resource_quarantines \
         WHERE resource_id=$1 AND lifted_at IS NULL \
           AND (release_version IS NULL OR release_version=$2) \
         ORDER BY release_version NULLS FIRST LIMIT 1",
    )
    .bind(resource_id)
    .bind(release_version)
    .fetch_optional(pool)
    .await
    .map_err(internal_api_error)?;
    Ok(row.map(
        |(quarantine_id, release_version, reason)| ActiveQuarantine {
            quarantine_id,
            release_version,
            reason,
        },
    ))
}

pub(crate) async fn active_quarantines_for_resources(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_ids: &[Uuid],
) -> Result<BTreeMap<Uuid, Vec<ActiveQuarantine>>, ApiError> {
    if resource_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let rows = sqlx::query_as::<_, (Uuid, Uuid, Option<i64>, String)>(
        "SELECT id,resource_id,release_version,reason FROM resource_quarantines \
         WHERE lifted_at IS NULL AND resource_id=ANY($1) \
         ORDER BY resource_id,release_version NULLS FIRST,id",
    )
    .bind(resource_ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let mut by_resource = BTreeMap::new();
    for (quarantine_id, resource_id, release_version, reason) in rows {
        by_resource
            .entry(resource_id)
            .or_insert_with(Vec::new)
            .push(ActiveQuarantine {
                quarantine_id,
                release_version,
                reason,
            });
    }
    Ok(by_resource)
}

pub(crate) async fn active_quarantine_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    release_version: Option<i64>,
) -> Result<Option<ActiveQuarantine>, ApiError> {
    let row = sqlx::query_as::<_, (Uuid, Option<i64>, String)>(
        "SELECT id,release_version,reason FROM resource_quarantines \
         WHERE resource_id=$1 AND lifted_at IS NULL \
           AND ((release_version IS NULL AND $2::bigint IS NULL) OR release_version=$2)",
    )
    .bind(resource_id)
    .bind(release_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(row.map(
        |(quarantine_id, release_version, reason)| ActiveQuarantine {
            quarantine_id,
            release_version,
            reason,
        },
    ))
}

pub(crate) async fn effective_quarantine_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    resource_id: Uuid,
    release_version: Option<i64>,
) -> Result<Option<ActiveQuarantine>, ApiError> {
    let row = sqlx::query_as::<_, (Uuid, Option<i64>, String)>(
        "SELECT id,release_version,reason FROM resource_quarantines \
         WHERE resource_id=$1 AND lifted_at IS NULL \
           AND (release_version IS NULL OR release_version=$2) \
         ORDER BY release_version NULLS FIRST LIMIT 1",
    )
    .bind(resource_id)
    .bind(release_version)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(row.map(
        |(quarantine_id, release_version, reason)| ActiveQuarantine {
            quarantine_id,
            release_version,
            reason,
        },
    ))
}

async fn replay_admin_operation<T: DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operator_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    kind: &str,
) -> Result<Option<T>, ApiError> {
    let row = sqlx::query_as::<_, (Vec<u8>, String, serde_json::Value)>(
        "SELECT request_hash,operation_kind,outcome_json FROM admin_operations \
         WHERE operator_id=$1 AND operation_id=$2",
    )
    .bind(operator_id)
    .bind(operation_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let Some((stored_hash, stored_kind, outcome)) = row else {
        return Ok(None);
    };
    if stored_hash.as_slice() != request_hash.as_bytes() || stored_kind != kind {
        return Err(ApiError::new(
            ApiErrorCode::OperationConflict,
            "operation_id was already used with different admin mutation content",
        ));
    }
    serde_json::from_value(outcome)
        .map(Some)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}

async fn record_admin_operation<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operator_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    kind: &str,
    outcome: &T,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO admin_operations \
         (operator_id,operation_id,request_hash,operation_kind,outcome_json) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(operator_id)
    .bind(operation_id.as_uuid())
    .bind(request_hash.as_bytes().as_slice())
    .bind(kind)
    .bind(
        serde_json::to_value(outcome)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?,
    )
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

fn operation_kind(kind: AdminQuarantineMutationKind) -> &'static str {
    match kind {
        AdminQuarantineMutationKind::Quarantine => "quarantine",
        AdminQuarantineMutationKind::Unquarantine => "unquarantine",
    }
}

fn hash_bearer(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn normalize_operator_name(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 64 {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "operator name must contain 1-64 characters",
        ));
    }
    Ok(value.to_owned())
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, format!("{field}: {error}")))
}

fn report_row(row: &sqlx::postgres::PgRow) -> Result<AdminReport, ApiError> {
    let resource_id: Uuid = row.get(1);
    let owner: Option<String> = row.get(2);
    let name: String = row.get(3);
    let kind: String = row.get(4);
    let locator = match kind.as_str() {
        "skill" => format!("@{}/{}", owner.unwrap_or_else(|| "deleted".into()), name),
        "pack" => format!(
            "@{}/packs/{}",
            owner.unwrap_or_else(|| "deleted".into()),
            name
        ),
        _ => {
            return Err(ApiError::new(
                ApiErrorCode::Internal,
                "stored resource kind is invalid",
            ));
        }
    };
    let created_micros: i64 = row.get(7);
    Ok(AdminReport {
        report_id: row.get::<Uuid, _>(0).to_string(),
        resource_id: resource_id.to_string(),
        locator,
        resource_generation: generation_u64(row.get(5))?,
        reason: row.get(6),
        created_at_unix_seconds: created_micros / 1_000_000,
    })
}

impl ReportCursor {
    fn encode(&self) -> String {
        hex::encode(
            serde_json::to_vec(self).expect("admin report cursor serialization is infallible"),
        )
    }

    fn decode(value: &str) -> Result<Self, ApiError> {
        let bytes = hex::decode(value)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid report cursor"))?;
        serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid report cursor"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_cursor_round_trip_is_opaque_and_stable() {
        let cursor = ReportCursor {
            created_at_unix_micros: 1_234_567,
            report_id: Uuid::now_v7().to_string(),
        };
        let decoded = ReportCursor::decode(&cursor.encode()).unwrap();
        assert_eq!(
            decoded.created_at_unix_micros,
            cursor.created_at_unix_micros
        );
        assert_eq!(decoded.report_id, cursor.report_id);
    }
}
