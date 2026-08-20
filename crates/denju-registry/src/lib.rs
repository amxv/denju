//! Registry use cases plus PostgreSQL and S3-compatible persistence boundaries.

mod identity;

use std::{str::FromStr, time::Duration};

use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client as S3Client, config::Region, presigning::PresigningConfig, primitives::ByteStream,
};
use denju_core::{
    AuthorPrincipalId, BlobId, DeterministicSkillSnapshot, OperationId, OwnedSkillEntry,
    ResourceId, ResourceKind, ResourceLocator, Revision, parse_skill_document,
};
use denju_wire::{
    ApiError, ApiErrorCode, CreateInstallationRequest, CreateInstallationResponse, PublicSkill,
    PublicSkillDetail, PublicSkillManifest, PublicSkillSearchResponse, RegistryCapabilities,
    RegistryLimits, RequestHash, SnapshotDownload, SubscribedSkill, SubscriptionCatalog,
    SubscriptionMutationKind, SubscriptionMutationRequest, SubscriptionMutationResponse,
    create_installation_request_hash, subscription_request_hash,
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const EXPECTED_SCHEMA_VERSION: i64 = 3;
const SNAPSHOT_URL_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone)]
pub struct RegistrySettings {
    pub database_url: String,
    pub public_origin: Url,
    pub object_store_endpoint: Url,
    pub object_store_bucket: String,
    pub object_store_region: String,
    pub object_store_access_key_id: String,
    pub object_store_secret_access_key: String,
    pub object_store_force_path_style: bool,
    pub limits: RegistryLimits,
}

#[derive(Clone)]
pub struct Registry {
    pool: PgPool,
    objects: ObjectStore,
    public_origin: Url,
    limits: RegistryLimits,
}

impl Registry {
    pub async fn connect(settings: RegistrySettings) -> Result<Self, RegistryError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(&settings.database_url)
            .await?;
        let objects = ObjectStore::new(&settings);
        Ok(Self {
            pool,
            objects,
            public_origin: settings.public_origin,
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
        self.objects.head_bucket().await?;
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

    pub async fn search_public_skills(
        &self,
        query: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<PublicSkillSearchResponse, ApiError> {
        let limit = limit.clamp(1, 50);
        let pattern = format!("%{}%", query.trim());
        let rows = if let Some(cursor) = cursor {
            let cursor = SearchCursor::decode(cursor)?;
            sqlx::query_as::<_, PublicSkillDbTuple>(
                "SELECT r.id, n.slug, r.slug, r.description, r.generation, sr.version, sr.revision_id \
                 FROM resources r \
                 JOIN namespaces n ON n.id = r.owner_namespace_id \
                 JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = r.latest_release_version \
                 WHERE r.visibility = 'public' AND r.kind = 'skill' \
                   AND (r.slug ILIKE $1 OR n.slug ILIKE $1 OR r.description ILIKE $1) \
                   AND (n.slug > $2 OR (n.slug = $2 AND r.slug > $3) \
                        OR (n.slug = $2 AND r.slug = $3 AND r.id > $4)) \
                 ORDER BY n.slug, r.slug, r.id LIMIT $5",
            )
            .bind(&pattern)
            .bind(cursor.owner)
            .bind(cursor.name)
            .bind(cursor.resource_id)
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await
            .map_err(internal_api_error)?
        } else {
            sqlx::query_as::<_, PublicSkillDbTuple>(
                "SELECT r.id, n.slug, r.slug, r.description, r.generation, sr.version, sr.revision_id \
                 FROM resources r \
                 JOIN namespaces n ON n.id = r.owner_namespace_id \
                 JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = r.latest_release_version \
                 WHERE r.visibility = 'public' AND r.kind = 'skill' \
                   AND (r.slug ILIKE $1 OR n.slug ILIKE $1 OR r.description ILIKE $1) \
                 ORDER BY n.slug, r.slug, r.id LIMIT $2",
            )
            .bind(&pattern)
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await
            .map_err(internal_api_error)?
        };

        let has_more = rows.len() > limit as usize;
        let visible = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
        let next_cursor = if has_more {
            visible
                .last()
                .map(SearchCursor::from_row)
                .map(|cursor| cursor.encode())
        } else {
            None
        };
        let items = visible
            .into_iter()
            .map(PublicSkillTupleExt::into_wire)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PublicSkillSearchResponse { items, next_cursor })
    }

    pub async fn show_public_skill(&self, locator: &str) -> Result<PublicSkillDetail, ApiError> {
        let locator = ResourceLocator::from_str(locator)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        if locator.kind() != ResourceKind::Skill {
            return Err(ApiError::new(
                ApiErrorCode::NotFound,
                "public skill not found",
            ));
        }
        self.public_skill_detail(locator.owner(), locator.name())
            .await
    }

    pub async fn mutate_subscription(
        &self,
        bearer: &str,
        kind: SubscriptionMutationKind,
        request: &SubscriptionMutationRequest,
    ) -> Result<SubscriptionMutationResponse, ApiError> {
        let subject = self.subscription_subject(bearer).await?;
        let operation_id = OperationId::from_str(&request.operation_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let resource_id = ResourceId::from_str(&request.resource_id)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequest, error.to_string()))?;
        let supplied_hash = RequestHash::from_str(&request.request_hash)
            .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        let expected_hash = subscription_request_hash(
            kind,
            &request.operation_id,
            &request.resource_id,
            request.expected_generation,
        )
        .map_err(|error| ApiError::new(ApiErrorCode::InvalidRequestHash, error.to_string()))?;
        if supplied_hash != expected_hash {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequestHash,
                "request_hash does not match the canonical request payload",
            ));
        }

