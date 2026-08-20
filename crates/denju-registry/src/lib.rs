//! Registry use cases plus PostgreSQL and S3-compatible persistence boundaries.

use std::str::FromStr;

use denju_core::OperationId;
use denju_wire::{
    ApiError, ApiErrorCode, CreateInstallationRequest, CreateInstallationResponse,
    RegistryCapabilities, RegistryLimits, RequestHash, create_installation_request_hash,
};
use reqwest::Client;
use sqlx::{PgPool, postgres::PgPoolOptions};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Clone)]
pub struct RegistrySettings {
    pub database_url: String,
    pub public_origin: Url,
    pub object_store_endpoint: Url,
    pub limits: RegistryLimits,
}

#[derive(Clone)]
pub struct Registry {
    pool: PgPool,
    http: Client,
    public_origin: Url,
    object_store_endpoint: Url,
    limits: RegistryLimits,
}

impl Registry {
    pub async fn connect(settings: RegistrySettings) -> Result<Self, RegistryError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(&settings.database_url)
            .await?;
        Ok(Self {
            pool,
            http: Client::new(),
            public_origin: settings.public_origin,
            object_store_endpoint: settings.object_store_endpoint,
            limits: settings.limits,
        })
    }

    pub async fn migrate(database_url: &str) -> Result<(), RegistryError> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(database_url)
            .await?;
        MIGRATOR.run(&pool).await?;
        pool.close().await;
        Ok(())
    }

    pub fn capabilities(&self) -> RegistryCapabilities {
        RegistryCapabilities {
            api_version: "v1".to_owned(),
            registry_origin: self.public_origin.as_str().trim_end_matches('/').to_owned(),
            object_store_required: true,
            limits: self.limits.clone(),
        }
    }

    pub async fn readiness(&self) -> Result<(), RegistryError> {
        self.validate_schema().await?;
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await?;

        let response = self
            .http
            .get(self.object_store_endpoint.clone())
            .send()
            .await?;
        if response.status().is_server_error() {
            return Err(RegistryError::ObjectStore(format!(
                "object store returned {}",
                response.status()
            )));
        }
        Ok(())
    }

    pub async fn validate_schema(&self) -> Result<(), RegistryError> {
        let version = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM _sqlx_migrations WHERE success = true ORDER BY version DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        if version != Some(1) {
            return Err(RegistryError::SchemaOutOfDate(version));
        }
        Ok(())
    }

    pub async fn create_installation(
        &self,
        request: &CreateInstallationRequest,
    ) -> Result<CreateInstallationResponse, ApiError> {
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let credential_hash = decode_hash(&request.credential_hash, "credential_hash")?;
        let supplied_request_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let expected_request_hash =
            create_installation_request_hash(&request.operation_id, &request.credential_hash)
                .map_err(|error| {
                    ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string())
                })?;
        if supplied_request_hash != expected_request_hash {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequestHash,
                "request_hash does not match the canonical request payload",
            ));
        }

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        if let Some((stored_hash, installation_id, author_principal_id)) =
            sqlx::query_as::<_, (Vec<u8>, Uuid, Uuid)>(
                "SELECT request_hash, installation_id, author_principal_id \
                 FROM bootstrap_operations WHERE operation_id = $1",
            )
            .bind(operation_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_api_error)?
        {
            if stored_hash.as_slice() != supplied_request_hash.as_bytes() {
                return Err(ApiError::new(
                    ApiErrorCode::OperationConflict,
                    "operation_id was already used with different request content",
                ));
            }
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(CreateInstallationResponse {
                installation_id: installation_id.to_string(),
                author_principal_id: author_principal_id.to_string(),
            });
        }

        let installation_id = Uuid::now_v7();
        let author_principal_id = Uuid::now_v7();

        sqlx::query("INSERT INTO author_principals (id, kind) VALUES ($1, 'installation')")
            .bind(author_principal_id)
            .execute(&mut *tx)
            .await
            .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO installations (id, author_principal_id, credential_hash) \
             VALUES ($1, $2, $3)",
        )
        .bind(installation_id)
        .bind(author_principal_id)
        .bind(credential_hash.as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO bootstrap_operations \
             (operation_id, request_hash, installation_id, author_principal_id) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(operation_id.as_uuid())
        .bind(supplied_request_hash.as_bytes().as_slice())
        .bind(installation_id)
        .bind(author_principal_id)
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        tx.commit().await.map_err(internal_api_error)?;

        Ok(CreateInstallationResponse {
            installation_id: installation_id.to_string(),
            author_principal_id: author_principal_id.to_string(),
        })
    }
}

fn decode_hash(value: &str, field: &str) -> Result<[u8; 32], ApiError> {
    let bytes = hex::decode(value)
        .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, format!("{field} must be hex")))?;
    bytes.try_into().map_err(|_| {
        ApiError::new(
            ApiErrorCode::InvalidRequest,
            format!("{field} must encode 32 bytes"),
        )
    })
}

fn internal_api_error(error: sqlx::Error) -> ApiError {
    ApiError::new(
        ApiErrorCode::Internal,
        format!("registry database error: {error}"),
    )
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("PostgreSQL error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("object-store connectivity error: {0}")]
    ObjectStore(String),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("registry schema is not current (found {0:?}, expected 1); run denju-server migrate")]
    SchemaOutOfDate(Option<i64>),
}
