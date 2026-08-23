use std::{cmp::Ordering, fs};

use denju_client::ClientError;
use denju_core::{OperationId, build_skill_manifest, parse_skill_document};
use denju_local::{AnonymousFollowRecord, LocalDiscoveryRecord, read_skill_source};
use denju_wire::{
    ApiErrorCode, CatalogResource, CatalogResourceKind, CatalogSearchQuery, CatalogSearchResponse,
    CatalogSource, CatalogTopQuery, CatalogVisibility, CliErrorCode, FollowMutationKind,
    FollowMutationRequest, ProfileUpdateRequest, ProfileUpdateResponse, PublicSkill,
    PublicSkillDetail, PublicSkillManifest, ReportResourceRequest, ReportResourceResponse,
    ResourceTopicsRequest, ResourceTopicsResponse, SearchSort, SkillForkProvenance,
    StarMutationKind, StarMutationRequest, StarMutationResponse, UniversalShowResponse,
    UserProfile, follow_request_hash, profile_update_request_hash, report_resource_request_hash,
    resource_topics_request_hash, star_request_hash,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    public::{catalog_context, client_error, installed_context, local_error, now_unix_ms},
    setup::RuntimeError,
};

const MAX_CLI_SEARCH_LIMIT: u32 = 50;

#[derive(Debug, Clone, Serialize)]
pub struct FollowOutcome {
    pub state: &'static str,
    pub user_id: String,
    pub username: String,
    pub synchronized: bool,
}

pub async fn search(
    query: &str,
    sort: SearchSort,
    following: bool,
    topic: Option<&str>,
    limit: u32,
    cursor: Option<&str>,
) -> Result<CatalogSearchResponse, RuntimeError> {
    let context = catalog_context().await?;
    let limit = limit.clamp(1, MAX_CLI_SEARCH_LIMIT);
    let remote = context
        .client
        .search_catalog(&CatalogSearchQuery {
            q: query.to_owned(),
            limit: Some(limit),
            cursor: cursor.map(str::to_owned),
            sort,
            following,
            topic: topic.map(str::to_owned),
        })
        .await
        .map_err(client_error)?;

    if following || topic.is_some() || cursor.is_some() {
        return Ok(remote);
    }

    refresh_local_metadata(&context).await?;
    let local = context
        .db
        .local_discovery_records()
        .await
        .map_err(local_error)?;
    Ok(merge_local_results(remote, local, query, sort, limit))
}

pub async fn top(
    topic: Option<&str>,
    limit: u32,
    cursor: Option<&str>,
) -> Result<CatalogSearchResponse, RuntimeError> {
    let context = catalog_context().await?;
    context
        .client
        .top_catalog(&CatalogTopQuery {
            limit: Some(limit.clamp(1, MAX_CLI_SEARCH_LIMIT)),
            cursor: cursor.map(str::to_owned),
            topic: topic.map(str::to_owned),
        })
        .await
        .map_err(client_error)
}

pub async fn show(
    locator: &str,
    followers_cursor: Option<&str>,
    following_cursor: Option<&str>,
) -> Result<UniversalShowResponse, RuntimeError> {
    let context = catalog_context().await?;
    if followers_cursor.is_none()
        && following_cursor.is_none()
        && let Some(skill) = local_skill_detail(&context, locator).await?
    {
        return Ok(UniversalShowResponse::Skill(skill));
    }
    match context
        .client
        .universal_show(locator, followers_cursor, following_cursor)
        .await
    {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            let not_found = matches!(
                &error,
                ClientError::Registry(api) if api.code == ApiErrorCode::NotFound
            );
            if not_found
                && followers_cursor.is_none()
                && following_cursor.is_none()
                && let Some(skill) = local_skill_detail(&context, locator).await?
            {
                return Ok(UniversalShowResponse::Skill(skill));
            }
            Err(client_error(error))
        }
    }
}

pub async fn follow(username: &str) -> Result<FollowOutcome, RuntimeError> {
    mutate_follow(username, FollowMutationKind::Follow).await
}

pub async fn unfollow(username: &str) -> Result<FollowOutcome, RuntimeError> {
    mutate_follow(username, FollowMutationKind::Unfollow).await
}

