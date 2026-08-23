use serde::{Deserialize, Serialize};

use crate::{PackDetail, PublicSkillDetail};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogResourceKind {
    Skill,
    Pack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogVisibility {
    Public,
    Private,
    Team,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSource {
    Public,
    Owned,
    PrivateShare,
    Team,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SearchSort {
    #[default]
    Relevance,
    Stars,
}

impl SearchSort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Stars => "stars",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CatalogSearchQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default)]
    pub sort: SearchSort,
    #[serde(default)]
    pub following: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CatalogTopQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogResource {
    pub resource_id: String,
    pub kind: CatalogResourceKind,
    pub locator: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topics: Vec<String>,
    pub source: CatalogSource,
    pub visibility: CatalogVisibility,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    pub star_count: u64,
    #[serde(default)]
    pub viewer_starred: bool,
    #[serde(default)]
    pub deprecated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_upstream_locator: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pack_members: Vec<String>,
    /// Stable integer rank used by the CLI when merging registry and local-only metadata.
    pub relevance_rank: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogSearchResponse {
    pub items: Vec<CatalogResource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileUserRef {
    pub user_id: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileConnections {
    pub count: u64,
    pub users: Vec<ProfileUserRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_id: String,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    pub followers_visible: bool,
    pub following_visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub followers: Option<ProfileConnections>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub following: Option<ProfileConnections>,
    #[serde(default)]
    pub viewer_follows: bool,
    pub public_skills: Vec<CatalogResource>,
    pub public_packs: Vec<CatalogResource>,
    pub public_forks: Vec<CatalogResource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileUpdateRequest {
    pub operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    pub followers_visible: bool,
    pub following_visible: bool,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileUpdateResponse {
    pub user_id: String,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    pub followers_visible: bool,
    pub following_visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowMutationRequest {
    pub operation_id: String,
    pub target_user_id: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowMutationResponse {
    pub target_user_id: String,
    pub username: String,
    pub following: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarMutationRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarMutationResponse {
    pub resource_id: String,
    pub locator: String,
    pub starred: bool,
    pub star_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceTopicsRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub expected_generation: u64,
    pub topics: Vec<String>,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceTopicsResponse {
    pub resource_id: String,
    pub locator: String,
    pub generation: u64,
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportResourceRequest {
    pub operation_id: String,
    pub resource_id: String,
    pub reason: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportResourceResponse {
    pub report_id: String,
    pub resource_id: String,
    pub accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum UniversalShowResponse {
    Profile(UserProfile),
    Skill(PublicSkillDetail),
    Pack(PackDetail),
}
