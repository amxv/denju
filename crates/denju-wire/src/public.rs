use std::str::FromStr;

use denju_core::{BlobId, SkillManifest, SkillManifestEntry, TreeId};
use serde::{Deserialize, Serialize};

use crate::SkillForkProvenance;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicSkill {
    pub resource_id: String,
    pub locator: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    #[serde(default)]
    pub live_private: bool,
    pub revision_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<SkillDeprecation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicSkillSearchResponse {
    pub items: Vec<PublicSkill>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicSkillDetail {
    #[serde(flatten)]
    pub skill: PublicSkill,
    pub manifest: PublicSkillManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<SkillForkProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirected_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDeprecation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement_resource_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement_locator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicSkillManifest {
    pub root_tree_id: String,
    pub entries: Vec<PublicSkillManifestEntry>,
}

impl PublicSkillManifest {
    pub fn from_core(manifest: &SkillManifest) -> Self {
        Self {
            root_tree_id: manifest.root_tree().to_string(),
            entries: manifest
                .entries()
                .iter()
                .map(PublicSkillManifestEntry::from_core)
                .collect(),
        }
    }

    pub fn to_core(&self) -> Result<SkillManifest, String> {
        let root_tree = TreeId::from_str(&self.root_tree_id).map_err(|error| error.to_string())?;
        let entries = self
            .entries
            .iter()
            .map(PublicSkillManifestEntry::to_core)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SkillManifest::from_declared_parts(root_tree, entries))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PublicSkillManifestEntry {
    File {
        path: String,
        blob_id: String,
        size: u64,
        executable: bool,
    },
    Directory {
        path: String,
    },
    Symlink {
        path: String,
        target: String,
    },
}

impl PublicSkillManifestEntry {
    fn from_core(entry: &SkillManifestEntry) -> Self {
        match entry {
            SkillManifestEntry::File {
                path,
                blob,
                size,
                executable,
            } => Self::File {
                path: path.clone(),
                blob_id: blob.to_string(),
                size: *size,
                executable: *executable,
            },
            SkillManifestEntry::Directory { path } => Self::Directory { path: path.clone() },
            SkillManifestEntry::Symlink { path, target } => Self::Symlink {
                path: path.clone(),
                target: target.clone(),
            },
        }
    }

    fn to_core(&self) -> Result<SkillManifestEntry, String> {
        Ok(match self {
            Self::File {
                path,
                blob_id,
                size,
                executable,
            } => SkillManifestEntry::File {
                path: path.clone(),
                blob: BlobId::from_str(blob_id).map_err(|error| error.to_string())?,
                size: *size,
                executable: *executable,
            },
            Self::Directory { path } => SkillManifestEntry::Directory { path: path.clone() },
            Self::Symlink { path, target } => SkillManifestEntry::Symlink {
                path: path.clone(),
                target: target.clone(),
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotDownload {
    pub sha256: String,
    pub size_bytes: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribedSkill {
    pub resource_id: String,
    pub locator: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub generation: u64,
    pub revision_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<SkillDeprecation>,
    pub content: SubscriptionContent,
    pub manifest: PublicSkillManifest,
    pub snapshot: SnapshotDownload,
    #[serde(default)]
    pub retain_on_delete: bool,
    #[serde(default)]
    pub retained_after_delete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubscriptionContent {
    Release {
        version: u64,
        following_latest: bool,
    },
    PrivateWorkspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionCatalog {
    pub skills: Vec<SubscribedSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionTarget {
    pub resource_id: String,
    pub locator: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub generation: u64,
    pub live_private: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<SkillDeprecation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionMutationRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    #[serde(default)]
    pub release_version: Option<u64>,
    #[serde(default)]
    pub retain_on_delete: bool,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionMutationResponse {
    pub resource_id: String,
    pub subscribed: bool,
    pub pinned_release_version: Option<u64>,
    pub retain_on_delete: bool,
}

#[cfg(test)]
mod tests {
    use denju_core::{OwnedSkillEntry, build_skill_manifest};

    use super::*;

    #[test]
    fn manifest_wire_round_trip_preserves_semantic_contract() {
        let entries = vec![OwnedSkillEntry::File {
            path: "SKILL.md".to_owned(),
            bytes: b"---\nname: review\ndescription: Reviews code.\n---\n".to_vec(),
            executable: false,
        }];
        let manifest = build_skill_manifest("review", &entries).unwrap();
        let wire = PublicSkillManifest::from_core(&manifest);
        assert_eq!(wire.to_core().unwrap(), manifest);
    }

    #[test]
    fn skill_detail_serializes_visible_fork_provenance() {
        let entries = vec![OwnedSkillEntry::File {
            path: "SKILL.md".to_owned(),
            bytes: b"---\nname: review\ndescription: Reviews code.\n---\n".to_vec(),
            executable: false,
        }];
        let manifest = build_skill_manifest("review", &entries).unwrap();
        let detail = PublicSkillDetail {
            skill: PublicSkill {
                resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f002".into(),
                locator: "@alice/review".into(),
                owner: "alice".into(),
                name: "review".into(),
                description: "Reviews code.".into(),
                generation: 3,
                version: None,
                live_private: true,
                revision_id: "11".repeat(32),
                deprecation: None,
            },
            manifest: PublicSkillManifest::from_core(&manifest),
            fork: Some(SkillForkProvenance {
                upstream_resource_id: "01890f47-6a1d-7ad0-8f43-9a4d8c29f003".into(),
                upstream_locator: "@upstream/review".into(),
                created_from_revision_id: "22".repeat(32),
                sync_base_revision_id: "33".repeat(32),
            }),
            redirected_from: None,
        };
        let value = serde_json::to_value(detail).unwrap();
        assert_eq!(value["fork"]["upstream_locator"], "@upstream/review");
        assert_eq!(value["fork"]["created_from_revision_id"], "22".repeat(32));
    }
}
