use denju_wire::ApiError;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{Registry, internal_api_error};

impl Registry {
    pub(crate) async fn begin_actor_tx(
        &self,
        user_id: Uuid,
    ) -> Result<Transaction<'_, Postgres>, ApiError> {
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        set_actor_user(&mut tx, user_id).await?;
        Ok(tx)
    }

    pub(crate) async fn begin_installation_actor_tx(
        &self,
        installation_id: Uuid,
    ) -> Result<Transaction<'_, Postgres>, ApiError> {
        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        set_actor_installation(&mut tx, installation_id).await?;
        Ok(tx)
    }

    pub(crate) async fn begin_worker_tx(&self) -> Result<Transaction<'_, Postgres>, ApiError> {
        self.worker_pool.begin().await.map_err(internal_api_error)
    }
}

pub(crate) async fn set_actor_user(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("SELECT set_config('denju.actor_user_id',$1,true)")
        .bind(user_id.to_string())
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    Ok(())
}

pub(crate) async fn set_actor_installation(
    tx: &mut Transaction<'_, Postgres>,
    installation_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("SELECT set_config('denju.actor_installation_id',$1,true)")
        .bind(installation_id.to_string())
        .execute(&mut **tx)
        .await
        .map_err(internal_api_error)?;
    Ok(())
}
