use denju_wire::{
    ApiError, ApiErrorCode, CatalogResource, CatalogResourceKind, CatalogSearchQuery,
    CatalogSearchResponse, CatalogSource, CatalogTopQuery, CatalogVisibility, ProfileConnections,
    ProfileUserRef, SearchSort, UniversalShowResponse, UserProfile,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Registry, internal_api_error, lifecycle::generation_u64};

const SEARCH_PAGE_MAX: u32 = 50;
const PROFILE_CONNECTION_PAGE: i64 = 20;
const PROFILE_RESOURCE_LIMIT: i64 = 100;

impl Registry {
    pub async fn search_catalog(
        &self,
        bearer: Option<&str>,
        query: &CatalogSearchQuery,
    ) -> Result<CatalogSearchResponse, ApiError> {
        self.catalog_search(
            bearer,
            CatalogSearchInput {
                query: &query.q,
                limit: query.limit.unwrap_or(20),
                cursor: query.cursor.as_deref(),
                sort: query.sort,
                following_only: query.following,
                topic: query.topic.as_deref(),
                public_skills_only: false,
            },
        )
        .await
    }

    pub async fn top_catalog(
        &self,
        bearer: Option<&str>,
        query: &CatalogTopQuery,
    ) -> Result<CatalogSearchResponse, ApiError> {
        self.catalog_search(
            bearer,
            CatalogSearchInput {
                query: "",
                limit: query.limit.unwrap_or(20),
                cursor: query.cursor.as_deref(),
                sort: SearchSort::Stars,
                following_only: false,
                topic: query.topic.as_deref(),
                public_skills_only: true,
            },
        )
        .await
    }

