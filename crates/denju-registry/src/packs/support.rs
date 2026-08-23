use std::{collections::BTreeSet, str::FromStr};

use denju_core::{OperationId, ResourceId};
use denju_wire::{
    ApiError, ApiErrorCode, PackMemberTarget, PackSubscriptionMutationKind, RequestHash,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Row;
use uuid::Uuid;

use crate::{identity_support::SubscriptionSubject, internal_api_error, lifecycle::generation_u64};

pub(super) fn validate_unique_members(members: &[PackMemberTarget]) -> Result<(), ApiError> {
    let mut seen = BTreeSet::new();
    for member in members {
        if !seen.insert(member.resource_id.as_str()) {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "pack mutation contains the same skill more than once",
            ));
        }
    }
    Ok(())
}

pub(super) async fn mutate_generic_subscription_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubscriptionSubject,
    resource_id: Uuid,
    kind: PackSubscriptionMutationKind,
) -> Result<(), ApiError> {
    match (subject, kind) {
        (SubscriptionSubject::Installation(id), PackSubscriptionMutationKind::Subscribe) => {
            sqlx::query(
                "INSERT INTO installation_subscriptions (installation_id,resource_id,pinned_release_version,retain_on_delete) \
                 VALUES ($1,$2,NULL,FALSE) ON CONFLICT(installation_id,resource_id) DO NOTHING",
            )
            .bind(id)
            .bind(resource_id)
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
        }
        (SubscriptionSubject::Installation(id), PackSubscriptionMutationKind::Unsubscribe) => {
            sqlx::query("DELETE FROM installation_subscriptions WHERE installation_id=$1 AND resource_id=$2")
                .bind(id)
                .bind(resource_id)
                .execute(&mut **tx)
                .await
                .map_err(internal_api_error)?;
        }
        (SubscriptionSubject::User(id), PackSubscriptionMutationKind::Subscribe) => {
            sqlx::query(
                "INSERT INTO account_subscriptions (user_id,resource_id,pinned_release_version,retain_on_delete) \
                 VALUES ($1,$2,NULL,FALSE) ON CONFLICT(user_id,resource_id) DO NOTHING",
            )
            .bind(id)
            .bind(resource_id)
            .execute(&mut **tx)
            .await
            .map_err(internal_api_error)?;
        }
        (SubscriptionSubject::User(id), PackSubscriptionMutationKind::Unsubscribe) => {
            sqlx::query("DELETE FROM account_subscriptions WHERE user_id=$1 AND resource_id=$2")
                .bind(id)
                .bind(resource_id)
                .execute(&mut **tx)
                .await
                .map_err(internal_api_error)?;
        }
    }
    Ok(())
}

pub(crate) async fn replay_pack_operation<T: DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    kind: &str,
) -> Result<Option<T>, ApiError> {
    let row = sqlx::query(
        "SELECT request_hash,operation_kind,outcome_json FROM pack_operations WHERE user_id=$1 AND operation_id=$2",
    )
    .bind(user_id)
    .bind(operation_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    let Some(row) = row else { return Ok(None) };
    let stored_hash: Vec<u8> = row.get(0);
    let stored_kind: String = row.get(1);
    if stored_hash.as_slice() != request_hash.as_bytes() || stored_kind != kind {
        return Err(ApiError::new(
            ApiErrorCode::OperationConflict,
            "operation_id was already used with different pack content",
        ));
    }
    serde_json::from_value(row.get(2))
        .map(Some)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}

pub(crate) async fn record_pack_operation<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    resource_id: Uuid,
    kind: &str,
    outcome: &T,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO pack_operations (user_id,operation_id,request_hash,resource_id,operation_kind,outcome_json) \
         VALUES ($1,$2,$3,$4,$5,$6)",
    )
    .bind(user_id)
    .bind(operation_id.as_uuid())
    .bind(request_hash.as_bytes().as_slice())
    .bind(resource_id)
    .bind(kind)
    .bind(serde_json::to_value(outcome).map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?)
    .execute(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    Ok(())
}

