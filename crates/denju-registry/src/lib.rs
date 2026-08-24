//! Registry use cases plus PostgreSQL and S3-compatible persistence boundaries.

mod access;
mod admin;
mod discovery;
mod fork_sync;
mod history;
mod identity;
mod identity_auth;
mod identity_delete;
mod identity_support;
mod ingest;
mod ingest_storage;
mod lifecycle;
mod lifecycle_hash;
mod lifecycle_storage;
mod observability;
mod outbox;
mod pack_detail;
mod pack_drain;
mod pack_lifecycle;
mod pack_storage;
mod packs;
mod private_catalog;
mod proposal_metadata;
mod proposals;
mod public_registry;
mod public_seed;
mod realtime;
mod release;
mod release_validation;
mod rename_content;
mod revision_graph;
mod rls;
mod sharing;
mod social;
mod subscription_access;
mod subscriptions;
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
use sqlx::{
    Connection, PgPool,
    postgres::{PgConnection, PgPoolOptions},
};
use thiserror::Error;
use tokio::sync::broadcast;
use url::Url;
use uuid::Uuid;

pub use observability::{RegistryMetricsSnapshot, RegistryOperationalMetrics};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const EXPECTED_SCHEMA_VERSION: i64 = 17;
const SNAPSHOT_URL_TTL: Duration = Duration::from_secs(5 * 60);
const STAGING_URL_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
pub struct RegistrySettings {
    /// Transaction-pooled request SQL. This URL must authenticate directly as the restricted
    /// `denju_app` login role; connecting as an owner and switching roles is deliberately
    /// rejected because the session identity could regain bypass privileges.
    pub database_url: String,
    /// Background/recovery SQL. This URL must authenticate directly as the separate restricted
    /// `denju_worker` login role and must not be usable by the request pool.
    pub database_worker_url: String,
    /// Optional session-mode/direct PostgreSQL URL used only for LISTEN/NOTIFY. Never point
    /// this at a transaction-mode pooler such as Neon's pooled endpoint. It must authenticate
    /// directly as `denju_app`, like the request pool.
    pub database_listen_url: Option<String>,
    pub public_origin: Url,
    /// S3 endpoint used by the registry process for SDK reads/writes.
    pub object_store_endpoint: Url,
    /// S3 endpoint embedded in product presigned URLs returned to clients. This may differ
    /// from the registry-internal endpoint when the object store lives on a private network.
    pub object_store_presign_endpoint: Url,
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
    worker_pool: PgPool,
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
        let pool = connect_role_pool(&settings.database_url, 10, DatabaseRole::App).await?;
        let worker_pool =
            connect_role_pool(&settings.database_worker_url, 4, DatabaseRole::Worker).await?;
        validate_database_role(&pool, DatabaseRole::App).await?;
        validate_database_role(&worker_pool, DatabaseRole::Worker).await?;
        if let Some(database_listen_url) = settings.database_listen_url.as_deref() {
            validate_direct_database_role(database_listen_url, DatabaseRole::App).await?;
        }
        let objects = ObjectStore::new(&settings);
        let (wake_tx, _) = broadcast::channel(256);
        Ok(Self {
            pool,
            worker_pool,
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
        validate_database_role(&self.pool, DatabaseRole::App).await?;
        validate_database_role(&self.worker_pool, DatabaseRole::Worker).await?;
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

        // The provider probe executes from the registry process itself, so use the internal
        // endpoint here. Product presigned URLs may intentionally point at a client-facing
        // hostname that is not routable from the server container (for example the bundled
        // loopback-only Garage publication in the reference self-host stack).
        let upload_url = self
            .objects
            .presign_put_internal(&staging_key, payload.len() as u64)
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
        let download_url = self.objects.presign_get_internal(&canonical_key).await?;
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
        sqlx::query_scalar::<_, Uuid>("SELECT denju_authenticate_installation($1)")
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DatabaseRole {
    App,
    Worker,
}

impl DatabaseRole {
    const fn name(self) -> &'static str {
        match self {
            Self::App => "denju_app",
            Self::Worker => "denju_worker",
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::App => Self::Worker,
            Self::Worker => Self::App,
        }
    }
}

async fn connect_role_pool(
    database_url: &str,
    max_connections: u32,
    _role: DatabaseRole,
) -> Result<PgPool, RegistryError> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database_url)
        .await
        .map_err(RegistryError::Database)
}

