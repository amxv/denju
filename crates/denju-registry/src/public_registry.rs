use std::str::FromStr;

use denju_core::{
    AuthorPrincipalId, BlobId, DeterministicSkillSnapshot, OperationId, OwnedSkillEntry,
    ResourceId, ResourceKind, ResourceLocator, Revision, parse_skill_document,
};
use denju_wire::{
    ApiError, ApiErrorCode, PublicSkill, PublicSkillDetail, PublicSkillManifest,
    PublicSkillSearchResponse, RequestHash, SkillDeprecation, SnapshotDownload, SubscribedSkill,
    SubscriptionCatalog, SubscriptionMutationKind, SubscriptionMutationRequest,
    SubscriptionMutationResponse, subscription_request_hash,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    Registry, RegistryError, RegistryWake, identity_support::SubscriptionSubject,
    internal_api_error,
};

impl Registry {
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
            sqlx::query_as::<_, PublicSkillSearchRow>(
                "SELECT r.id, n.slug AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, \
                        r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                        replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name \
                 FROM resources r \
                 JOIN namespaces n ON n.id = r.owner_namespace_id \
                 JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = r.latest_release_version \
                 LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                 LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                 WHERE r.visibility = 'public' AND r.kind = 'skill' AND r.deleted_at IS NULL \
                   AND (r.slug ILIKE $1 OR n.slug ILIKE $1 OR r.description ILIKE $1) \
                   AND ((r.deprecated_at IS NOT NULL) > $2 OR ((r.deprecated_at IS NOT NULL) = $2 AND \
                        (n.slug > $3 OR (n.slug = $3 AND r.slug > $4) \
                         OR (n.slug = $3 AND r.slug = $4 AND r.id > $5)))) \
                 ORDER BY (r.deprecated_at IS NOT NULL), n.slug, r.slug, r.id LIMIT $6",
            )
            .bind(&pattern)
            .bind(cursor.deprecated)
            .bind(cursor.owner)
            .bind(cursor.name)
            .bind(cursor.resource_id)
            .bind(i64::from(limit) + 1)
            .fetch_all(&self.pool)
            .await
            .map_err(internal_api_error)?
        } else {
            sqlx::query_as::<_, PublicSkillSearchRow>(
                "SELECT r.id, n.slug AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, \
                        r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                        replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name \
                 FROM resources r \
                 JOIN namespaces n ON n.id = r.owner_namespace_id \
                 JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = r.latest_release_version \
                 LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                 LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                 WHERE r.visibility = 'public' AND r.kind = 'skill' AND r.deleted_at IS NULL \
                   AND (r.slug ILIKE $1 OR n.slug ILIKE $1 OR r.description ILIKE $1) \
                 ORDER BY (r.deprecated_at IS NOT NULL), n.slug, r.slug, r.id LIMIT $2",
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
            .map(PublicSkillSearchRow::into_wire)
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
            request.release_version,
            request.retain_on_delete,
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
            SubscriptionSubject::Installation(installation_id) => {
                sqlx::query_as::<_, (Vec<u8>, Uuid, bool, Option<i64>, bool)>(
                    "SELECT request_hash, resource_id, subscribed, pinned_release_version, retain_on_delete FROM subscription_operations \
                     WHERE installation_id = $1 AND operation_id = $2",
                )
                .bind(installation_id)
                .bind(operation_id.as_uuid())
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal_api_error)?
            }
            SubscriptionSubject::User(user_id) => sqlx::query_as::<
                _,
                (Vec<u8>, Uuid, bool, Option<i64>, bool),
            >(
                "SELECT request_hash, resource_id, subscribed, pinned_release_version, retain_on_delete FROM account_subscription_operations \
                     WHERE user_id = $1 AND operation_id = $2",
            )
            .bind(user_id)
            .bind(operation_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(internal_api_error)?,
        };
        if let Some((
            stored_hash,
            stored_resource,
            subscribed,
            pinned_release_version,
            retain_on_delete,
        )) = replay
        {
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
                pinned_release_version: pinned_release_version
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored pin is invalid"))?,
                retain_on_delete,
            });
        }

        let resource = sqlx::query_as::<_, (i64, String, bool)>(
            "SELECT generation,visibility,deleted_at IS NOT NULL FROM resources WHERE id = $1 AND kind = 'skill'",
        )
        .bind(resource_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "skill not found"))?;
        if kind == SubscriptionMutationKind::Subscribe && (resource.1 != "public" || resource.2) {
            return Err(ApiError::new(
                ApiErrorCode::NotFound,
                "public skill not found",
            ));
        }
        let generation = u64::try_from(resource.0)
            .map_err(|_| ApiError::new(ApiErrorCode::Internal, "resource generation is invalid"))?;
        if generation != request.expected_generation {
            return Err(ApiError::new(
                ApiErrorCode::GenerationConflict,
                format!("resource generation changed to {generation}"),
            ));
        }

        if kind == SubscriptionMutationKind::Unsubscribe
            && (request.release_version.is_some() || request.retain_on_delete)
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "unsubscribe does not accept release or retention options",
            ));
        }
        let pinned_release_version = request
            .release_version
            .map(i64::try_from)
            .transpose()
            .map_err(|_| {
                ApiError::new(
                    ApiErrorCode::InvalidRequest,
                    "release version exceeds database range",
                )
            })?;
        if let Some(version) = pinned_release_version {
            let exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM skill_releases WHERE resource_id=$1 AND version=$2)",
            )
            .bind(resource_id.as_uuid())
            .bind(version)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            if !exists {
                return Err(ApiError::new(
                    ApiErrorCode::NotFound,
                    "requested release does not exist",
                ));
            }
        }

        let subscribed = kind == SubscriptionMutationKind::Subscribe;
        match (subject, kind) {
            (
                SubscriptionSubject::Installation(installation_id),
                SubscriptionMutationKind::Subscribe,
            ) => {
                sqlx::query(
                    "INSERT INTO installation_subscriptions (installation_id, resource_id, pinned_release_version, retain_on_delete) \
                     VALUES ($1, $2, $3, $4) ON CONFLICT(installation_id,resource_id) DO UPDATE \
                     SET pinned_release_version=excluded.pinned_release_version,retain_on_delete=excluded.retain_on_delete",
                )
                .bind(installation_id)
                .bind(resource_id.as_uuid())
                .bind(pinned_release_version)
                .bind(request.retain_on_delete)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            (
                SubscriptionSubject::Installation(installation_id),
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
            (SubscriptionSubject::User(user_id), SubscriptionMutationKind::Subscribe) => {
                sqlx::query(
                    "INSERT INTO account_subscriptions (user_id, resource_id, pinned_release_version, retain_on_delete) VALUES ($1, $2, $3, $4) \
                     ON CONFLICT(user_id,resource_id) DO UPDATE SET pinned_release_version=excluded.pinned_release_version,retain_on_delete=excluded.retain_on_delete",
                )
                .bind(user_id)
                .bind(resource_id.as_uuid())
                .bind(pinned_release_version)
                .bind(request.retain_on_delete)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            (SubscriptionSubject::User(user_id), SubscriptionMutationKind::Unsubscribe) => {
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
            SubscriptionSubject::Installation(installation_id) => {
                sqlx::query(
                    "INSERT INTO subscription_operations \
                     (installation_id, operation_id, request_hash, action, resource_id, subscribed, pinned_release_version, retain_on_delete) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                )
                .bind(installation_id)
                .bind(operation_id.as_uuid())
                .bind(supplied_hash.as_bytes().as_slice())
                .bind(action)
                .bind(resource_id.as_uuid())
                .bind(subscribed)
                .bind(pinned_release_version)
                .bind(request.retain_on_delete)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
            SubscriptionSubject::User(user_id) => {
                sqlx::query(
                    "INSERT INTO account_subscription_operations \
                     (user_id, operation_id, request_hash, action, resource_id, subscribed, pinned_release_version, retain_on_delete) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                )
                .bind(user_id)
                .bind(operation_id.as_uuid())
                .bind(supplied_hash.as_bytes().as_slice())
                .bind(action)
                .bind(resource_id.as_uuid())
                .bind(subscribed)
                .bind(pinned_release_version)
                .bind(request.retain_on_delete)
                .execute(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            }
        }
        tx.commit().await.map_err(internal_api_error)?;
        // Subscription membership changes the per-connection reverse watch set. Wake local
        // listeners without exposing any resource ID so a long-lived daemon reconnects and
        // rebuilds that disposable index from authoritative subscription rows.
        let _ = self.wake_tx.send(RegistryWake::ResyncAll);

        Ok(SubscriptionMutationResponse {
            resource_id: resource_id.to_string(),
            subscribed,
            pinned_release_version: request.release_version,
            retain_on_delete: request.retain_on_delete,
        })
    }

    pub async fn subscription_catalog(
        &self,
        bearer: &str,
    ) -> Result<SubscriptionCatalog, ApiError> {
        let subject = self.subscription_subject(bearer).await?;
        let rows = match subject {
            SubscriptionSubject::Installation(installation_id) => {
                sqlx::query_as::<_, SubscriptionRow>(
                    "SELECT r.id, COALESCE(n.slug,r.deleted_owner_slug) AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, \
                            sr.manifest_json, sr.snapshot_key, sr.snapshot_sha256, sr.snapshot_size, s.pinned_release_version, \
                            s.retain_on_delete, r.deleted_at IS NOT NULL AS retained_after_delete, \
                            r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                            replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name \
                     FROM installation_subscriptions s \
                     JOIN resources r ON r.id = s.resource_id \
                     LEFT JOIN namespaces n ON n.id = r.owner_namespace_id \
                     LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                     LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                     JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = CASE \
                         WHEN r.deleted_at IS NOT NULL THEN r.tombstone_release_version \
                         ELSE COALESCE(s.pinned_release_version,r.latest_release_version) END \
                     WHERE s.installation_id = $1 AND r.kind = 'skill' AND ( \
                         (r.deleted_at IS NULL AND r.visibility = 'public') OR \
                         (r.deleted_at IS NOT NULL AND s.retain_on_delete AND r.tombstone_release_version IS NOT NULL)) \
                     ORDER BY COALESCE(n.slug,r.deleted_owner_slug), r.slug, r.id",
                )
                .bind(installation_id)
                .fetch_all(&self.pool)
                .await
                .map_err(internal_api_error)?
            }
            SubscriptionSubject::User(user_id) => {
                sqlx::query_as::<_, SubscriptionRow>(
                    "SELECT r.id, COALESCE(n.slug,r.deleted_owner_slug) AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, \
                            sr.manifest_json, sr.snapshot_key, sr.snapshot_sha256, sr.snapshot_size, s.pinned_release_version, \
                            s.retain_on_delete, r.deleted_at IS NOT NULL AS retained_after_delete, \
                            r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                            replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name \
                     FROM account_subscriptions s \
                     JOIN resources r ON r.id = s.resource_id \
                     LEFT JOIN namespaces n ON n.id = r.owner_namespace_id \
                     LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
                     LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
                     JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = CASE \
                         WHEN r.deleted_at IS NOT NULL THEN r.tombstone_release_version \
                         ELSE COALESCE(s.pinned_release_version,r.latest_release_version) END \
                     WHERE s.user_id = $1 AND r.kind = 'skill' AND ( \
                         (r.deleted_at IS NULL AND r.visibility = 'public') OR \
                         (r.deleted_at IS NOT NULL AND s.retain_on_delete AND r.tombstone_release_version IS NOT NULL)) \
                     ORDER BY COALESCE(n.slug,r.deleted_owner_slug), r.slug, r.id",
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
            "SELECT id FROM resources WHERE owner_namespace_id = $1 AND kind = 'skill' AND slug = $2 AND deleted_at IS NULL",
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
            "DELETE FROM resource_redirects WHERE namespace_id=$1 AND kind='skill' AND old_slug=$2",
        )
        .bind(namespace_id)
        .bind(&yaml_name)
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
        sqlx::query(
            "INSERT INTO revisions (revision_id,root_tree_id,author_principal_id,operation_id) \
             VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING",
        )
        .bind(revision_id.as_bytes().as_slice())
        .bind(snapshot.manifest().root_tree().as_bytes().as_slice())
        .bind(author_id.as_uuid())
        .bind(operation_id.as_uuid())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO resource_revision_snapshots \
             (resource_id,revision_id,manifest_json,snapshot_key,snapshot_sha256,snapshot_size) \
             VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING",
        )
        .bind(resource_id.as_uuid())
        .bind(revision_id.as_bytes().as_slice())
        .bind(serde_json::to_value(&manifest)?)
        .bind(&snapshot_key)
        .bind(snapshot_sha256.as_slice())
        .bind(snapshot_size)
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
                deprecation: None,
            },
            manifest,
            redirected_from: None,
        })
    }

    async fn public_skill_detail(
        &self,
        owner: &str,
        name: &str,
    ) -> Result<PublicSkillDetail, ApiError> {
        let active = sqlx::query_as::<_, PublicSkillDetailRow>(
            "SELECT r.id, n.slug AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, sr.manifest_json, \
                    r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                    replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name \
             FROM resources r \
             JOIN namespaces n ON n.id = r.owner_namespace_id \
             JOIN skill_releases sr ON sr.resource_id = r.id AND sr.version = r.latest_release_version \
             LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
             LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
             WHERE r.visibility = 'public' AND r.kind = 'skill' AND r.deleted_at IS NULL AND n.slug = $1 AND r.slug = $2",
        )
        .bind(owner)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?;
        if let Some(row) = active {
            return row.into_wire(None);
        }
        let redirected = sqlx::query_as::<_, PublicSkillDetailRow>(
            "SELECT r.id, target_owner.slug AS owner, r.slug AS name, r.description, r.generation, sr.version, sr.revision_id, sr.manifest_json, \
                    r.deprecated_at IS NOT NULL AS deprecated, replacement.id AS replacement_id, \
                    replacement_owner.slug AS replacement_owner, replacement.slug AS replacement_name \
             FROM resource_redirects rr JOIN namespaces old_owner ON old_owner.id=rr.namespace_id \
             JOIN resources r ON r.id=rr.target_resource_id AND r.deleted_at IS NULL \
             JOIN namespaces target_owner ON target_owner.id=r.owner_namespace_id \
             JOIN skill_releases sr ON sr.resource_id=r.id AND sr.version=r.latest_release_version \
             LEFT JOIN resources replacement ON replacement.id=r.deprecation_replacement_resource_id AND replacement.deleted_at IS NULL \
             LEFT JOIN namespaces replacement_owner ON replacement_owner.id=replacement.owner_namespace_id \
             WHERE old_owner.slug=$1 AND rr.kind='skill' AND rr.old_slug=$2 AND r.visibility='public'",
        )
        .bind(owner)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "public skill not found"))?;
        redirected.into_wire(Some(format!("@{owner}/{name}")))
    }
}

#[derive(sqlx::FromRow)]
struct PublicSkillSearchRow {
    id: Uuid,
    owner: String,
    name: String,
    description: String,
    generation: i64,
    version: i64,
    revision_id: Vec<u8>,
    deprecated: bool,
    replacement_id: Option<Uuid>,
    replacement_owner: Option<String>,
    replacement_name: Option<String>,
}

impl PublicSkillSearchRow {
    fn into_wire(self) -> Result<PublicSkill, ApiError> {
        let deprecation = self.deprecated.then(|| SkillDeprecation {
            replacement_resource_id: self.replacement_id.map(|id| id.to_string()),
            replacement_locator: self
                .replacement_owner
                .zip(self.replacement_name)
                .map(|(owner, name)| format!("@{owner}/{name}")),
        });
        public_skill_from_parts(PublicSkillParts {
            id: self.id,
            owner: self.owner,
            name: self.name,
            description: self.description,
            generation: self.generation,
            version: self.version,
            revision_id: self.revision_id,
            deprecation,
        })
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
    deprecated: bool,
    replacement_id: Option<Uuid>,
    replacement_owner: Option<String>,
    replacement_name: Option<String>,
}

impl PublicSkillDetailRow {
    fn into_wire(self, redirected_from: Option<String>) -> Result<PublicSkillDetail, ApiError> {
        let deprecation = self.deprecated.then(|| SkillDeprecation {
            replacement_resource_id: self.replacement_id.map(|id| id.to_string()),
            replacement_locator: self
                .replacement_owner
                .zip(self.replacement_name)
                .map(|(owner, name)| format!("@{owner}/{name}")),
        });
        let skill = public_skill_from_parts(PublicSkillParts {
            id: self.id,
            owner: self.owner,
            name: self.name,
            description: self.description,
            generation: self.generation,
            version: self.version,
            revision_id: self.revision_id,
            deprecation,
        })?;
        let manifest = serde_json::from_value(self.manifest_json)
            .map_err(|error| ApiError::new(ApiErrorCode::Internal, error.to_string()))?;
        Ok(PublicSkillDetail {
            skill,
            manifest,
            redirected_from,
        })
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
    pinned_release_version: Option<i64>,
    retain_on_delete: bool,
    retained_after_delete: bool,
    deprecated: bool,
    replacement_id: Option<Uuid>,
    replacement_owner: Option<String>,
    replacement_name: Option<String>,
}

impl SubscriptionRow {
    fn into_wire(self, snapshot_url: String) -> Result<SubscribedSkill, ApiError> {
        let deprecation = self.deprecated.then(|| SkillDeprecation {
            replacement_resource_id: self.replacement_id.map(|id| id.to_string()),
            replacement_locator: self
                .replacement_owner
                .zip(self.replacement_name)
                .map(|(owner, name)| format!("@{owner}/{name}")),
        });
        let skill = public_skill_from_parts(PublicSkillParts {
            id: self.id,
            owner: self.owner,
            name: self.name,
            description: self.description,
            generation: self.generation,
            version: self.version,
            revision_id: self.revision_id,
            deprecation,
        })?;
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
            following_latest: !self.retained_after_delete && self.pinned_release_version.is_none(),
            retained_after_delete: self.retained_after_delete,
            retain_on_delete: self.retain_on_delete,
        })
    }
}

struct PublicSkillParts {
    id: Uuid,
    owner: String,
    name: String,
    description: String,
    generation: i64,
    version: i64,
    revision_id: Vec<u8>,
    deprecation: Option<SkillDeprecation>,
}

fn public_skill_from_parts(parts: PublicSkillParts) -> Result<PublicSkill, ApiError> {
    let generation = u64::try_from(parts.generation)
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored generation is invalid"))?;
    let version = u64::try_from(parts.version)
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored release version is invalid"))?;
    let revision: [u8; 32] = parts
        .revision_id
        .try_into()
        .map_err(|_| ApiError::new(ApiErrorCode::Internal, "stored revision ID is invalid"))?;
    Ok(PublicSkill {
        resource_id: parts.id.to_string(),
        locator: format!("@{}/{}", parts.owner, parts.name),
        owner: parts.owner,
        name: parts.name,
        description: parts.description,
        generation,
        version,
        revision_id: hex::encode(revision),
        deprecation: parts.deprecation,
    })
}

#[derive(Debug)]
struct SearchCursor {
    deprecated: bool,
    owner: String,
    name: String,
    resource_id: Uuid,
}

impl SearchCursor {
    fn from_parts(deprecated: bool, owner: &str, name: &str, resource_id: Uuid) -> Self {
        Self {
            deprecated,
            owner: owner.to_owned(),
            name: name.to_owned(),
            resource_id,
        }
    }

    fn from_row(row: &PublicSkillSearchRow) -> Self {
        Self::from_parts(row.deprecated, &row.owner, &row.name, row.id)
    }

    fn encode(&self) -> String {
        hex::encode(format!(
            "{}\0{}\0{}\0{}",
            u8::from(self.deprecated),
            self.owner,
            self.name,
            self.resource_id
        ))
    }

    fn decode(value: &str) -> Result<Self, ApiError> {
        let bytes = hex::decode(value)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor"))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor"))?;
        let mut parts = text.split('\0');
        let deprecated = parts.next().unwrap_or_default();
        let owner = parts.next().unwrap_or_default();
        let name = parts.next().unwrap_or_default();
        let id = parts.next().unwrap_or_default();
        if !matches!(deprecated, "0" | "1")
            || owner.is_empty()
            || name.is_empty()
            || id.is_empty()
            || parts.next().is_some()
        {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "invalid search cursor",
            ));
        }
        Ok(Self {
            deprecated: deprecated == "1",
            owner: owner.to_owned(),
            name: name.to_owned(),
            resource_id: Uuid::parse_str(id).map_err(|_| {
                ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor")
            })?,
        })
    }
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