    pub async fn show_profile(
        &self,
        bearer: Option<&str>,
        username: &str,
        followers_cursor: Option<&str>,
        following_cursor: Option<&str>,
    ) -> Result<UserProfile, ApiError> {
        let slug = parse_profile_slug(username)?;
        let viewer = self.optional_read_authority(bearer).await?;
        let row = sqlx::query_as::<_, (Uuid, Option<String>, bool, bool)>(
            "SELECT u.id,u.bio,u.followers_visible,u.following_visible FROM users u \
             JOIN namespaces n ON n.id=u.namespace_id \
             WHERE n.slug=$1 AND n.kind='user' AND u.deleted_at IS NULL",
        )
        .bind(&slug)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal_api_error)?
        .ok_or_else(|| ApiError::new(ApiErrorCode::NotFound, "user not found"))?;
        let viewer_follows = if let Some(viewer) = viewer.as_ref() {
            let mut tx = self.begin_actor_tx(viewer.user_id).await?;
            let follows = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM user_follows WHERE follower_user_id=$1 AND followed_user_id=$2)",
            )
            .bind(viewer.user_id)
            .bind(row.0)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal_api_error)?;
            tx.commit().await.map_err(internal_api_error)?;
            follows
        } else {
            false
        };
        let followers = if row.2 {
            Some(
                self.profile_connections(row.0, ConnectionDirection::Followers, followers_cursor)
                    .await?,
            )
        } else {
            None
        };
        let following = if row.3 {
            Some(
                self.profile_connections(row.0, ConnectionDirection::Following, following_cursor)
                    .await?,
            )
        } else {
            None
        };
        let catalog = self
            .profile_public_catalog(viewer.as_ref().map(|viewer| viewer.user_id), &slug)
            .await?;
        let mut public_skills = Vec::new();
        let mut public_packs = Vec::new();
        let mut public_forks = Vec::new();
        for item in catalog {
            match item.kind {
                CatalogResourceKind::Pack => public_packs.push(item),
                CatalogResourceKind::Skill if item.fork_upstream_locator.is_some() => {
                    public_forks.push(item)
                }
                CatalogResourceKind::Skill => public_skills.push(item),
            }
        }
        Ok(UserProfile {
            user_id: row.0.to_string(),
            username: format!("@{slug}"),
            bio: row.1,
            followers_visible: row.2,
            following_visible: row.3,
            followers,
            following,
            viewer_follows,
            public_skills,
            public_packs,
            public_forks,
        })
    }

    pub async fn universal_show(
        &self,
        bearer: Option<&str>,
        locator: &str,
        followers_cursor: Option<&str>,
        following_cursor: Option<&str>,
    ) -> Result<UniversalShowResponse, ApiError> {
        let value = locator.trim();
        if value.starts_with('@') && !value.contains('/') {
            return self
                .show_profile(bearer, value, followers_cursor, following_cursor)
                .await
                .map(UniversalShowResponse::Profile);
        }
        if value.contains("/packs/") {
            return self
                .pack_detail(bearer, value)
                .await
                .map(UniversalShowResponse::Pack);
        }
        self.show_public_skill(bearer, value)
            .await
            .map(UniversalShowResponse::Skill)
    }

    async fn catalog_search(
        &self,
        bearer: Option<&str>,
        input: CatalogSearchInput<'_>,
    ) -> Result<CatalogSearchResponse, ApiError> {
        let viewer = self.optional_read_authority(bearer).await?;
        if input.following_only && viewer.is_none() {
            return Err(ApiError::new(
                ApiErrorCode::Unauthorized,
                "--following requires a claimed user identity",
            ));
        }
        let topic = input.topic.map(normalize_topic).transpose()?;
        let limit = input.limit.clamp(1, SEARCH_PAGE_MAX);
        let cursor = input
            .cursor
            .map(CatalogCursor::decode)
            .transpose()?
            .unwrap_or_else(|| CatalogCursor::start(input.sort));
        if cursor.sort != input.sort {
            return Err(ApiError::new(
                ApiErrorCode::InvalidRequest,
                "search cursor was created for a different sort order",
            ));
        }
        let query = input.query.trim();
        let cursor_resource_id = cursor.resource_uuid()?;
        // `catalog_query` interpolates only one of two hard-coded ORDER BY/keyset fragments.
        // Every user-controlled value remains a bind parameter, so this dynamic string is audited.
        let rows = if let Some(viewer) = viewer.as_ref() {
            let mut tx = self.begin_actor_tx(viewer.user_id).await?;
            let rows =
                sqlx::query_as::<_, CatalogRow>(sqlx::AssertSqlSafe(catalog_query(input.sort)))
                    .bind(query)
                    .bind(Some(viewer.user_id))
                    .bind(Some(viewer.namespace_id))
                    .bind(input.following_only)
                    .bind(topic.as_deref())
                    .bind(input.public_skills_only)
                    .bind(input.cursor.is_some())
                    .bind(i32::from(cursor.deprecated))
                    .bind(cursor.relevance_rank)
                    .bind(cursor.star_count)
                    .bind(&cursor.owner)
                    .bind(&cursor.name)
                    .bind(cursor_resource_id)
                    .bind(i64::from(limit) + 1)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(internal_api_error)?;
            tx.commit().await.map_err(internal_api_error)?;
            rows
        } else {
            sqlx::query_as::<_, CatalogRow>(sqlx::AssertSqlSafe(catalog_public_query(input.sort)))
                .bind(query)
                .bind(topic.as_deref())
                .bind(input.public_skills_only)
                .bind(input.cursor.is_some())
                .bind(i32::from(cursor.deprecated))
                .bind(cursor.relevance_rank)
                .bind(cursor.star_count)
                .bind(&cursor.owner)
                .bind(&cursor.name)
                .bind(cursor_resource_id)
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
                .map(|row| CatalogCursor::from_row(input.sort, row).encode())
        } else {
            None
        };
        let items = visible
            .into_iter()
            .map(CatalogRow::into_wire)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CatalogSearchResponse { items, next_cursor })
    }

    async fn profile_connections(
        &self,
        target_user_id: Uuid,
        direction: ConnectionDirection,
        cursor: Option<&str>,
    ) -> Result<ProfileConnections, ApiError> {
        let cursor = cursor
            .map(ProfileCursor::decode)
            .transpose()?
            .unwrap_or_else(ProfileCursor::start);
        let (count_sql, page_sql) = match direction {
            ConnectionDirection::Followers => (
                "SELECT COUNT(*) FROM user_follows WHERE followed_user_id=$1",
                "SELECT u.id,n.slug FROM user_follows f JOIN users u ON u.id=f.follower_user_id \
                 JOIN namespaces n ON n.id=u.namespace_id WHERE f.followed_user_id=$1 AND u.deleted_at IS NULL \
                 AND (NOT $2 OR n.slug>$3 OR (n.slug=$3 AND u.id>$4)) \
                 ORDER BY n.slug,u.id LIMIT $5",
            ),
            ConnectionDirection::Following => (
                "SELECT COUNT(*) FROM user_follows WHERE follower_user_id=$1",
                "SELECT u.id,n.slug FROM user_follows f JOIN users u ON u.id=f.followed_user_id \
                 JOIN namespaces n ON n.id=u.namespace_id WHERE f.follower_user_id=$1 AND u.deleted_at IS NULL \
                 AND (NOT $2 OR n.slug>$3 OR (n.slug=$3 AND u.id>$4)) \
                 ORDER BY n.slug,u.id LIMIT $5",
            ),
        };
        let count: i64 = sqlx::query_scalar(count_sql)
            .bind(target_user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(internal_api_error)?;
        let cursor_user_id = cursor.user_uuid()?;
        let rows = sqlx::query_as::<_, (Uuid, String)>(page_sql)
            .bind(target_user_id)
            .bind(cursor.active)
            .bind(&cursor.slug)
            .bind(cursor_user_id)
            .bind(PROFILE_CONNECTION_PAGE + 1)
            .fetch_all(&self.pool)
            .await
            .map_err(internal_api_error)?;
        let has_more = rows.len() > PROFILE_CONNECTION_PAGE as usize;
        let visible = rows
            .into_iter()
            .take(PROFILE_CONNECTION_PAGE as usize)
            .collect::<Vec<_>>();
        let next_cursor = if has_more {
            visible.last().map(|row| {
                ProfileCursor {
                    active: true,
                    slug: row.1.clone(),
                    user_id: row.0.to_string(),
                }
                .encode()
            })
        } else {
            None
        };
        Ok(ProfileConnections {
            count: generation_u64(count)?,
            users: visible
                .into_iter()
                .map(|(id, slug)| ProfileUserRef {
                    user_id: id.to_string(),
                    username: format!("@{slug}"),
                })
                .collect(),
            next_cursor,
        })
    }

    async fn profile_public_catalog(
        &self,
        viewer_user_id: Option<Uuid>,
        owner: &str,
    ) -> Result<Vec<CatalogResource>, ApiError> {
        let query = sqlx::query_as::<_, CatalogRow>(
            "SELECT sd.resource_id,sd.resource_kind,sd.owner_slug,sd.resource_slug,sd.description,sd.license,sd.compatibility, \
                    sd.topics,'public'::text AS source,'public'::text AS visibility,r.generation, \
                    CASE WHEN r.kind='skill' THEN r.latest_release_version ELSE pack.current_version END AS version, \
                    sd.star_count,star.user_id IS NOT NULL AS viewer_starred,r.deprecated_at IS NOT NULL AS deprecated, \
                    sd.fork_upstream_locator,sd.pack_membership_text,0::bigint AS relevance_rank \
             FROM resource_search_documents sd JOIN resources r ON r.id=sd.resource_id \
             LEFT JOIN pack_state pack ON pack.resource_id=r.id \
             LEFT JOIN resource_stars star ON star.resource_id=r.id AND star.user_id=$2 \
             WHERE sd.owner_slug=$1 AND r.visibility='public' AND r.deleted_at IS NULL \
               AND NOT EXISTS(SELECT 1 FROM resource_quarantines rq WHERE rq.resource_id=r.id AND rq.lifted_at IS NULL \
                 AND (rq.release_version IS NULL OR (r.kind='skill' AND rq.release_version=r.latest_release_version))) \
               AND ((r.kind='skill' AND r.latest_release_version IS NOT NULL) OR (r.kind='pack' AND pack.resource_id IS NOT NULL)) \
             ORDER BY sd.resource_kind,sd.resource_slug,sd.resource_id LIMIT $3",
        )
        .bind(owner)
        .bind(viewer_user_id)
        .bind(PROFILE_RESOURCE_LIMIT);
        let rows = if let Some(viewer_user_id) = viewer_user_id {
            let mut tx = self.begin_actor_tx(viewer_user_id).await?;
            let rows = query
                .fetch_all(&mut *tx)
                .await
                .map_err(internal_api_error)?;
            tx.commit().await.map_err(internal_api_error)?;
            rows
        } else {
            query
                .fetch_all(&self.pool)
                .await
                .map_err(internal_api_error)?
        };
        rows.into_iter().map(CatalogRow::into_wire).collect()
    }
}