async fn mutate_follow(
    username: &str,
    kind: FollowMutationKind,
) -> Result<FollowOutcome, RuntimeError> {
    let context = catalog_context().await?;
    let profile = profile_from_show(
        context
            .client
            .universal_show(username, None, None)
            .await
            .map_err(client_error)?,
    )?;
    let authenticated = context
        .db
        .identity()
        .await
        .map_err(local_error)?
        .is_some_and(|identity| identity.session_backend.is_some());

    if authenticated {
        let operation_id = new_operation_id()?;
        let request_hash =
            follow_request_hash(kind, &operation_id, &profile.user_id).map_err(internal_error)?;
        let request = FollowMutationRequest {
            operation_id,
            target_user_id: profile.user_id.clone(),
            request_hash: request_hash.to_string(),
        };
        let response = match kind {
            FollowMutationKind::Follow => context.client.follow(&request).await,
            FollowMutationKind::Unfollow => context.client.unfollow(&request).await,
        }
        .map_err(client_error)?;
        context
            .db
            .remove_anonymous_follow(profile.user_id.clone())
            .await
            .map_err(local_error)?;
        return Ok(FollowOutcome {
            state: if response.following {
                "following"
            } else {
                "not_following"
            },
            user_id: response.target_user_id,
            username: response.username,
            synchronized: true,
        });
    }

    match kind {
        FollowMutationKind::Follow => {
            context
                .db
                .upsert_anonymous_follow(
                    AnonymousFollowRecord {
                        user_id: profile.user_id.clone(),
                        username: profile.username.clone(),
                    },
                    now_unix_ms(),
                )
                .await
                .map_err(local_error)?;
        }
        FollowMutationKind::Unfollow => {
            context
                .db
                .remove_anonymous_follow(profile.user_id.clone())
                .await
                .map_err(local_error)?;
        }
    }
    Ok(FollowOutcome {
        state: if kind == FollowMutationKind::Follow {
            "following_locally"
        } else {
            "not_following"
        },
        user_id: profile.user_id,
        username: profile.username,
        synchronized: false,
    })
}

pub async fn adopt_anonymous_follows() -> Result<usize, RuntimeError> {
    let context = installed_context(true).await?;
    let follows = context.db.anonymous_follows().await.map_err(local_error)?;
    let mut adopted = 0usize;
    for follow in follows {
        let operation_id = new_operation_id()?;
        let request_hash =
            follow_request_hash(FollowMutationKind::Follow, &operation_id, &follow.user_id)
                .map_err(internal_error)?;
        context
            .client
            .follow(&FollowMutationRequest {
                operation_id,
                target_user_id: follow.user_id.clone(),
                request_hash: request_hash.to_string(),
            })
            .await
            .map_err(client_error)?;
        context
            .db
            .remove_anonymous_follow(follow.user_id)
            .await
            .map_err(local_error)?;
        adopted += 1;
    }
    Ok(adopted)
}

pub async fn star(locator: &str) -> Result<StarMutationResponse, RuntimeError> {
    mutate_star(locator, StarMutationKind::Star).await
}

pub async fn unstar(locator: &str) -> Result<StarMutationResponse, RuntimeError> {
    mutate_star(locator, StarMutationKind::Unstar).await
}

async fn mutate_star(
    locator: &str,
    kind: StarMutationKind,
) -> Result<StarMutationResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let shown = context
        .client
        .universal_show(locator, None, None)
        .await
        .map_err(client_error)?;
    let (resource_id, _) = resource_identity(&shown)?;
    let operation_id = new_operation_id()?;
    let request_hash =
        star_request_hash(kind, &operation_id, &resource_id).map_err(internal_error)?;
    let request = StarMutationRequest {
        operation_id,
        resource_id,
        request_hash: request_hash.to_string(),
    };
    match kind {
        StarMutationKind::Star => context.client.star(&request).await,
        StarMutationKind::Unstar => context.client.unstar(&request).await,
    }
    .map_err(client_error)
}

