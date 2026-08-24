#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApiMethod {
    Get,
    Post,
}

impl ApiMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiAuth {
    Public,
    OptionalBearer,
    Bearer,
    Operator,
    Recovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiRoute {
    pub method: ApiMethod,
    pub path: &'static str,
    pub auth: ApiAuth,
}

impl ApiRoute {
    pub const fn new(method: ApiMethod, path: &'static str, auth: ApiAuth) -> Self {
        Self { method, path, auth }
    }
}

/// Complete versioned HTTP route/auth catalog for the Denju registry API.
///
/// Exact JSON request/response DTO shapes remain owned by the other `denju-wire`
/// modules. `xtask` generates the checked OpenAPI inspection artifact from this
/// catalog and rejects drift between this list and the ordinary Axum router.
pub const OPENAPI_V1_ROUTES: &[ApiRoute] = &[
    ApiRoute::new(ApiMethod::Post, "/v1/account/delete", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/admin/quarantine", ApiAuth::Operator),
    ApiRoute::new(ApiMethod::Get, "/v1/admin/reports", ApiAuth::Operator),
    ApiRoute::new(
        ApiMethod::Get,
        "/v1/admin/resources/resolve",
        ApiAuth::Operator,
    ),
    ApiRoute::new(ApiMethod::Post, "/v1/admin/unquarantine", ApiAuth::Operator),
    ApiRoute::new(ApiMethod::Get, "/v1/capabilities", ApiAuth::Public),
    ApiRoute::new(ApiMethod::Get, "/v1/devices", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/devices/revoke", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/events", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/follows", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/follows/remove", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/identity", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/identity/backup", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/identity/claim", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/identity/login", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/identity/recover", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/installations", ApiAuth::Public),
    ApiRoute::new(
        ApiMethod::Post,
        "/v1/internal/outbox/drain",
        ApiAuth::Recovery,
    ),
    ApiRoute::new(
        ApiMethod::Post,
        "/v1/internal/packs/drain",
        ApiAuth::Recovery,
    ),
    ApiRoute::new(ApiMethod::Get, "/v1/internal/recover", ApiAuth::Recovery),
    ApiRoute::new(ApiMethod::Get, "/v1/pack-subscriptions", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/pack-subscriptions", ApiAuth::Bearer),
    ApiRoute::new(
        ApiMethod::Post,
        "/v1/pack-subscriptions/remove",
        ApiAuth::Bearer,
    ),
    ApiRoute::new(ApiMethod::Get, "/v1/packs", ApiAuth::OptionalBearer),
    ApiRoute::new(ApiMethod::Post, "/v1/packs", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/packs/add", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/packs/delete", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/packs/publish", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/packs/remove", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/packs/rename", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/packs/unpublish", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/private-skills", ApiAuth::Bearer),
    ApiRoute::new(
        ApiMethod::Post,
        "/v1/private-skills/imports/commit",
        ApiAuth::Bearer,
    ),
    ApiRoute::new(
        ApiMethod::Post,
        "/v1/private-skills/imports/prepare",
        ApiAuth::Bearer,
    ),
    ApiRoute::new(
        ApiMethod::Post,
        "/v1/private-skills/revisions/commit",
        ApiAuth::Bearer,
    ),
    ApiRoute::new(
        ApiMethod::Post,
        "/v1/private-skills/revisions/prepare",
        ApiAuth::Bearer,
    ),
    ApiRoute::new(ApiMethod::Post, "/v1/profile", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/proposals", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/proposals", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/proposals/accept", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/proposals/reject", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/proposals/show", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/proposals/withdraw", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/reports", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/resources/topics", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/resources/transfer", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/search", ApiAuth::OptionalBearer),
    ApiRoute::new(ApiMethod::Post, "/v1/shares", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/shares/remove", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/show", ApiAuth::OptionalBearer),
    ApiRoute::new(ApiMethod::Post, "/v1/skills/delete", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/skills/deprecate", ApiAuth::Bearer),
    ApiRoute::new(
        ApiMethod::Get,
        "/v1/skills/history",
        ApiAuth::OptionalBearer,
    ),
    ApiRoute::new(ApiMethod::Post, "/v1/skills/history/prune", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/skills/publish", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/skills/rename", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/skills/restore", ApiAuth::Bearer),
    ApiRoute::new(
        ApiMethod::Get,
        "/v1/skills/revision",
        ApiAuth::OptionalBearer,
    ),
    ApiRoute::new(ApiMethod::Get, "/v1/skills/show", ApiAuth::OptionalBearer),
    ApiRoute::new(ApiMethod::Post, "/v1/skills/unpublish", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/stars", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/stars/remove", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/subscriptions", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/subscriptions", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/subscriptions/remove", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/subscriptions/resolve", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/sync/reconcile", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/teams", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/teams", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/teams/delete", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/teams/invites", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/teams/invites/revoke", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/teams/join", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/teams/leave", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/teams/members/remove", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/teams/members/role", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/teams/owner-transfer", ApiAuth::Bearer),
    ApiRoute::new(
        ApiMethod::Post,
        "/v1/teams/owner-transfer/accept",
        ApiAuth::Bearer,
    ),
    ApiRoute::new(ApiMethod::Post, "/v1/teams/packs/assign", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/teams/packs/unassign", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/teams/settings", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/teams/show", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/tokens", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/tokens", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Post, "/v1/tokens/revoke", ApiAuth::Bearer),
    ApiRoute::new(ApiMethod::Get, "/v1/top", ApiAuth::OptionalBearer),
    ApiRoute::new(ApiMethod::Get, "/v1/usage", ApiAuth::Bearer),
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn route_catalog_is_unique_and_versioned() {
        let mut seen = BTreeSet::new();
        for route in OPENAPI_V1_ROUTES {
            assert!(route.path.starts_with("/v1/"));
            assert!(seen.insert((route.method, route.path)));
        }
        assert_eq!(seen.len(), 87);
    }
}