struct CatalogSearchInput<'a> {
    query: &'a str,
    limit: u32,
    cursor: Option<&'a str>,
    sort: SearchSort,
    following_only: bool,
    topic: Option<&'a str>,
    public_skills_only: bool,
}

fn catalog_query(sort: SearchSort) -> String {
    let (cursor_order, order) = catalog_sort_fragments(sort, 9, 10, 11, 12, 13);
    let follow_boost = match sort {
        SearchSort::Relevance => {
            "CASE WHEN followed.followed_user_id IS NOT NULL THEN 250 ELSE 0 END"
        }
        SearchSort::Stars => "0",
    };
    format!(
        "WITH candidate AS (\
           SELECT sd.resource_id,sd.resource_kind,sd.owner_slug,sd.resource_slug,sd.description,sd.license,sd.compatibility,sd.topics, \
                  CASE WHEN r.visibility='public' THEN 'public' \
                       WHEN r.owner_namespace_id=$3 THEN 'owned' \
                       WHEN n.kind='team' AND tm.user_id IS NOT NULL THEN 'team' ELSE 'private_share' END AS source, \
                  CASE WHEN r.visibility='public' THEN 'public' WHEN n.kind='team' THEN 'team' ELSE 'private' END AS visibility, \
                  r.generation,CASE WHEN r.kind='skill' AND r.visibility='public' THEN r.latest_release_version \
                                    WHEN r.kind='pack' THEN pack.current_version \
                                    WHEN n.kind='team' AND r.kind='skill' AND team_w.resource_id IS NULL THEN r.latest_release_version ELSE NULL END AS version, \
                  CASE WHEN r.visibility='public' THEN sd.star_count ELSE 0 END AS star_count, \
                  (r.visibility='public' AND viewer_star.user_id IS NOT NULL) AS viewer_starred, \
                  (r.visibility='public' AND r.deprecated_at IS NOT NULL) AS deprecated,sd.fork_upstream_locator,sd.pack_membership_text, \
                  (CASE WHEN $1='' THEN 0 ELSE \
                      CASE WHEN lower('@' || sd.owner_slug || '/' || sd.resource_slug)=lower($1) THEN 12000 \
                           WHEN lower(sd.resource_slug)=lower($1) THEN 10000 \
                           WHEN lower(sd.resource_slug) LIKE lower($1) || '%' THEN 7000 ELSE 0 END + \
                      (ts_rank_cd(sd.search_vector,websearch_to_tsquery('simple',$1))*10000)::bigint + \
                      (similarity(sd.search_text,$1)*1000)::bigint END + \
                   {follow_boost})::bigint AS relevance_rank \
             FROM resource_search_documents sd JOIN resources r ON r.id=sd.resource_id JOIN namespaces n ON n.id=r.owner_namespace_id \
             LEFT JOIN users owner_user ON owner_user.namespace_id=r.owner_namespace_id AND n.kind='user' \
             LEFT JOIN skill_private_workspaces owner_w ON owner_w.resource_id=r.id AND owner_w.workspace_user_id=owner_user.id \
             LEFT JOIN skill_private_workspaces team_w ON team_w.resource_id=r.id AND team_w.workspace_user_id=$2 AND n.kind='team' \
             LEFT JOIN private_skill_shares private_share ON private_share.resource_id=r.id AND private_share.recipient_user_id=$2 \
             LEFT JOIN team_memberships tm ON tm.team_namespace_id=r.owner_namespace_id AND tm.user_id=$2 \
             LEFT JOIN pack_state pack ON pack.resource_id=r.id \
             LEFT JOIN resource_stars viewer_star ON viewer_star.resource_id=r.id AND viewer_star.user_id=$2 \
             LEFT JOIN user_follows followed ON followed.follower_user_id=$2 AND followed.followed_user_id=owner_user.id \
            WHERE r.deleted_at IS NULL \
              AND NOT EXISTS(SELECT 1 FROM resource_quarantines rq WHERE rq.resource_id=r.id AND rq.lifted_at IS NULL AND ( \
                   rq.release_version IS NULL OR (r.kind='skill' AND ( \
                     (r.visibility='public' AND rq.release_version=r.latest_release_version) OR \
                     (n.kind='team' AND team_w.resource_id IS NULL AND rq.release_version=r.latest_release_version) \
                   )))) \
              AND (NOT $6 OR (r.visibility='public' AND r.kind='skill')) \
              AND (NOT $4 OR (r.visibility='public' AND followed.followed_user_id IS NOT NULL)) \
              AND ($5::text IS NULL OR $5=ANY(sd.topics)) \
              AND ($1='' OR sd.search_vector @@ websearch_to_tsquery('simple',$1) OR sd.search_text % $1 OR position(lower($1) in lower(sd.search_text))>0) \
              AND ( \
                  (r.visibility='public' AND ((r.kind='skill' AND r.latest_release_version IS NOT NULL) OR (r.kind='pack' AND pack.resource_id IS NOT NULL))) OR \
                  ($2::uuid IS NOT NULL AND r.visibility<>'public' AND ( \
                     (n.kind='user' AND r.kind='skill' AND owner_w.resource_id IS NOT NULL AND (r.owner_namespace_id=$3 OR private_share.resource_id IS NOT NULL)) OR \
                     (n.kind='user' AND r.kind='pack' AND r.owner_namespace_id=$3 AND pack.resource_id IS NOT NULL) OR \
                     (n.kind='team' AND r.kind='pack' AND tm.user_id IS NOT NULL AND pack.resource_id IS NOT NULL) OR \
                     (n.kind='team' AND r.kind='skill' AND (team_w.resource_id IS NOT NULL OR ((tm.user_id IS NOT NULL OR private_share.resource_id IS NOT NULL) AND r.latest_release_version IS NOT NULL))) \
                  )) \
              ) \
         ) SELECT * FROM candidate c WHERE (NOT $7 OR c.deprecated::int>$8 OR \
              (c.deprecated::int=$8 AND ({cursor_order}))) \
         ORDER BY {order} LIMIT $14"
    )
}