pub async fn update_topics(
    locator: &str,
    topics: Vec<String>,
) -> Result<ResourceTopicsResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let shown = context
        .client
        .universal_show(locator, None, None)
        .await
        .map_err(client_error)?;
    let (resource_id, generation) = resource_identity(&shown)?;
    let operation_id = new_operation_id()?;
    let request_hash =
        resource_topics_request_hash(&operation_id, &resource_id, generation, &topics)
            .map_err(internal_error)?;
    context
        .client
        .update_resource_topics(&ResourceTopicsRequest {
            operation_id,
            resource_id,
            expected_generation: generation,
            topics,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub async fn report(locator: &str, reason: &str) -> Result<ReportResourceResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let shown = context
        .client
        .universal_show(locator, None, None)
        .await
        .map_err(client_error)?;
    let (resource_id, _) = resource_identity(&shown)?;
    let operation_id = new_operation_id()?;
    let request_hash = report_resource_request_hash(&operation_id, &resource_id, reason)
        .map_err(internal_error)?;
    context
        .client
        .report_resource(&ReportResourceRequest {
            operation_id,
            resource_id,
            reason: reason.to_owned(),
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

pub async fn update_profile(
    bio: Option<String>,
    clear_bio: bool,
    followers_visible: Option<bool>,
    following_visible: Option<bool>,
) -> Result<ProfileUpdateResponse, RuntimeError> {
    let context = installed_context(true).await?;
    let identity = context
        .db
        .identity()
        .await
        .map_err(local_error)?
        .ok_or_else(|| {
            RuntimeError::new(CliErrorCode::CredentialUnavailable, "identity required")
        })?;
    let current = profile_from_show(
        context
            .client
            .universal_show(&identity.username, None, None)
            .await
            .map_err(client_error)?,
    )?;
    let bio = if clear_bio { None } else { bio.or(current.bio) };
    let followers_visible = followers_visible.unwrap_or(current.followers_visible);
    let following_visible = following_visible.unwrap_or(current.following_visible);
    let operation_id = new_operation_id()?;
    let request_hash = profile_update_request_hash(
        &operation_id,
        bio.as_deref(),
        followers_visible,
        following_visible,
    )
    .map_err(internal_error)?;
    context
        .client
        .update_profile(&ProfileUpdateRequest {
            operation_id,
            bio,
            followers_visible,
            following_visible,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(client_error)
}

async fn refresh_local_metadata(
    context: &crate::public::InstalledContext,
) -> Result<(), RuntimeError> {
    for record in context.db.owned_skills().await.map_err(local_error)? {
        let skill_md = context
            .paths
            .skills
            .join(&record.owner)
            .join(&record.skill_name)
            .join("SKILL.md");
        let Ok(bytes) = fs::read(skill_md) else {
            continue;
        };
        let Ok(document) = parse_skill_document(&record.skill_name, &bytes) else {
            continue;
        };
        context
            .db
            .upsert_skill_discovery_metadata(
                record.resource_id,
                document.frontmatter().description().to_owned(),
                document.frontmatter().license().map(str::to_owned),
                document.frontmatter().compatibility().map(str::to_owned),
                now_unix_ms(),
            )
            .await
            .map_err(local_error)?;
    }
    Ok(())
}

fn merge_local_results(
    mut remote: CatalogSearchResponse,
    local: Vec<LocalDiscoveryRecord>,
    query: &str,
    sort: SearchSort,
    limit: u32,
) -> CatalogSearchResponse {
    let max_items = usize::try_from(limit)
        .unwrap_or(MAX_CLI_SEARCH_LIMIT as usize)
        .saturating_mul(2);
    for record in local {
        let relevance_rank = local_relevance(&record, query);
        if !query.trim().is_empty() && relevance_rank == 0 {
            continue;
        }
        let existing = remote
            .items
            .iter()
            .position(|item| item.resource_id == record.resource_id);
        let remote_item = existing.map(|index| remote.items[index].clone());
        let item = CatalogResource {
            resource_id: record.resource_id,
            kind: CatalogResourceKind::Skill,
            locator: record.locator,
            owner: record.owner,
            name: record.skill_name,
            description: record.description,
            license: record.license,
            compatibility: record.compatibility,
            topics: remote_item
                .as_ref()
                .map(|item| item.topics.clone())
                .unwrap_or_default(),
            source: CatalogSource::Local,
            visibility: CatalogVisibility::Private,
            generation: u64::try_from(record.resource_generation).unwrap_or_default(),
            version: None,
            star_count: remote_item.as_ref().map_or(0, |item| item.star_count),
            viewer_starred: remote_item.as_ref().is_some_and(|item| item.viewer_starred),
            deprecated: remote_item.as_ref().is_some_and(|item| item.deprecated),
            fork_upstream_locator: record.fork_upstream_locator.or_else(|| {
                remote_item
                    .as_ref()
                    .and_then(|item| item.fork_upstream_locator.clone())
            }),
            pack_members: Vec::new(),
            relevance_rank: remote_item.as_ref().map_or(relevance_rank, |item| {
                relevance_rank.max(item.relevance_rank)
            }),
        };
        if let Some(index) = existing {
            remote.items[index] = item;
        } else if remote.items.len() < max_items {
            remote.items.push(item);
        }
    }
    remote
        .items
        .sort_by(|left, right| catalog_order(left, right, sort));
    remote
}

async fn local_skill_detail(
    context: &crate::public::InstalledContext,
    locator: &str,
) -> Result<Option<PublicSkillDetail>, RuntimeError> {
    let Some(record) = context
        .db
        .owned_skills()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|record| record.locator == locator)
    else {
        return Ok(None);
    };
    let canonical = context
        .paths
        .skills
        .join(&record.owner)
        .join(&record.skill_name);
    let working = fs::canonicalize(&canonical).map_err(local_error)?;
    let generations = fs::canonicalize(&context.paths.generations).map_err(local_error)?;
    if !working.starts_with(&generations) {
        return Err(RuntimeError::new(
            CliErrorCode::LocalState,
            format!(
                "{} resolves outside Denju managed generations",
                record.locator
            ),
        )
        .recovery("denju doctor"));
    }
    let entries = read_skill_source(&working).map_err(local_error)?;
    let manifest = build_skill_manifest(&record.skill_name, &entries)
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error.to_string()))?;
    let skill_md = fs::read(working.join("SKILL.md")).map_err(local_error)?;
    let document = parse_skill_document(&record.skill_name, &skill_md)
        .map_err(|error| RuntimeError::new(CliErrorCode::ContentVerification, error.to_string()))?;
    let revision_id = context
        .db
        .workspace_state(record.resource_id.clone())
        .await
        .map_err(local_error)?
        .map(|state| state.local_head_revision_id)
        .unwrap_or_else(|| record.desired_revision_id.clone());
    let fork = context
        .db
        .local_forks()
        .await
        .map_err(local_error)?
        .into_iter()
        .find(|fork| fork.resource_id == record.resource_id)
        .map(|fork| SkillForkProvenance {
            upstream_resource_id: fork.upstream_resource_id,
            upstream_locator: fork.upstream_locator,
            created_from_revision_id: fork.created_from_revision_id,
            sync_base_revision_id: fork.sync_base_revision_id,
        });
    Ok(Some(PublicSkillDetail {
        skill: PublicSkill {
            resource_id: record.resource_id,
            locator: record.locator,
            owner: record.owner,
            name: record.skill_name,
            description: document.frontmatter().description().to_owned(),
            generation: u64::try_from(record.resource_generation).unwrap_or_default(),
            version: None,
            live_private: true,
            revision_id,
            deprecation: None,
        },
        manifest: PublicSkillManifest::from_core(&manifest),
        fork,
        redirected_from: None,
    }))
}

fn local_relevance(record: &LocalDiscoveryRecord, query: &str) -> i64 {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return 0;
    }
    let locator = record.locator.to_ascii_lowercase();
    let name = record.skill_name.to_ascii_lowercase();
    if locator == query {
        return 12_000;
    }
    if name == query {
        return 10_000;
    }
    if name.starts_with(&query) {
        return 7_000;
    }
    let mut text = format!(
        "{} {} {} {} {}",
        record.owner,
        record.skill_name,
        record.description,
        record.license.as_deref().unwrap_or_default(),
        record.compatibility.as_deref().unwrap_or_default(),
    )
    .to_ascii_lowercase();
    if let Some(upstream) = record.fork_upstream_locator.as_deref() {
        text.push(' ');
        text.push_str(&upstream.to_ascii_lowercase());
    }
    if text.contains(&query) { 2_000 } else { 0 }
}

fn catalog_order(left: &CatalogResource, right: &CatalogResource, sort: SearchSort) -> Ordering {
    left.deprecated
        .cmp(&right.deprecated)
        .then_with(|| match sort {
            SearchSort::Relevance => right
                .relevance_rank
                .cmp(&left.relevance_rank)
                .then_with(|| right.star_count.cmp(&left.star_count)),
            SearchSort::Stars => right
                .star_count
                .cmp(&left.star_count)
                .then_with(|| right.relevance_rank.cmp(&left.relevance_rank)),
        })
        .then_with(|| left.locator.cmp(&right.locator))
        .then_with(|| left.resource_id.cmp(&right.resource_id))
}

fn profile_from_show(shown: UniversalShowResponse) -> Result<UserProfile, RuntimeError> {
    match shown {
        UniversalShowResponse::Profile(profile) => Ok(profile),
        _ => Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "expected an immutable @username",
        )),
    }
}

