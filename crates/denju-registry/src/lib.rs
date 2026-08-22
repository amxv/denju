//! Registry use cases plus PostgreSQL and S3-compatible persistence boundaries.

mod access;
mod fork_sync;
mod history;
mod identity;
mod identity_auth;
mod identity_support;
mod ingest;
mod ingest_storage;
mod lifecycle;
mod lifecycle_hash;
mod lifecycle_storage;
mod outbox;
mod pack_detail;
mod pack_drain;
mod pack_lifecycle;
mod pack_storage;
mod packs;
mod private_catalog;
mod proposals;
mod public_registry;
mod public_seed;
mod realtime;
mod release;
mod release_validation;
mod rename_content;
mod revision_graph;
mod sharing;
mod subscription_access;
mod team_access;
mod team_policy;
mod team_rename;
mod teams;
mod transfer;
mod workspace;
mod workspace_conflict;
mod workspace_storage;

use std::{
    str::FromStr,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client as S3Client, config::Region, presigning::PresigningConfig, primitives::ByteStream,
};
use denju_core::{BlobId, OperationId};
use denju_wire::{
    ApiError, ApiErrorCode, CreateInstallationRequest, CreateInstallationResponse,
    RegistryCapabilities, RegistryLimits, RequestHash, create_installation_request_hash,
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use thiserror::Error;
use tokio::sync::broadcast;
use url::Url;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const EXPECTED_SCHEMA_VERSION: i64 = 13;
const SNAPSHOT_URL_TTL: Duration = Duration::from_secs(5 * 60);
const STAGING_URL_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
pub struct RegistrySettings {
    pub database_url: String,
    /// Optional session-mode/direct PostgreSQL URL used only for LISTEN/NOTIFY. Never point
    /// this at a transaction-mode pooler such as Neon's pooled endpoint.
    pub database_listen_url: Option<String>,
    pub public_origin: Url,
    pub object_store_endpoint: Url,
    pub object_store_bucket: String,
    pub object_store_region: String,
    pub object_store_access_key_id: String,
    pub object_store_secret_access_key: String,
    pub object_store_force_path_style: bool,
    pub limits: RegistryLimits,
    pub gc_grace: Duration,
}

#[derive(Clone)]
pub struct Registry {
    pool: PgPool,
    objects: ObjectStore,
    public_origin: Url,
    limits: RegistryLimits,
    gc_grace: Duration,
    wake_tx: broadcast::Sender<RegistryWake>,
    database_listen_url: Option<String>,
    wake_listener_started: Arc<AtomicBool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryWake {
    Resource { resource_id: Uuid, generation: u64 },
    ResyncAll,
}

impl Registry {
    pub async fn connect(settings: RegistrySettings) -> Result<Self, RegistryError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(&settings.database_url)
            .await?;
        let objects = ObjectStore::new(&settings);
        let (wake_tx, _) = broadcast::channel(256);
        Ok(Self {
            pool,
            objects,
            public_origin: settings.public_origin,
            limits: settings.limits,
            gc_grace: settings.gc_grace,
            wake_tx,
            database_listen_url: settings.database_listen_url,
            wake_listener_started: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn subscribe_wakes(&self) -> broadcast::Receiver<RegistryWake> {
        self.wake_tx.subscribe()
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
        self.objects.head_bucket().await?;
        Ok(())
    }

    /// Exercise the generic S3-compatible provider boundary used by Denju. This is a
    /// deployment/development conformance probe, not a product-data path: it verifies
    /// presigned staging upload, SDK reads/writes, presigned reads, promotion-style copy
    /// semantics, and idempotent deletion against the configured provider.
    pub async fn verify_object_store_provider(&self) -> Result<(), RegistryError> {
        let payload = b"denju-s3-provider-conformance-v1\n";
        let payload_hash = BlobId::hash(payload);
        let run = Uuid::now_v7();
        let staging_key = format!("conformance/{run}/staging/{payload_hash}");
        let canonical_key = format!("conformance/{run}/canonical/{payload_hash}");

        let upload_url = self
            .objects
            .presign_put(&staging_key, payload.len() as u64)
            .await?;
        let upload = reqwest::Client::new()
            .put(upload_url)
            .body(payload.to_vec())
            .send()
            .await
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        if !upload.status().is_success() {
            return Err(RegistryError::ObjectStore(format!(
                "presigned provider upload returned {}",
                upload.status()
            )));
        }
        let staged = self.objects.get(&staging_key).await?;
        if staged != payload {
            return Err(RegistryError::ObjectStore(
                "provider returned different staged bytes".to_owned(),
            ));
        }

        self.objects.put(&canonical_key, &staged).await?;
        // A retry of the canonical write must remain safe for immutable content.
        self.objects.put(&canonical_key, &staged).await?;
        let download_url = self.objects.presign_get(&canonical_key).await?;
        let downloaded = reqwest::get(download_url)
            .await
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        if !downloaded.status().is_success() {
            return Err(RegistryError::ObjectStore(format!(
                "presigned provider download returned {}",
                downloaded.status()
            )));
        }
        let downloaded = downloaded
            .bytes()
            .await
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        if downloaded.as_ref() != payload {
            return Err(RegistryError::ObjectStore(
                "provider returned different canonical bytes".to_owned(),
            ));
        }

        self.objects.delete(&staging_key).await?;
        self.objects.delete(&canonical_key).await?;
        // S3 DELETE is intentionally idempotent and Denju relies on safe retries.
        self.objects.delete(&canonical_key).await?;
        Ok(())
    }

    pub async fn validate_schema(&self) -> Result<(), RegistryError> {
        let version = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM _sqlx_migrations WHERE success = true ORDER BY version DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        if version != Some(EXPECTED_SCHEMA_VERSION) {
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
            "INSERT INTO installations (id, author_principal_id, credential_hash) VALUES ($1, $2, $3)",
        )
        .bind(installation_id)
        .bind(author_principal_id)
        .bind(credential_hash.as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal_api_error)?;
        sqlx::query(
            "INSERT INTO bootstrap_operations (operation_id, request_hash, installation_id, author_principal_id) \
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

    pub(crate) async fn authenticate_installation(&self, bearer: &str) -> Result<Uuid, ApiError> {
        let raw = hex::decode(bearer)
            .ok()
            .filter(|bytes| bytes.len() == 32)
            .ok_or_else(|| {
                ApiError::new(
                    ApiErrorCode::Unauthorized,
                    "invalid installation credential",
                )
            })?;
        let credential_hash: [u8; 32] = Sha256::digest(&raw).into();
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM installations WHERE credential_hash = $1 AND revoked_at IS NULL",
        )
        .bind(credential_hash.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| {
            ApiError::new(
                ApiErrorCode::Unauthorized,
                "invalid installation credential",
            )
        })
    }
}

#[derive(Clone)]
struct ObjectStore {
    client: S3Client,
    bucket: String,
}

impl ObjectStore {
    fn new(settings: &RegistrySettings) -> Self {
        let credentials = Credentials::new(
            settings.object_store_access_key_id.clone(),
            settings.object_store_secret_access_key.clone(),
            None,
            None,
            "denju-static",
        );
        let config = aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .region(Region::new(settings.object_store_region.clone()))
            .credentials_provider(credentials)
            .endpoint_url(settings.object_store_endpoint.to_string())
            .force_path_style(settings.object_store_force_path_style)
            .build();
        Self {
            client: S3Client::from_conf(config),
            bucket: settings.object_store_bucket.clone(),
        }
    }

    async fn head_bucket(&self) -> Result<(), RegistryError> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        Ok(())
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), RegistryError> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(bytes.to_vec()))
            .send()
            .await
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, RegistryError> {
        let output = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        output
            .body
            .collect()
            .await
            .map(|bytes| bytes.into_bytes().to_vec())
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))
    }

    async fn delete(&self, key: &str) -> Result<(), RegistryError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        Ok(())
    }

    async fn presign_put(&self, key: &str, size_bytes: u64) -> Result<String, RegistryError> {
        let config = PresigningConfig::expires_in(STAGING_URL_TTL)
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        let size = i64::try_from(size_bytes)
            .map_err(|_| RegistryError::ObjectStore("object is too large to upload".to_owned()))?;
        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_length(size)
            .presigned(config)
            .await
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        Ok(request.uri().to_string())
    }

    async fn presign_get(&self, key: &str) -> Result<String, RegistryError> {
        let config = PresigningConfig::expires_in(SNAPSHOT_URL_TTL)
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        let request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(config)
            .await
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        Ok(request.uri().to_string())
    }
}

// Use tuples for the two owner/name columns to keep SQL identifiers simple and avoid
// exposing SQL storage naming through public structs.
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
    #[error("object-store error: {0}")]
    ObjectStore(String),
    #[error(
        "registry schema is not current (found {0:?}, expected {EXPECTED_SCHEMA_VERSION}); run denju-server migrate"
    )]
    SchemaOutOfDate(Option<i64>),
    #[error("public seed error: {0}")]
    Seed(String),
    #[error("seed manifest serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