fn catalog_public_query(sort: SearchSort) -> String {
    let (cursor_order, order) = catalog_sort_fragments(sort, 6, 7, 8, 9, 10);
    format!(
        "WITH candidate AS (\
           SELECT sd.resource_id,sd.resource_kind,sd.owner_slug,sd.resource_slug,sd.description,sd.license,sd.compatibility,sd.topics, \
                  'public'::text AS source,'public'::text AS visibility,r.generation, \
                  CASE WHEN r.kind='skill' THEN r.latest_release_version ELSE pack.current_version END AS version, \
                  sd.star_count,false AS viewer_starred,(r.deprecated_at IS NOT NULL) AS deprecated, \
                  sd.fork_upstream_locator,sd.pack_membership_text, \
                  (CASE WHEN $1='' THEN 0 ELSE \
                      CASE WHEN lower('@' || sd.owner_slug || '/' || sd.resource_slug)=lower($1) THEN 12000 \
                           WHEN lower(sd.resource_slug)=lower($1) THEN 10000 \
                           WHEN lower(sd.resource_slug) LIKE lower($1) || '%' THEN 7000 ELSE 0 END + \
                      (ts_rank_cd(sd.search_vector,websearch_to_tsquery('simple',$1))*10000)::bigint + \
                      (similarity(sd.search_text,$1)*1000)::bigint END)::bigint AS relevance_rank \
             FROM resource_search_documents sd JOIN resources r ON r.id=sd.resource_id \
             LEFT JOIN pack_state pack ON pack.resource_id=r.id \
            WHERE r.deleted_at IS NULL AND r.visibility='public' \
              AND ((r.kind='skill' AND r.latest_release_version IS NOT NULL) OR (r.kind='pack' AND pack.resource_id IS NOT NULL)) \
              AND NOT EXISTS(SELECT 1 FROM resource_quarantines rq WHERE rq.resource_id=r.id AND rq.lifted_at IS NULL AND ( \
                   rq.release_version IS NULL OR (r.kind='skill' AND rq.release_version=r.latest_release_version))) \
              AND (NOT $3 OR r.kind='skill') \
              AND ($2::text IS NULL OR $2=ANY(sd.topics)) \
              AND ($1='' OR sd.search_vector @@ websearch_to_tsquery('simple',$1) OR sd.search_text % $1 OR position(lower($1) in lower(sd.search_text))>0) \
         ) SELECT * FROM candidate c WHERE (NOT $4 OR c.deprecated::int>$5 OR \
              (c.deprecated::int=$5 AND ({cursor_order}))) \
         ORDER BY {order} LIMIT $11"
    )
}

