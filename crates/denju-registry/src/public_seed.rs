use std::str::FromStr;

use denju_core::{
    AuthorPrincipalId, BlobId, DeterministicSkillSnapshot, OperationId, OwnedSkillEntry,
    ResourceId, ResourceLocator, Revision, parse_skill_document, validate_declared_skill_manifest,
};
use denju_wire::{PublicSkill, PublicSkillDetail, PublicSkillManifest};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    Registry, RegistryError,
    ingest_storage::{manifest_blobs, persist_canonical_blobs, persist_trees},
};

impl Registry {
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
        let expected_blobs = manifest_blobs(snapshot.manifest())
            .map_err(|error| RegistryError::Seed(error.message))?;
        let trees = validate_declared_skill_manifest(snapshot.manifest())
            .map_err(|error| RegistryError::Seed(error.to_string()))?;
        let manifest_json = serde_json::to_value(&manifest)?;
        let snapshot_size = i64::try_from(snapshot.bytes().len())
            .map_err(|_| RegistryError::Seed("snapshot is too large".to_owned()))?;
        let namespace_id = Uuid::now_v7();
        let mut tx = self.worker_pool.begin().await?;
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
        persist_canonical_blobs(&mut tx, &expected_blobs)
            .await
            .map_err(|error| RegistryError::Seed(error.message))?;
        persist_trees(&mut tx, &trees)
            .await
            .map_err(|error| RegistryError::Seed(error.message))?;
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
        for blob in expected_blobs.keys() {
            sqlx::query(
                "INSERT INTO revision_blob_reachability (revision_id,blob_id) VALUES ($1,$2) ON CONFLICT DO NOTHING",
            )
            .bind(revision_id.as_bytes().as_slice())
            .bind(blob.as_bytes().as_slice())
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO resource_blob_reachability (resource_id,blob_id,reference_count) VALUES ($1,$2,1) \
                 ON CONFLICT(resource_id,blob_id) DO UPDATE SET reference_count=resource_blob_reachability.reference_count+1",
            )
            .bind(resource_id.as_uuid())
            .bind(blob.as_bytes().as_slice())
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO namespace_blob_reachability (namespace_id,blob_id,reference_count) VALUES ($1,$2,1) \
                 ON CONFLICT(namespace_id,blob_id) DO UPDATE SET reference_count=namespace_blob_reachability.reference_count+1",
            )
            .bind(namespace_id)
            .bind(blob.as_bytes().as_slice())
            .execute(&mut *tx)
            .await?;
        }
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
                version: Some(1),
                live_private: false,
                revision_id: revision_id.to_string(),
                deprecation: None,
            },
            manifest,
            fork: None,
            redirected_from: None,
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
