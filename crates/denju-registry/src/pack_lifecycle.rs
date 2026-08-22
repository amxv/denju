use std::str::FromStr;

use denju_core::{OperationId, ResourceId, ResourceKind, ResourceLocator};
use denju_wire::{
    ApiError, ApiErrorCode, PackLifecycleRequest, PackLifecycleResponse, PackRenameRequest,
    RequestHash, pack_delete_request_hash, pack_rename_request_hash, pack_unpublish_request_hash,
};

use crate::{
    Registry, internal_api_error,
    lifecycle::{generation_u64, next_generation},
    outbox::enqueue_resource_wake,
    pack_drain::lock_and_catch_up_pack,
    pack_storage::{PackRow, load_owned_pack_for_update, pack_summary},
    packs::{record_pack_operation, replay_pack_operation},
    team_access::authorize_resource_publish,
};

impl Registry {
    pub async fn rename_pack(
        &self,
        bearer: &str,
        request: &PackRenameRequest,
    ) -> Result<PackLifecycleResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let resource_id = parse_resource(&request.resource_id)?;
        let supplied_hash = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied_hash,
            pack_rename_request_hash(
                &request.operation_id,
                &request.resource_id,
                request.expected_generation,
                &request.new_name,
            )
            .map_err(hash_error)?,
        )?;
        let proposed = ResourceLocator::from_str(&format!(
            "@{}/packs/{}",
            authority.namespace_slug, request.new_name
        ))
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        if proposed.kind() != ResourceKind::Pack {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "invalid pack name",
            ));
        }
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_pack_operation::<PackLifecycleResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            "rename",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let resource_authority =
            authorize_resource_publish(&mut tx, &authority, resource_id.as_uuid()).await?;
        let mut pack = load_owned_pack_for_update(
            &mut tx,
            resource_id.as_uuid(),
            resource_authority.namespace_id,
        )
        .await?;
        if let Some(generation) = catch_up_pack(&mut tx, &mut pack, &[]).await? {
            enqueue_resource_wake(&mut tx, pack.id, generation).await?;
            tx.commit().await.map_err(internal_api_error)?;
            let _ = self.drain_outbox(32).await;
            return Err(pack_advanced_conflict(generation));
        }
        ensure_generation(pack.generation, request.expected_generation)?;
        if pack.name == proposed.name() {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "the pack already has that name",
            ));
        }
        let collision = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM resources WHERE owner_namespace_id=$1 AND kind='pack' AND slug=$2 AND deleted_at IS NULL AND id<>$3)",
        )
        .bind(resource_authority.namespace_id)
        .bind(proposed.name())
        .bind(pack.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        if collision {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("@{}/packs/{} already exists", pack.owner, proposed.name()),
            ));
        }
        let old_locator = format!("@{}/packs/{}", pack.owner, pack.name);
        let old_name = pack.name.clone();
        let next = next_generation(pack.generation)?;
        sqlx::query(
            "DELETE FROM resource_redirects WHERE namespace_id=$1 AND kind='pack' AND old_slug=$2",
        )
        .bind(resource_authority.namespace_id)
        .bind(proposed.name())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query("UPDATE resources SET slug=$1,generation=$2 WHERE id=$3")
            .bind(proposed.name())
            .bind(next)
            .bind(pack.id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO resource_redirects (namespace_id,kind,old_slug,target_resource_id) VALUES ($1,'pack',$2,$3) \
             ON CONFLICT(namespace_id,kind,old_slug) DO UPDATE SET target_resource_id=excluded.target_resource_id,created_at=now()",
        )
        .bind(resource_authority.namespace_id)
        .bind(old_name)
        .bind(pack.id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        pack.name = proposed.name().to_owned();
        pack.generation = next;
        let outcome = PackLifecycleResponse {
            pack: pack_summary(&mut tx, &pack).await?,
            old_locator: Some(old_locator),
            deleted: false,
        };
        record_pack_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            pack.id,
            "rename",
            &outcome,
        )
        .await?;
        enqueue_resource_wake(&mut tx, pack.id, generation_u64(next)?).await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.drain_outbox(32).await;
        Ok(outcome)
    }

    pub async fn unpublish_pack(
        &self,
        bearer: &str,
        request: &PackLifecycleRequest,
    ) -> Result<PackLifecycleResponse, ApiError> {
        self.pack_visibility_lifecycle(bearer, request, false).await
    }

    pub async fn delete_pack(
        &self,
        bearer: &str,
        request: &PackLifecycleRequest,
    ) -> Result<PackLifecycleResponse, ApiError> {
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let resource_id = parse_resource(&request.resource_id)?;
        let supplied_hash = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied_hash,
            pack_delete_request_hash(
                &request.operation_id,
                &request.resource_id,
                request.expected_generation,
            )
            .map_err(hash_error)?,
        )?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_pack_operation::<PackLifecycleResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            "delete",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let resource_authority =
            authorize_resource_publish(&mut tx, &authority, resource_id.as_uuid()).await?;
        if resource_authority.is_team {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "team pack deletion requires the owner-only team lifecycle rules",
            ));
        }
        let mut pack = load_owned_pack_for_update(
            &mut tx,
            resource_id.as_uuid(),
            resource_authority.namespace_id,
        )
        .await?;
        if let Some(generation) = catch_up_pack(&mut tx, &mut pack, &[]).await? {
            enqueue_resource_wake(&mut tx, pack.id, generation).await?;
            tx.commit().await.map_err(internal_api_error)?;
            let _ = self.drain_outbox(32).await;
            return Err(pack_advanced_conflict(generation));
        }
        ensure_generation(pack.generation, request.expected_generation)?;
        let next = next_generation(pack.generation)?;
        sqlx::query(
            "UPDATE resources SET visibility='private',generation=$1,deleted_at=now(),deleted_owner_slug=$2, \
             tombstone_release_version=latest_release_version WHERE id=$3",
        )
        .bind(next)
        .bind(&pack.owner)
        .bind(pack.id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query("DELETE FROM resource_redirects WHERE target_resource_id=$1")
            .bind(pack.id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        // Pack subscriptions never retain a tombstoned pack. Remove the durable desired
        // roots now rather than leaving permanently inactive rows behind.
        sqlx::query("DELETE FROM installation_subscriptions WHERE resource_id=$1")
            .bind(pack.id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query("DELETE FROM account_subscriptions WHERE resource_id=$1")
            .bind(pack.id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        pack.visibility = "private".to_owned();
        pack.generation = next;
        let outcome = PackLifecycleResponse {
            pack: pack_summary(&mut tx, &pack).await?,
            old_locator: None,
            deleted: true,
        };
        record_pack_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            pack.id,
            "delete",
            &outcome,
        )
        .await?;
        enqueue_resource_wake(&mut tx, pack.id, generation_u64(next)?).await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.wake_tx.send(crate::RegistryWake::ResyncAll);
        let _ = self.drain_outbox(32).await;
        Ok(outcome)
    }

    async fn pack_visibility_lifecycle(
        &self,
        bearer: &str,
        request: &PackLifecycleRequest,
        public: bool,
    ) -> Result<PackLifecycleResponse, ApiError> {
        debug_assert!(!public, "publishing packs uses publish_pack");
        let authority = self.user_authority(bearer, "skills:write").await?;
        let operation_id = parse_operation(&request.operation_id)?;
        let resource_id = parse_resource(&request.resource_id)?;
        let supplied_hash = parse_hash(&request.request_hash)?;
        ensure_hash(
            supplied_hash,
            pack_unpublish_request_hash(
                &request.operation_id,
                &request.resource_id,
                request.expected_generation,
            )
            .map_err(hash_error)?,
        )?;
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some(outcome) = replay_pack_operation::<PackLifecycleResponse>(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            "unpublish",
        )
        .await?
        {
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(outcome);
        }
        let resource_authority =
            authorize_resource_publish(&mut tx, &authority, resource_id.as_uuid()).await?;
        let mut pack = load_owned_pack_for_update(
            &mut tx,
            resource_id.as_uuid(),
            resource_authority.namespace_id,
        )
        .await?;
        if let Some(generation) = catch_up_pack(&mut tx, &mut pack, &[]).await? {
            enqueue_resource_wake(&mut tx, pack.id, generation).await?;
            tx.commit().await.map_err(internal_api_error)?;
            let _ = self.drain_outbox(32).await;
            return Err(pack_advanced_conflict(generation));
        }
        ensure_generation(pack.generation, request.expected_generation)?;
        if pack.visibility != "public" {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "the pack is already unpublished",
            ));
        }
        let next = next_generation(pack.generation)?;
        sqlx::query("UPDATE resources SET visibility='private',generation=$1 WHERE id=$2")
            .bind(next)
            .bind(pack.id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        pack.visibility = "private".to_owned();
        pack.generation = next;
        let outcome = PackLifecycleResponse {
            pack: pack_summary(&mut tx, &pack).await?,
            old_locator: None,
            deleted: false,
        };
        record_pack_operation(
            &mut tx,
            authority.user_id,
            operation_id,
            supplied_hash,
            pack.id,
            "unpublish",
            &outcome,
        )
        .await?;
        enqueue_resource_wake(&mut tx, pack.id, generation_u64(next)?).await?;
        tx.commit().await.map_err(internal_api_error)?;
        let _ = self.drain_outbox(32).await;
        Ok(outcome)
    }
}