fn catalog_sort_fragments(
    sort: SearchSort,
    relevance_parameter: u8,
    stars_parameter: u8,
    owner_parameter: u8,
    name_parameter: u8,
    resource_parameter: u8,
) -> (String, &'static str) {
    let cursor_order = match sort {
        SearchSort::Relevance => format!(
            "c.relevance_rank < ${relevance_parameter} OR (c.relevance_rank=${relevance_parameter} AND \
             (c.star_count < ${stars_parameter} OR (c.star_count=${stars_parameter} AND \
             (c.owner_slug>${owner_parameter} OR (c.owner_slug=${owner_parameter} AND \
             (c.resource_slug>${name_parameter} OR (c.resource_slug=${name_parameter} AND c.resource_id>${resource_parameter})))))))"
        ),
        SearchSort::Stars => format!(
            "c.star_count < ${stars_parameter} OR (c.star_count=${stars_parameter} AND \
             (c.relevance_rank < ${relevance_parameter} OR (c.relevance_rank=${relevance_parameter} AND \
             (c.owner_slug>${owner_parameter} OR (c.owner_slug=${owner_parameter} AND \
             (c.resource_slug>${name_parameter} OR (c.resource_slug=${name_parameter} AND c.resource_id>${resource_parameter})))))))"
        ),
    };
    let order = match sort {
        SearchSort::Relevance => {
            "c.deprecated,c.relevance_rank DESC,c.star_count DESC,c.owner_slug,c.resource_slug,c.resource_id"
        }
        SearchSort::Stars => {
            "c.deprecated,c.star_count DESC,c.relevance_rank DESC,c.owner_slug,c.resource_slug,c.resource_id"
        }
    };
    (cursor_order, order)
}