fn resource_identity(shown: &UniversalShowResponse) -> Result<(String, u64), RuntimeError> {
    match shown {
        UniversalShowResponse::Skill(skill) => {
            Ok((skill.skill.resource_id.clone(), skill.skill.generation))
        }
        UniversalShowResponse::Pack(pack) => {
            Ok((pack.pack.resource_id.clone(), pack.pack.generation))
        }
        UniversalShowResponse::Profile(_) => Err(RuntimeError::new(
            CliErrorCode::InvalidArguments,
            "operation requires a skill or pack locator",
        )),
    }
}

fn new_operation_id() -> Result<String, RuntimeError> {
    OperationId::from_uuid(Uuid::now_v7())
        .map(|value| value.to_string())
        .map_err(internal_error)
}

fn internal_error(error: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::new(CliErrorCode::Internal, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_record() -> LocalDiscoveryRecord {
        LocalDiscoveryRecord {
            resource_id: "local-1".into(),
            locator: "@alice/review".into(),
            owner: "alice".into(),
            skill_name: "review".into(),
            resource_generation: 2,
            description: "Reviews Rust agent code".into(),
            license: Some("MIT".into()),
            compatibility: Some("Rust projects".into()),
            fork_upstream_locator: Some("@upstream/review".into()),
        }
    }

    #[test]
    fn local_metadata_ranking_never_uses_skill_body() {
        let record = local_record();
        assert_eq!(local_relevance(&record, "review"), 10_000);
        assert_eq!(local_relevance(&record, "rust"), 2_000);
        assert_eq!(local_relevance(&record, "secret-body-token"), 0);
    }

    #[test]
    fn local_workspace_replaces_same_resource_without_dropping_public_signals() {
        let record = local_record();
        let remote = CatalogSearchResponse {
            items: vec![CatalogResource {
                resource_id: record.resource_id.clone(),
                kind: CatalogResourceKind::Skill,
                locator: record.locator.clone(),
                owner: record.owner.clone(),
                name: record.skill_name.clone(),
                description: "older remote metadata".into(),
                license: None,
                compatibility: None,
                topics: vec!["agents".into()],
                source: CatalogSource::Public,
                visibility: CatalogVisibility::Public,
                generation: 1,
                version: Some(1),
                star_count: 9,
                viewer_starred: true,
                deprecated: true,
                fork_upstream_locator: None,
                pack_members: Vec::new(),
                relevance_rank: 500,
            }],
            next_cursor: Some("remote-next".into()),
        };
        let merged = merge_local_results(remote, vec![record], "rust", SearchSort::Relevance, 20);
        assert_eq!(merged.items.len(), 1);
        assert_eq!(merged.items[0].source, CatalogSource::Local);
        assert_eq!(merged.items[0].description, "Reviews Rust agent code");
        assert_eq!(merged.items[0].star_count, 9);
        assert_eq!(merged.items[0].topics, vec!["agents"]);
        assert!(merged.items[0].deprecated);
        assert_eq!(merged.next_cursor.as_deref(), Some("remote-next"));
    }
}