async fn catch_up_pack(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    pack: &mut PackRow,
    extra_skill_ids: &[uuid::Uuid],
) -> Result<Option<u64>, ApiError> {
    let caught_up = lock_and_catch_up_pack(tx, pack, extra_skill_ids).await?;
    Ok(caught_up.last().copied())
}

fn pack_advanced_conflict(generation: u64) -> ApiError {
    ApiError::new(
        ApiErrorCode::GenerationConflict,
        format!(
            "pack advanced through pending skill releases to generation {generation}; retry from current state"
        ),
    )
}

fn parse_operation(value: &str) -> Result<OperationId, ApiError> {
    OperationId::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
}

fn parse_resource(value: &str) -> Result<ResourceId, ApiError> {
    ResourceId::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))
}

fn parse_hash(value: &str) -> Result<RequestHash, ApiError> {
    RequestHash::from_str(value)
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))
}

fn hash_error(error: denju_wire::RequestHashError) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string())
}

fn ensure_hash(actual: RequestHash, expected: RequestHash) -> Result<(), ApiError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::InvalidRequestHash,
            "request_hash does not match the canonical request payload",
        ))
    }
}

fn ensure_generation(stored: i64, expected: u64) -> Result<(), ApiError> {
    if generation_u64(stored)? == expected {
        Ok(())
    } else {
        Err(ApiError::new(
            ApiErrorCode::GenerationConflict,
            format!("resource generation changed to {}", generation_u64(stored)?),
        ))
    }
}