#[derive(sqlx::FromRow)]
struct CatalogRow {
    resource_id: Uuid,
    resource_kind: String,
    owner_slug: String,
    resource_slug: String,
    description: String,
    license: Option<String>,
    compatibility: Option<String>,
    topics: Vec<String>,
    source: String,
    visibility: String,
    generation: i64,
    version: Option<i64>,
    star_count: i64,
    viewer_starred: bool,
    deprecated: bool,
    fork_upstream_locator: Option<String>,
    pack_membership_text: String,
    relevance_rank: i64,
}

impl CatalogRow {
    fn into_wire(self) -> Result<CatalogResource, ApiError> {
        let kind = match self.resource_kind.as_str() {
            "skill" => CatalogResourceKind::Skill,
            "pack" => CatalogResourceKind::Pack,
            _ => return Err(stored_error("resource kind")),
        };
        let source = match self.source.as_str() {
            "public" => CatalogSource::Public,
            "owned" => CatalogSource::Owned,
            "private_share" => CatalogSource::PrivateShare,
            "team" => CatalogSource::Team,
            _ => return Err(stored_error("catalog source")),
        };
        let visibility = match self.visibility.as_str() {
            "public" => CatalogVisibility::Public,
            "private" => CatalogVisibility::Private,
            "team" => CatalogVisibility::Team,
            _ => return Err(stored_error("catalog visibility")),
        };
        Ok(CatalogResource {
            resource_id: self.resource_id.to_string(),
            kind,
            locator: match kind {
                CatalogResourceKind::Skill => {
                    format!("@{}/{}", self.owner_slug, self.resource_slug)
                }
                CatalogResourceKind::Pack => {
                    format!("@{}/packs/{}", self.owner_slug, self.resource_slug)
                }
            },
            owner: self.owner_slug,
            name: self.resource_slug,
            description: self.description,
            license: self.license,
            compatibility: self.compatibility,
            topics: self.topics,
            source,
            visibility,
            generation: generation_u64(self.generation)?,
            version: self.version.map(generation_u64).transpose()?,
            star_count: generation_u64(self.star_count)?,
            viewer_starred: self.viewer_starred,
            deprecated: self.deprecated,
            fork_upstream_locator: self.fork_upstream_locator,
            pack_members: self
                .pack_membership_text
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            relevance_rank: self.relevance_rank,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatalogCursor {
    sort: SearchSort,
    deprecated: bool,
    relevance_rank: i64,
    star_count: i64,
    owner: String,
    name: String,
    resource_id: String,
}

impl CatalogCursor {
    fn start(sort: SearchSort) -> Self {
        Self {
            sort,
            deprecated: false,
            relevance_rank: 0,
            star_count: 0,
            owner: String::new(),
            name: String::new(),
            resource_id: Uuid::nil().to_string(),
        }
    }

    fn from_row(sort: SearchSort, row: &CatalogRow) -> Self {
        Self {
            sort,
            deprecated: row.deprecated,
            relevance_rank: row.relevance_rank,
            star_count: row.star_count,
            owner: row.owner_slug.clone(),
            name: row.resource_slug.clone(),
            resource_id: row.resource_id.to_string(),
        }
    }

    fn encode(&self) -> String {
        hex::encode(serde_json::to_vec(self).expect("catalog cursor serialization is infallible"))
    }

    fn decode(value: &str) -> Result<Self, ApiError> {
        let bytes = hex::decode(value)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor"))?;
        let cursor: Self = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor"))?;
        cursor.resource_uuid()?;
        Ok(cursor)
    }

    fn resource_uuid(&self) -> Result<Uuid, ApiError> {
        Uuid::parse_str(&self.resource_id)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid search cursor"))
    }
}

#[derive(Clone, Copy)]
enum ConnectionDirection {
    Followers,
    Following,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfileCursor {
    active: bool,
    slug: String,
    user_id: String,
}

impl ProfileCursor {
    fn start() -> Self {
        Self {
            active: false,
            slug: String::new(),
            user_id: Uuid::nil().to_string(),
        }
    }

    fn encode(&self) -> String {
        hex::encode(serde_json::to_vec(self).expect("profile cursor serialization is infallible"))
    }

    fn decode(value: &str) -> Result<Self, ApiError> {
        let bytes = hex::decode(value)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid profile cursor"))?;
        let cursor: Self = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid profile cursor"))?;
        cursor.user_uuid()?;
        Ok(cursor)
    }

    fn user_uuid(&self) -> Result<Uuid, ApiError> {
        Uuid::parse_str(&self.user_id)
            .map_err(|_| ApiError::new(ApiErrorCode::InvalidRequest, "invalid profile cursor"))
    }
}

fn parse_profile_slug(value: &str) -> Result<String, ApiError> {
    let slug = value.strip_prefix('@').unwrap_or(value);
    if slug.is_empty()
        || slug.contains('/')
        || slug != slug.to_ascii_lowercase()
        || !slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "profile must be an immutable @username",
        ));
    }
    Ok(slug.to_owned())
}

fn normalize_topic(value: &str) -> Result<String, ApiError> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 32
        || value.starts_with('-')
        || value.ends_with('-')
        || value.contains("--")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ApiError::new(
            ApiErrorCode::InvalidRequest,
            "invalid discovery topic",
        ));
    }
    Ok(value)
}

fn stored_error(field: &str) -> ApiError {
    ApiError::new(ApiErrorCode::Internal, format!("stored {field} is invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_cursor_round_trip_binds_sort_and_position() {
        let cursor = CatalogCursor {
            sort: SearchSort::Stars,
            deprecated: true,
            relevance_rank: 51,
            star_count: 8,
            owner: "alice".into(),
            name: "review".into(),
            resource_id: Uuid::now_v7().to_string(),
        };
        let decoded = CatalogCursor::decode(&cursor.encode()).unwrap();
        assert_eq!(decoded.sort, cursor.sort);
        assert_eq!(decoded.resource_id, cursor.resource_id);
        assert_eq!(decoded.star_count, 8);
    }

    #[test]
    fn topic_validation_matches_registry_metadata_vocabulary() {
        assert_eq!(normalize_topic(" Rust-Agents ").unwrap(), "rust-agents");
        assert!(normalize_topic("rust--agents").is_err());
    }
}