async fn validate_database_role(
    pool: &PgPool,
    expected: DatabaseRole,
) -> Result<(), RegistryError> {
    let row = sqlx::query_as::<_, (String, String, bool, bool, bool, bool, bool)>(
        "SELECT current_user,session_user,role.rolsuper,role.rolbypassrls,role.rolcanlogin, \
                pg_has_role(session_user,$1,'SET'), \
                EXISTS(SELECT 1 FROM pg_roles target WHERE target.rolname<>session_user \
                  AND (target.rolsuper OR target.rolbypassrls) \
                  AND pg_has_role(session_user,target.oid,'SET')) \
         FROM pg_roles role WHERE role.rolname=current_user",
    )
    .bind(expected.other().name())
    .fetch_one(pool)
    .await?;
    if row.0 != expected.name()
        || row.1 != expected.name()
        || row.2
        || row.3
        || !row.4
        || row.5
        || row.6
    {
        return Err(RegistryError::SecurityBoundary(format!(
            "database pool must authenticate directly as isolated non-bypass role {} (current_user={}, session_user={}, superuser={}, bypassrls={}, login={}, can_set_other_role={}, can_set_bypass_role={})",
            expected.name(),
            row.0,
            row.1,
            row.2,
            row.3,
            row.4,
            row.5,
            row.6
        )));
    }
    Ok(())
}

async fn validate_direct_database_role(
    database_url: &str,
    expected: DatabaseRole,
) -> Result<(), RegistryError> {
    let mut connection = PgConnection::connect(database_url).await?;
    let row = sqlx::query_as::<_, (String, String, bool, bool, bool, bool, bool)>(
        "SELECT current_user,session_user,role.rolsuper,role.rolbypassrls,role.rolcanlogin, \
                pg_has_role(session_user,$1,'SET'), \
                EXISTS(SELECT 1 FROM pg_roles target WHERE target.rolname<>session_user \
                  AND (target.rolsuper OR target.rolbypassrls) \
                  AND pg_has_role(session_user,target.oid,'SET')) \
         FROM pg_roles role WHERE role.rolname=current_user",
    )
    .bind(expected.other().name())
    .fetch_one(&mut connection)
    .await?;
    connection.close().await?;
    if row.0 != expected.name()
        || row.1 != expected.name()
        || row.2
        || row.3
        || !row.4
        || row.5
        || row.6
    {
        return Err(RegistryError::SecurityBoundary(format!(
            "direct database connection must authenticate as isolated non-bypass role {}",
            expected.name()
        )));
    }
    Ok(())
}

#[derive(Clone)]
struct ObjectStore {
    client: S3Client,
    presign_client: S3Client,
    bucket: String,
}

impl ObjectStore {
    fn new(settings: &RegistrySettings) -> Self {
        let client = object_store_client(settings, &settings.object_store_endpoint);
        let presign_client = object_store_client(settings, &settings.object_store_presign_endpoint);
        Self {
            client,
            presign_client,
            bucket: settings.object_store_bucket.clone(),
        }
    }

