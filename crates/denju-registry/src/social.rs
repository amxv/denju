use std::str::FromStr;

use denju_core::OperationId;
use denju_wire::{
    ApiError, ApiErrorCode, FollowMutationKind, FollowMutationRequest, FollowMutationResponse,
    ProfileUpdateRequest, ProfileUpdateResponse, ReportResourceRequest, ReportResourceResponse,
    RequestHash, ResourceTopicsRequest, ResourceTopicsResponse, StarMutationKind,
    StarMutationRequest, StarMutationResponse, follow_request_hash, profile_update_request_hash,
    report_resource_request_hash, resource_topics_request_hash, star_request_hash,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    Registry,
    identity_support::invalid_credentials,
    internal_api_error,
    lifecycle::{generation_u64, next_generation},
    outbox::enqueue_resource_wake,
    team_access::authorize_resource_publish,
};

impl Registry {
    pub async fn update_profile(
        &self,
        bearer: &str,
        request: &ProfileUpdateRequest,
    ) -> Result<ProfileUpdateResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            profile_update_request_hash(
                &request.operation_id,
                request.bio.as_deref(),
                request.followers_visible,
                request.following_visible,
            )
            .map_err(hash_error)?,
        )?;
        let bio = request
            .bio
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if bio
            .as_ref()
            .is_some_and(|value| value.chars().count() > 500)
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "profile bio must contain at most 500 characters",
            ));
        }
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        lock_live_social_user(&mut tx, authority.user_id).await?;
        if let Some(outcome) = replay_social_operation::<ProfileUpdateResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "profile_update",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        sqlx::query(
            "UPDATE users SET bio=$1,followers_visible=$2,following_visible=$3 WHERE id=$4 AND deleted_at IS NULL",
        )
        .bind(&bio)
        .bind(request.followers_visible)
        .bind(request.following_visible)
        .bind(authority.user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let outcome = ProfileUpdateResponse {
            user_id: authority.user_id.to_string(),
            username: format!("@{}", authority.namespace_slug),
            bio,
            followers_visible: request.followers_visible,
            following_visible: request.following_visible,
        };
        record_social_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "profile_update",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn mutate_follow(
        &self,
        bearer: &str,
        kind: FollowMutationKind,
        request: &FollowMutationRequest,
    ) -> Result<FollowMutationResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let target = parse_uuid(&request.target_user_id, "target user ID")?;
        if target == authority.user_id {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "users cannot follow themselves",
            ));
        }
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            follow_request_hash(kind, &request.operation_id, &request.target_user_id)
                .map_err(hash_error)?,
        )?;
        let operation_kind = match kind {
            FollowMutationKind::Follow => "follow",
            FollowMutationKind::Unfollow => "unfollow",
        };
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        lock_live_social_user(&mut tx, authority.user_id).await?;
        if let Some(outcome) = replay_social_operation::<FollowMutationResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            operation_kind,
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let username =
            sqlx::query_scalar::<_, String>("SELECT username FROM denju_lock_live_social_user($1)")
                .bind(target)
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal_api_error)?
                .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "user not found"))?;
        let following = match kind {
            FollowMutationKind::Follow => {
                sqlx::query(
                    "INSERT INTO user_follows (follower_user_id,followed_user_id) VALUES ($1,$2) \
                     ON CONFLICT DO NOTHING",
                )
                .bind(authority.user_id)
                .bind(target)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
                true
            }
            FollowMutationKind::Unfollow => {
                sqlx::query(
                    "DELETE FROM user_follows WHERE follower_user_id=$1 AND followed_user_id=$2",
                )
                .bind(authority.user_id)
                .bind(target)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
                false
            }
        };
        let outcome = FollowMutationResponse {
            target_user_id: target.to_string(),
            username: format!("@{username}"),
            following,
        };
        record_social_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            operation_kind,
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn mutate_star(
        &self,
        bearer: &str,
        kind: StarMutationKind,
        request: &StarMutationRequest,
    ) -> Result<StarMutationResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let resource_id = parse_uuid(&request.resource_id, "resource ID")?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            star_request_hash(kind, &request.operation_id, &request.resource_id)
                .map_err(hash_error)?,
        )?;
        let operation_kind = match kind {
            StarMutationKind::Star => "star",
            StarMutationKind::Unstar => "unstar",
        };
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        lock_live_social_user(&mut tx, authority.user_id).await?;
        if let Some(outcome) = replay_social_operation::<StarMutationResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            operation_kind,
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let row = sqlx::query(
            "SELECT owner_slug,resource_slug,resource_kind,visibility,deleted,released,star_count \
             FROM denju_lock_social_resource($1)",
        )
        .bind(resource_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "skill not found"))?;
        let owner: Option<String> = row.get(0);
        let name: String = row.get(1);
        let resource_kind: String = row.get(2);
        let visibility: String = row.get(3);
        let deleted: bool = row.get(4);
        let released: bool = row.get(5);
        if resource_kind != "skill" {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "only skills can be starred",
            ));
        }
        if kind == StarMutationKind::Star && (deleted || visibility != "public" || !released) {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "only active public skills can be starred",
            ));
        }
        let changed = match kind {
            StarMutationKind::Star => sqlx::query(
                "INSERT INTO resource_stars (user_id,resource_id) VALUES ($1,$2) ON CONFLICT DO NOTHING",
            )
            .bind(authority.user_id)
            .bind(resource_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?
            .rows_affected()
                == 1,
            StarMutationKind::Unstar => sqlx::query(
                "DELETE FROM resource_stars WHERE user_id=$1 AND resource_id=$2",
            )
            .bind(authority.user_id)
            .bind(resource_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?
            .rows_affected()
                == 1,
        };
        let star_count: i64 = if changed {
            sqlx::query_scalar("SELECT denju_refresh_resource_star_count($1)")
                .bind(resource_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(internal_api_error)?
        } else {
            sqlx::query_scalar("SELECT star_count FROM resources WHERE id=$1")
                .bind(resource_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(internal_api_error)?
        };
        let outcome = StarMutationResponse {
            resource_id: resource_id.to_string(),
            locator: format!("@{}/{}", owner.unwrap_or_else(|| "deleted".into()), name),
            starred: kind == StarMutationKind::Star,
            star_count: generation_u64(star_count)?,
        };
        record_social_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            operation_kind,
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }

    pub async fn update_resource_topics(
        &self,
        bearer: &str,
        request: &ResourceTopicsRequest,
    ) -> Result<ResourceTopicsResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let resource_id = parse_uuid(&request.resource_id, "resource ID")?;
        let topics = normalize_topics(&request.topics)?;
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            resource_topics_request_hash(
                &request.operation_id,
                &request.resource_id,
                request.expected_generation,
                &request.topics,
            )
            .map_err(hash_error)?,
        )?;
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        lock_live_social_user(&mut tx, authority.user_id).await?;
        if let Some(outcome) = replay_social_operation::<ResourceTopicsResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "topics",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        authorize_resource_publish(&mut tx, &authority, resource_id).await?;
        let row = sqlx::query(
            "SELECT n.slug,r.slug,r.generation FROM resources r JOIN namespaces n ON n.id=r.owner_namespace_id \
             WHERE r.id=$1 AND r.deleted_at IS NULL FOR UPDATE OF r",
        )
        .bind(resource_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "resource not found"))?;
        let owner: String = row.get(0);
        let name: String = row.get(1);
        let generation: i64 = row.get(2);
        if generation_u64(generation)? != request.expected_generation {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                "resource generation changed; refresh and retry",
            ));
        }
        let next = next_generation(generation)?;
        sqlx::query("UPDATE resources SET discovery_topics=$1,generation=$2 WHERE id=$3")
            .bind(&topics)
            .bind(next)
            .bind(resource_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        enqueue_resource_wake(&mut tx, resource_id, generation_u64(next)?).await?;
        let outcome = ResourceTopicsResponse {
            resource_id: resource_id.to_string(),
            locator: format!("@{owner}/{name}"),
            generation: generation_u64(next)?,
            topics,
        };
        record_social_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "topics",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.wake_tx.send(crate::RegistryWake::Resource {
            resource_id,
            generation: outcome.generation,
        });
        Ok(outcome)
    }

    pub async fn report_resource(
        &self,
        bearer: &str,
        request: &ReportResourceRequest,
    ) -> Result<ReportResourceResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let resource_id = parse_uuid(&request.resource_id, "resource ID")?;
        let reason = request.reason.trim();
        if reason.is_empty() || reason.chars().count() > 64 {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "report reason must contain 1-64 characters",
            ));
        }
        let operation_id = parse_operation(&request.operation_id)?;
        let supplied = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied,
            report_resource_request_hash(
                &request.operation_id,
                &request.resource_id,
                &request.reason,
            )
            .map_err(hash_error)?,
        )?;
        let mut tx = self.begin_actor_tx(authority.user_id).await?;
        lock_live_social_user(&mut tx, authority.user_id).await?;
        if let Some(outcome) = replay_social_operation::<ReportResourceResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "report",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let public = sqlx::query_scalar::<_, bool>(
            "SELECT visibility='public' AND NOT deleted FROM denju_lock_social_resource($1)",
        )
        .bind(resource_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if public != Some(true) {
            return Err(ApiError::new(
                ApiErrorCode::NotFound,
                "public resource not found",
            ));
        }
        let report_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO resource_reports (id,reporter_user_id,resource_id,reason) VALUES ($1,$2,$3,$4)",
        )
        .bind(report_id)
        .bind(authority.user_id)
        .bind(resource_id)
        .bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        let outcome = ReportResourceResponse {
            report_id: report_id.to_string(),
            resource_id: resource_id.to_string(),
            accepted: true,
        };
        record_social_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied,
            "report",
            &outcome,
        )
        .await?;
        tx.commit().await.map_err(internal_api_error)?;
        Ok(outcome)
    }
}