pub(super) async fn replay_subscription_operation<T: DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubscriptionSubject,
    operation_id: OperationId,
    request_hash: RequestHash,
    action: &str,
) -> Result<Option<T>, ApiError> {
    let row = match subject {
        SubscriptionSubject::Installation(id) => sqlx::query(
            "SELECT request_hash,action,pack_outcome_json FROM subscription_operations WHERE installation_id=$1 AND operation_id=$2",
        )
        .bind(id)
        .bind(operation_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?,
        SubscriptionSubject::User(id) => sqlx::query(
            "SELECT request_hash,action,pack_outcome_json FROM account_subscription_operations WHERE user_id=$1 AND operation_id=$2",
        )
        .bind(id)
        .bind(operation_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal_api_error)?,
    };
    let Some(row) = row else { return Ok(None) };
    let stored_hash: Vec<u8> = row.get(0);
    let stored_action: String = row.get(1);
    if stored_hash.as_slice() != request_hash.as_bytes() || stored_action != action {
        return Err(ApiError::new(
            ApiErrorCode::OperationConflict,
            "operation_id was already used with different subscription content",
        ));
    }
    let outcome: Option<serde_json::Value> = row.get(2);
    let outcome = outcome.ok_or_else(|| {
        ApiError::new(
            ApiErrorCode::Internal,
            "pack subscription replay is missing its committed outcome",
        )
    })?;
    serde_json::from_value(outcome)
        .map(Some)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}

pub(super) async fn record_subscription_operation<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubscriptionSubject,
    operation_id: OperationId,
    request_hash: RequestHash,
    action: &str,
    resource_id: Uuid,
    outcome: &T,
) -> Result<(), ApiError> {
    let subscribed = serde_json::to_value(outcome)
        .ok()
        .and_then(|value| value.get("subscribed").and_then(serde_json::Value::as_bool))
        .ok_or_else(|| ApiError::new(ApiErrorCode::Internal, "subscription outcome is invalid"))?;
    match subject {
        SubscriptionSubject::Installation(id) => sqlx::query(
            "INSERT INTO subscription_operations \
             (installation_id,operation_id,request_hash,action,resource_id,subscribed,pinned_release_version,retain_on_delete,pack_outcome_json) \
             VALUES ($1,$2,$3,$4,$5,$6,NULL,FALSE,$7)",
        )
        .bind(id)
        .bind(operation_id.as_uuid())
        .bind(request_hash.as_bytes().as_slice())
        .bind(action)
        .bind(resource_id)
        .bind(subscribed)
        .bind(serde_json::to_value(outcome).map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?)
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?,
        SubscriptionSubject::User(id) => sqlx::query(
            "INSERT INTO account_subscription_operations \
             (user_id,operation_id,request_hash,action,resource_id,subscribed,pinned_release_version,retain_on_delete,pack_outcome_json) \
             VALUES ($1,$2,$3,$4,$5,$6,NULL,FALSE,$7)",
        )
        .bind(id)
        .bind(operation_id.as_uuid())
        .bind(request_hash.as_bytes().as_slice())
        .bind(action)
        .bind(resource_id)
        .bind(subscribed)
        .bind(serde_json::to_value(outcome).map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?)
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?,
    };
    Ok(())
}

pub(super) fn parse_operation(value: &str) -> Result<OperationId, ApiError> {
    OperationId::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
}

pub(super) fn parse_resource(value: &str) -> Result<ResourceId, ApiError> {
    ResourceId::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
}

pub(super) fn parse_hash(value: &str) -> Result<RequestHash, ApiError> {
    RequestHash::from_str(value).map_err(hash_error)
}

pub(super) fn hash_error(error: denju_wire::RequestHashError) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string())
}

pub(super) fn ensure_hash(actual: RequestHash, expected: RequestHash) -> Result<(), ApiError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::InvalidRequestHash,
            "request_hash does not match the canonical request payload",
        ))
    }
}

pub(super) fn ensure_generation(stored: i64, expected: u64) -> Result<(), ApiError> {
    if generation_u64(stored)? == expected {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::GenerationConflict,
            format!("resource generation changed to {}", generation_u64(stored)?),
        ))
    }
}

pub(super) fn i64_version(value: Option<u64>) -> Result<Option<i64>, ApiError> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|_| {
                ApiError::new(ApiErrorCode::InvalidRequest, "release version is too large")
            })
        })
        .transpose()
}