        let mut tx = self.pool.begin().await.map_err(internal_api_error)?;
        let replay = match subject {
            identity::SubscriptionSubject::Installation(installation_id) => {
                sqlx::query_as::<_, (Vec<u8>, Uuid, bool)>(
                    "SELECT request_hash, resource_id, subscribed FROM subscription_operations \
                     WHERE installation_id = $1 AND operation_id = $2",
                )
                .bind(installation_id)
                .bind(operation_id.as_uuid())
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal_api_error)?
            }
            identity::SubscriptionSubject::User(user_id) => sqlx::query_as::<
                _,
                (Vec<u8>, Uuid, bool),
            >(
                "SELECT request_hash, resource_id, subscribed FROM account_subscription_operations \
                     WHERE user_id = $1 AND operation_id = $2",
            )
            .bind(user_id)
            .bind(operation_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_api_error)?,
        };
        if let Some((stored_hash, stored_resource, subscribed)) = replay {
            if stored_hash.as_slice() != supplied_hash.as_bytes()
                || stored_resource != resource_id.as_uuid()
            {
                return Err(ApiError::new(
                    ApiErrorCode::OperationConflict,
                    "operation_id was already used with different request content",
                ));
            }
            tx.commit().await.map_err(internal_api_error)?;
            return Ok(SubscriptionMutationResponse {
                resource_id: stored_resource.to_string(),
                subscribed,
            });
        }

        let generation = sqlx::query_scalar::<_, i64>(
            "SELECT generation FROM resources WHERE id = $1 AND kind = 'skill' AND visibility = 'public'",
        )
        .bind(resource_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "public skill not found"))?;
        let generation = u64::try_from(generation)
            .map_err(|_| ApiError::new(ApiErrorCode::Internal, "resource generation is invalid"))?;
        if generation != request.expected_generation {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("resource generation changed to {generation}"),
            ));
        }

        let subscribed = kind == SubscriptionMutationKind::Subscribe;
        match (subject, kind) {
            (
                identity::SubscriptionSubject::Installation(installation_id),
                SubscriptionMutationKind::Subscribe,
            ) => {
                sqlx::query(
                    "INSERT INTO installation_subscriptions (installation_id, resource_id) \
                     VALUES ($1, $2) ON CONFLICT DO NOTHING",
                )
                .bind(installation_id)
                .bind(resource_id.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            (
                identity::SubscriptionSubject::Installation(installation_id),
                SubscriptionMutationKind::Unsubscribe,
            ) => {
                sqlx::query(
                    "DELETE FROM installation_subscriptions WHERE installation_id = $1 AND resource_id = $2",
                )
                .bind(installation_id)
                .bind(resource_id.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            (identity::SubscriptionSubject::User(user_id), SubscriptionMutationKind::Subscribe) => {
                sqlx::query(
                    "INSERT INTO account_subscriptions (user_id, resource_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                )
                .bind(user_id)
                .bind(resource_id.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            (
                identity::SubscriptionSubject::User(user_id),
                SubscriptionMutationKind::Unsubscribe,
            ) => {
                sqlx::query(
                    "DELETE FROM account_subscriptions WHERE user_id = $1 AND resource_id = $2",
                )
                .bind(user_id)
                .bind(resource_id.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
        }
        let action = match kind {
            SubscriptionMutationKind::Subscribe => "subscribe",
            SubscriptionMutationKind::Unsubscribe => "unsubscribe",
        };
        match subject {
            identity::SubscriptionSubject::Installation(installation_id) => {
                sqlx::query(
                    "INSERT INTO subscription_operations \
                     (installation_id, operation_id, request_hash, action, resource_id, subscribed) \
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(installation_id)
                .bind(operation_id.as_uuid())
                .bind(supplied_hash.as_bytes().as_slice())
                .bind(action)
                .bind(resource_id.as_uuid())
                .bind(subscribed)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            identity::SubscriptionSubject::User(user_id) => {
                sqlx::query(
                    "INSERT INTO account_subscription_operations \
                     (user_id, operation_id, request_hash, action, resource_id, subscribed) \
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(user_id)
                .bind(operation_id.as_uuid())
                .bind(supplied_hash.as_bytes().as_slice())
                .bind(action)
                .bind(resource_id.as_uuid())
                .bind(subscribed)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
        }
        tx.commit().await.map_err(internal_api_error)?;

        Ok(SubscriptionMutationResponse {
            resource_id: resource_id.to_string(),
            subscribed,
        })
    }

    pub async fn subscription_catalog(
        &self,
        bearer: &str,
    ) -> Result<SubscriptionCatalog, ApiError> {
        let subject = self.subscription_subject(bearer).await?;
        let rows = match subject {
            identity::SubscriptionSubject::Installation(installation_id) => {
                sqlx::query_as::<_, SubscriptionRow>(
                    "SELECT r.id, n.slug AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, \
                            sr.manifest_json, sr.snapshot_key, sr.snapshot_sha256, sr.snapshot_size \
                     FROM installation_subscriptions s \
                     JOIN resources r ON r.id = s.resource_id \
                     JOIN namespaces n ON n.id = r.owner_namespace_id \
                     JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = r.latest_release_version \
                     WHERE s.installation_id = $1 AND r.visibility = 'public' AND r.kind = 'skill' \
                     ORDER BY n.slug, r.slug, r.id",
                )
                .bind(installation_id)
                .fetch_all(&self.pool)
                .await
                .map_err(internal_api_error)?
            }
            identity::SubscriptionSubject::User(user_id) => {
                sqlx::query_as::<_, SubscriptionRow>(
                    "SELECT r.id, n.slug AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, \
                            sr.manifest_json, sr.snapshot_key, sr.snapshot_sha256, sr.snapshot_size \
                     FROM account_subscriptions s \
                     JOIN resources r ON r.id = s.resource_id \
                     JOIN namespaces n ON n.id = r.owner_namespace_id \
                     JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = r.latest_release_version \
                     WHERE s.user_id = $1 AND r.visibility = 'public' AND r.kind = 'skill' \
                     ORDER BY n.slug, r.slug, r.id",
                )
                .bind(user_id)
                .fetch_all(&self.pool)
                .await
                .map_err(internal_api_error)?
            }
        };

        let mut skills = Vec::with_capacity(rows.len());
        for row in rows {
            let snapshot_url = self
                .objects
                .presign_get(&row.snapshot_key)
                .await
                .map_err(|error| ApiError::new(ApiErrorCode::Unavailable, error.to_string()))?;
            skills.push(row.into_wire(snapshot_url)?);
        }
        Ok(SubscriptionCatalog { skills })
    }

    /// Trusted development/test harness seam used to seed public immutable releases before
    /// authenticated import/publish exist. It persists through the same PostgreSQL/S3 read
    /// model consumed by every public HTTP request; there is no in-memory seed catalog.
    pub async fn seed_public_skill(
        &self,
        owner: &str,
        snapshot: &DeterministicSkillSnapshot,
        entries: &[OwnedSkillEntry],
    ) -> Result<PublicSkillDetail, RegistryError> {
        let skill_md = entries
            .iter()
            .find_map(|entry| match entry {
                OwnedSkillEntry::File { path, bytes, .. } if path == "SKILL.md" => {
                    Some(bytes.as_slice())
                }
                _ => None,
            })
            .ok_or_else(|| RegistryError::Seed("seed skill is missing SKILL.md".to_owned()))?;
        // The directory name is not authority at this trusted seed edge, so discover the
        // declared name first and then run the canonical denju-core parser against it.
        let yaml_name = skill_frontmatter_name(skill_md)?;
        let document = parse_skill_document(&yaml_name, skill_md)
            .map_err(|error| RegistryError::Seed(error.to_string()))?;
        let description = document.frontmatter().description().to_owned();
        ResourceLocator::from_str(&format!("@{owner}/{yaml_name}"))
            .map_err(|error| RegistryError::Seed(error.to_string()))?;

        let resource_id = ResourceId::from_uuid(Uuid::now_v7())
            .map_err(|error| RegistryError::Seed(error.to_string()))?;
        let author_id = AuthorPrincipalId::from_uuid(Uuid::now_v7())
            .map_err(|error| RegistryError::Seed(error.to_string()))?;
        let operation_id = OperationId::from_uuid(Uuid::now_v7())
            .map_err(|error| RegistryError::Seed(error.to_string()))?;
        let revision = Revision::new(
            snapshot.manifest().root_tree(),
            Vec::new(),
            author_id,
            operation_id,
        )
        .map_err(|error| RegistryError::Seed(error.to_string()))?;
        let revision_id = revision.id();
        let snapshot_sha256: [u8; 32] = Sha256::digest(snapshot.bytes()).into();
        let snapshot_key = format!("snapshots/sha256/{}.tar.zst", hex::encode(snapshot_sha256));

        for entry in entries {
            if let OwnedSkillEntry::File { bytes, .. } = entry {
                let blob = BlobId::hash(bytes);
                self.objects
                    .put(
                        &format!("blobs/sha256/{}/{blob}", &blob.to_string()[..2]),
                        bytes,
                    )
                    .await?;
            }
        }
        self.objects.put(&snapshot_key, snapshot.bytes()).await?;

        let manifest = PublicSkillManifest::from_core(snapshot.manifest());
        let manifest_json = serde_json::to_value(&manifest)?;
        let snapshot_size = i64::try_from(snapshot.bytes().len())
            .map_err(|_| RegistryError::Seed("snapshot is too large".to_owned()))?;
        let namespace_id = Uuid::now_v7();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO namespaces (id, slug, kind) VALUES ($1, $2, 'user') ON CONFLICT (slug) DO NOTHING",
        )
        .bind(namespace_id)
        .bind(owner)
        .execute(&mut *tx)
        .await?;
        let namespace_id =
            sqlx::query_scalar::<_, Uuid>("SELECT id FROM namespaces WHERE slug = $1")
                .bind(owner)
                .fetch_one(&mut *tx)
                .await?;
        if sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM resources WHERE owner_namespace_id = $1 AND kind = 'skill' AND slug = $2",
        )
        .bind(namespace_id)
        .bind(&yaml_name)
        .fetch_optional(&mut *tx)
        .await?
        .is_some()
        {
            return Err(RegistryError::Seed(format!(
                "public skill @{owner}/{yaml_name} is already seeded"
            )));
        }
        sqlx::query("INSERT INTO author_principals (id, kind) VALUES ($1, 'user')")
            .bind(author_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO resources \
             (id, owner_namespace_id, slug, kind, visibility, description, generation, latest_release_version) \
             VALUES ($1, $2, $3, 'skill', 'public', $4, 1, 1)",
        )
        .bind(resource_id.as_uuid())
        .bind(namespace_id)
        .bind(&yaml_name)
        .bind(&description)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO skill_releases \
             (resource_id, version, revision_id, root_tree_id, manifest_json, snapshot_key, snapshot_sha256, snapshot_size, author_principal_id) \
             VALUES ($1, 1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(resource_id.as_uuid())
        .bind(revision_id.as_bytes().as_slice())
        .bind(snapshot.manifest().root_tree().as_bytes().as_slice())
        .bind(manifest_json)
        .bind(&snapshot_key)
        .bind(snapshot_sha256.as_slice())
        .bind(snapshot_size)
        .bind(author_id.as_uuid())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(PublicSkillDetail {
            skill: PublicSkill {
                resource_id: resource_id.to_string(),
                locator: format!("@{owner}/{yaml_name}"),
                owner: owner.to_owned(),
                name: yaml_name,
                description,
                generation: 1,
                version: 1,
                revision_id: revision_id.to_string(),
            },
            manifest,
        })
    }

    async fn public_skill_detail(
        &self,
        owner: &str,
        name: &str,
    ) -> Result<PublicSkillDetail, ApiError> {
        let row = sqlx::query_as::<_, PublicSkillDetailRow>(
            "SELECT r.id, n.slug AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, sr.manifest_json \
             FROM resources r \
             JOIN namespaces n ON n.id = r.owner_namespace_id \
             JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = r.latest_release_version \
             WHERE r.visibility = 'public' AND r.kind = 'skill' AND n.slug = $1 AND r.slug = $2",
        )
        .bind(owner)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "public skill not found"))?;
        row.into_wire()
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
type PublicSkillDbTuple = (Uuid, String, String, String, i64, i64, Vec<u8>);

trait PublicSkillTupleExt {
    fn into_wire(self) -> Result<PublicSkill, ApiError>;
}

impl PublicSkillTupleExt for PublicSkillDbTuple {
    fn into_wire(self) -> Result<PublicSkill, ApiError> {
        public_skill_from_parts(self.0, self.1, self.2, self.3, self.4, self.5, self.6)
    }
}

// Keep the actual query row alias as the tuple; this type alias is referenced below by
// helper functions but never serialized.
#[derive(sqlx::FromRow)]
struct PublicSkillDetailRow {
    id: Uuid,
    owner: String,
    name: String,
    description: String,
    generation: i64,
    version: i64,
    revision_id: Vec<u8>,
    manifest_json: serde_json::Value,
}

impl PublicSkillDetailRow {
    fn into_wire(self) -> Result<PublicSkillDetail, ApiError> {
        let skill = public_skill_from_parts(
            self.id,
            self.owner,
            self.name,
            self.description,
            self.generation,
            self.version,
            self.revision_id,
        )?;
        let manifest = serde_json::from_value(self.manifest_json)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        Ok(PublicSkillDetail { skill, manifest })
    }
}

#[derive(sqlx::FromRow)]
struct SubscriptionRow {
    id: Uuid,
    owner: String,
    name: String,
    description: String,
    generation: i64,
    version: i64,
    revision_id: Vec<u8>,
    manifest_json: serde_json::Value,
    snapshot_key: String,
    snapshot_sha256: Vec<u8>,
    snapshot_size: i64,
}

impl SubscriptionRow {
    fn into_wire(self, snapshot_url: String) -> Result<SubscribedSkill, ApiError> {
        let skill = public_skill_from_parts(
            self.id,
            self.owner,
            self.name,
            self.description,
            self.generation,
            self.version,
            self.revision_id,
        )?;
        let manifest = serde_json::from_value(self.manifest_json)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        let sha: [u8; 32] = self.snapshot_sha256.try_into().map_err(|_| {
            ApiError::new(
                ApiErrorCode::Internal,
                "stored snapshot checksum is invalid",
            )
        })?;
        let size_bytes = u64::try_from(self.snapshot_size).map_err(|_| {
            ApiError::new(ApiErrorCode::Internal, "stored snapshot size is invalid")
        })?;
        Ok(SubscribedSkill {
            skill,
            manifest,
            snapshot: SnapshotDownload {
                sha256: hex::encode(sha),
                size_bytes,
                url: snapshot_url,
            },
        })
    }
}

fn public_skill_from_parts(
    id: Uuid,
    owner: String,
    name: String,
    description: String,
    generation: i64,
    version: i64,
    revision_id: Vec<u8>,
) -> Result<PublicSkill, ApiError> {
    let generation = u64::try_from(generation)
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored generation is invalid"))?;
    let version = u64::try_from(version)
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored release version is invalid"))?;
    let revision: [u8; 32] = revision_id
        .try_into()
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored revision ID is invalid"))?;
    Ok(PublicSkill {
        resource_id: id.to_string(),
        locator: format!("@{owner}/{name}"),
        owner,
        name,
        description,
        generation,
        version,
        revision_id: hex::encode(revision),
    })
}

#[derive(Debug)]
struct SearchCursor {
    owner: String,
    name: String,
    resource_id: Uuid,
}

impl SearchCursor {
    fn from_parts(owner: &str, name: &str, resource_id: Uuid) -> Self {
        Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
            resource_id,
        }
    }

    fn from_row(row: &PublicSkillDbTuple) -> Self {
        Self::from_parts(&row.1, &row.2, row.0)
    }

    fn encode(&self) -> String {
        hex::encode(format!(
            "{}\0{}\0{}",
            self.owner, self.name, self.resource_id
        ))
    }

    fn decode(value: &str) -> Result<Self, ApiError> {
        let bytes = hex::decode(value)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor"))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor"))?;
        let mut parts = text.split('\0');
        let owner = parts.next().unwrap_or_default();
        let name = parts.next().unwrap_or_default();
        let id = parts.next().unwrap_or_default();
        if owner.is_empty() || name.is_empty() || id.is_empty() || parts.next().is_some() {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "invalid search cursor",
            ));
        }
        Ok(Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
            resource_id: Uuid::parse_str(id).map_err(|_| {
                ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor")
            })?,
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

fn skill_frontmatter_name(bytes: &[u8]) -> Result<String, RegistryError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|_| RegistryError::Seed("SKILL.md is not UTF-8".to_owned()))?;
    let Some(rest) = source
        .strip_prefix("---\n")
        .or_else(|| source.strip_prefix("---\r\n"))
    else {
        return Err(RegistryError::Seed(
            "SKILL.md is missing frontmatter".to_owned(),
        ));
    };
    let marker = rest
        .find("\n---")
        .ok_or_else(|| RegistryError::Seed("SKILL.md frontmatter is not terminated".to_owned()))?;
    let value: serde_yaml::Value = serde_yaml::from_str(&rest[..marker])
        .map_err(|error| RegistryError::Seed(error.to_string()))?;
    value
        .as_mapping()
        .and_then(|mapping| mapping.get(serde_yaml::Value::String("name".to_owned())))
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| RegistryError::Seed("SKILL.md is missing name".to_owned()))
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