async fn lock_live_social_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let live = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM users WHERE id=$1 AND deleted_at IS NULL FOR SHARE",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal_api_error)?;
    if live.is_some() {
        Ok(())
    } else {
        Err(invalid_credentials())
    }
}

fn normalize_topics(input: &[String]) -> Result<Vec<String>, ApiError> {
    if input.len() > 12 {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "a resource may have at most 12 discovery topics",
        ));
    }
    let mut topics = input
        .iter()
        .map(|topic| topic.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    topics.sort();
    topics.dedup();
    if topics.iter().any(|topic| {
        topic.is_empty()
            || topic.len() > 32
            || !topic.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || (byte == b'-' && index > 0)
            })
            || topic.ends_with('-')
            || topic.contains("--")
    }) {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "discovery topics must be lowercase letters/numbers with single internal hyphens and at most 32 bytes",
        ));
    }
    Ok(topics)
}

pub(crate) async fn replay_social_operation<T: DeserializeOwned>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    kind: &str,
) -> Result<Option<T>, ApiError> {
    let row = sqlx::query_as::<_, (Vec<u8>, String, serde_json::Value)>(
        "SELECT request_hash,operation_kind,outcome_json FROM social_operations \
         WHERE user_id=$1 AND operation_id=$2",
    )
    .bind(user_id)
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
            "operation_id was already used with different social mutation content",
        ));
    }
    serde_json::from_value(outcome)
        .map(Some)
        .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))
}

pub(crate) async fn record_social_operation<T: Serialize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    operation_id: OperationId,
    request_hash: RequestHash,
    kind: &str,
    outcome: &T,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO social_operations (user_id,operation_id,request_hash,operation_kind,outcome_json) \
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(user_id)
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

fn parse_operation(value: &str) -> Result<OperationId, ApiError> {
    OperationId::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
}

fn parse_hash(value: &str) -> Result<RequestHash, ApiError> {
    RequestHash::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, format!("{field}: {error}")))
}

fn ensure_hash(actual: RequestHash, expected: RequestHash) -> Result<(), ApiError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::InvalidRequestHash,
            "request_hash does not match the canonical social mutation payload",
        ))
    }
}

fn hash_error(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_topics_are_normalized_and_strict() {
        assert_eq!(
            normalize_topics(&[" Rust ".into(), "agents".into(), "rust".into()]).unwrap(),
            vec!["agents", "rust"]
        );
        assert!(normalize_topics(&["bad--topic".into()]).is_err());
        assert!(normalize_topics(&["Ends-".into()]).is_err());
    }
}