    async fn head_bucket(&self) -> Result<(), RegistryError> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .map_err(object_store_error)?;
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
            .map_err(object_store_error)?;
        observability::record_object_store_write_bytes(bytes.len());
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
            .map_err(object_store_error)?;
        let bytes = output
            .body
            .collect()
            .await
            .map(|bytes| bytes.into_bytes().to_vec())
            .map_err(object_store_error)?;
        observability::record_object_store_read_bytes(bytes.len());
        Ok(bytes)
    }

    async fn delete(&self, key: &str) -> Result<(), RegistryError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(object_store_error)?;
        Ok(())
    }

    async fn presign_put(&self, key: &str, size_bytes: u64) -> Result<String, RegistryError> {
        self.presign_put_with(&self.presign_client, key, size_bytes)
            .await
    }

    async fn presign_put_internal(
        &self,
        key: &str,
        size_bytes: u64,
    ) -> Result<String, RegistryError> {
        self.presign_put_with(&self.client, key, size_bytes).await
    }

    async fn presign_put_with(
        &self,
        client: &S3Client,
        key: &str,
        size_bytes: u64,
    ) -> Result<String, RegistryError> {
        let config = PresigningConfig::expires_in(STAGING_URL_TTL)
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        let size = i64::try_from(size_bytes)
            .map_err(|_| RegistryError::ObjectStore("object is too large to upload".to_owned()))?;
        let request = client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_length(size)
            .presigned(config)
            .await
            .map_err(object_store_error)?;
        Ok(request.uri().to_string())
    }

    async fn presign_get(&self, key: &str) -> Result<String, RegistryError> {
        self.presign_get_with(&self.presign_client, key).await
    }

    async fn presign_get_internal(&self, key: &str) -> Result<String, RegistryError> {
        self.presign_get_with(&self.client, key).await
    }

    async fn presign_get_with(
        &self,
        client: &S3Client,
        key: &str,
    ) -> Result<String, RegistryError> {
        let config = PresigningConfig::expires_in(SNAPSHOT_URL_TTL)
            .map_err(|error| RegistryError::ObjectStore(error.to_string()))?;
        let request = client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(config)
            .await
            .map_err(object_store_error)?;
        Ok(request.uri().to_string())
    }
}

fn object_store_client(settings: &RegistrySettings, endpoint: &Url) -> S3Client {
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
        .endpoint_url(endpoint.to_string())
        .force_path_style(settings.object_store_force_path_style)
        .build();
    S3Client::from_conf(config)
}

#[cfg(test)]
mod object_store_tests {
    use super::*;

    fn settings() -> RegistrySettings {
        RegistrySettings {
            database_url: "postgresql://app@localhost/denju".to_owned(),
            database_worker_url: "postgresql://worker@localhost/denju".to_owned(),
            database_listen_url: None,
            public_origin: Url::parse("http://127.0.0.1:7788").unwrap(),
            object_store_endpoint: Url::parse("http://garage:3900").unwrap(),
            object_store_presign_endpoint: Url::parse("http://127.0.0.1:53900").unwrap(),
            object_store_bucket: "denju".to_owned(),
            object_store_region: "garage".to_owned(),
            object_store_access_key_id: "GK1234567890ABCDEFGH".to_owned(),
            object_store_secret_access_key:
                "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
            object_store_force_path_style: true,
            limits: RegistryLimits {
                max_object_bytes: 16 * 1024 * 1024,
                max_release_bytes: 10 * 1024 * 1024,
                namespace_storage_bytes: 512 * 1024 * 1024,
                max_transfer_bytes: 16 * 1024 * 1024,
            },
            gc_grace: Duration::from_secs(86_400),
        }
    }

    #[tokio::test]
    async fn product_presigns_use_the_client_facing_endpoint() {
        let store = ObjectStore::new(&settings());
        for uri in [
            store.presign_put("staging/blob", 3).await.unwrap(),
            store.presign_get("canonical/snapshot").await.unwrap(),
        ] {
            let url = Url::parse(&uri).unwrap();
            assert_eq!(url.host_str(), Some("127.0.0.1"));
            assert_eq!(url.port(), Some(53_900));
        }

        let internal = Url::parse(&store.presign_get_internal("probe").await.unwrap()).unwrap();
        assert_eq!(internal.host_str(), Some("garage"));
        assert_eq!(internal.port(), Some(3900));
    }
}

fn object_store_error(error: impl std::fmt::Display) -> RegistryError {
    observability::record_object_store_error();
    RegistryError::ObjectStore(error.to_string())
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
    observability::record_database_error();
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
    #[error("security boundary error: {0}")]
    SecurityBoundary(String),
    #[error(
        "registry schema is not current (found {0:?}, expected {EXPECTED_SCHEMA_VERSION}); run denju-server migrate"
    )]
    SchemaOutOfDate(Option<i64>),
    #[error("public seed error: {0}")]
    Seed(String),
    #[error("seed manifest serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
