-- Runtime RLS is intentionally ENABLEd rather than FORCEd. The migration/table owner is not a
-- runtime credential and is used by the SECURITY DEFINER policy helpers above to inspect the
-- protected base rows without recursive RLS evaluation. Both runtime pools authenticate directly
-- as non-owner, non-superuser, non-BYPASSRLS roles and cannot SET ROLE to the owner or each other.

-- Stable resource metadata is the root tenant boundary. Anonymous request SQL can see only live
-- public resources; authenticated actor transactions additionally see their own/team/shared
-- resources. Operator transactions can inspect quarantined/deleted rows, while the worker gets
-- the explicit all-row authority required for background reconciliation.
ALTER TABLE resources ENABLE ROW LEVEL SECURITY;
CREATE POLICY resources_read ON resources
    FOR SELECT TO denju_app
    USING (
        denju_actor_can_read_resource(id)
        OR denju_active_operator_id() IS NOT NULL
    );
CREATE POLICY resources_insert ON resources
    FOR INSERT TO denju_app
    WITH CHECK (denju_actor_can_publish_namespace(owner_namespace_id));
CREATE POLICY resources_update ON resources
    FOR UPDATE TO denju_app
    USING (
        denju_actor_can_manage_resource(id)
        OR denju_active_operator_id() IS NOT NULL
    )
    WITH CHECK (
        denju_actor_can_publish_namespace(owner_namespace_id)
        OR denju_active_operator_id() IS NOT NULL
    );
CREATE POLICY resources_delete ON resources
    FOR DELETE TO denju_app
    USING (
        denju_actor_can_manage_resource(id)
        OR denju_active_operator_id() IS NOT NULL
    );
CREATE POLICY resources_worker ON resources
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE resource_redirects ENABLE ROW LEVEL SECURITY;
CREATE POLICY resource_redirects_read ON resource_redirects
    FOR SELECT TO denju_app
    USING (denju_actor_can_read_resource(target_resource_id));
CREATE POLICY resource_redirects_write ON resource_redirects
    FOR ALL TO denju_app
    USING (denju_actor_can_manage_resource(target_resource_id))
    WITH CHECK (denju_actor_can_manage_resource(target_resource_id));
CREATE POLICY resource_redirects_worker ON resource_redirects
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

-- Search rows contain derived descriptions and relationship metadata for private resources too;
-- direct SQL therefore receives the same stable-resource visibility rather than table-wide read.
ALTER TABLE resource_search_documents ENABLE ROW LEVEL SECURITY;
CREATE POLICY resource_search_documents_read ON resource_search_documents
    FOR SELECT TO denju_app
    USING (denju_actor_can_read_resource(resource_id));
CREATE POLICY resource_search_documents_write ON resource_search_documents
    FOR ALL TO denju_app
    USING (denju_actor_can_manage_resource(resource_id))
    WITH CHECK (denju_actor_can_manage_resource(resource_id));
CREATE POLICY resource_search_documents_worker ON resource_search_documents
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

-- Immutable releases contain manifest JSON and object-store locations. A release row is visible
-- only when that exact revision/release is visible; app writes are append-only and resource-owner
-- scoped. Historical release mutation remains a worker/migration concern.
REVOKE UPDATE, DELETE ON skill_releases FROM denju_app;
ALTER TABLE skill_releases ENABLE ROW LEVEL SECURITY;
CREATE POLICY skill_releases_read ON skill_releases
    FOR SELECT TO denju_app
    USING (
        denju_actor_can_read_revision_snapshot(resource_id, revision_id)
        OR denju_active_operator_id() IS NOT NULL
    );
CREATE POLICY skill_releases_insert ON skill_releases
    FOR INSERT TO denju_app
    WITH CHECK (denju_actor_can_manage_resource(resource_id));
CREATE POLICY skill_releases_worker ON skill_releases
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

-- The global Merkle/revision graph is content-addressed implementation state, not a tenant-wide
-- discovery surface. Tree rows are write-only to actor-scoped request flows; revisions and edges
-- are readable only when the actor can read a resource snapshot that reaches them (or authored
-- the still-unattached revision while constructing it).
REVOKE SELECT, UPDATE, DELETE ON merkle_trees, tree_entries FROM denju_app;
ALTER TABLE merkle_trees ENABLE ROW LEVEL SECURITY;
CREATE POLICY merkle_trees_insert ON merkle_trees
    FOR INSERT TO denju_app WITH CHECK (denju_actor_user_id() IS NOT NULL);
CREATE POLICY merkle_trees_worker ON merkle_trees
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE tree_entries ENABLE ROW LEVEL SECURITY;
CREATE POLICY tree_entries_insert ON tree_entries
    FOR INSERT TO denju_app WITH CHECK (denju_actor_user_id() IS NOT NULL);
CREATE POLICY tree_entries_worker ON tree_entries
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

REVOKE UPDATE, DELETE ON revisions FROM denju_app;
ALTER TABLE revisions ENABLE ROW LEVEL SECURITY;
CREATE POLICY revisions_read ON revisions
    FOR SELECT TO denju_app
    USING (denju_actor_can_read_revision(revision_id));
CREATE POLICY revisions_insert ON revisions
    FOR INSERT TO denju_app
    WITH CHECK (
        EXISTS(
            SELECT 1 FROM author_principal_users link
            WHERE link.author_principal_id = revisions.author_principal_id
              AND link.user_id = denju_actor_user_id()
        )
    );
CREATE POLICY revisions_worker ON revisions
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

REVOKE UPDATE, DELETE ON revision_parents FROM denju_app;
ALTER TABLE revision_parents ENABLE ROW LEVEL SECURITY;
CREATE POLICY revision_parents_read ON revision_parents
    FOR SELECT TO denju_app
    USING (denju_actor_can_read_revision(revision_id));
CREATE POLICY revision_parents_insert ON revision_parents
    FOR INSERT TO denju_app
    WITH CHECK (denju_actor_can_read_revision(revision_id));
CREATE POLICY revision_parents_worker ON revision_parents
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

REVOKE UPDATE ON revision_blob_reachability FROM denju_app;
ALTER TABLE revision_blob_reachability ENABLE ROW LEVEL SECURITY;
CREATE POLICY revision_blob_reachability_read ON revision_blob_reachability
    FOR SELECT TO denju_app
    USING (denju_actor_can_read_revision(revision_id));
CREATE POLICY revision_blob_reachability_insert ON revision_blob_reachability
    FOR INSERT TO denju_app
    WITH CHECK (denju_actor_can_read_revision(revision_id));
CREATE POLICY revision_blob_reachability_delete ON revision_blob_reachability
    FOR DELETE TO denju_app
    USING (
        denju_actor_can_read_revision(revision_id)
        OR (
            NOT EXISTS(
                SELECT 1 FROM resource_revision_snapshots snapshot
                WHERE snapshot.revision_id = revision_blob_reachability.revision_id
            )
            AND NOT EXISTS(
                SELECT 1 FROM skill_releases release
                WHERE release.revision_id = revision_blob_reachability.revision_id
            )
        )
    );
CREATE POLICY revision_blob_reachability_worker ON revision_blob_reachability
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE skill_forks ENABLE ROW LEVEL SECURITY;
CREATE POLICY skill_forks_read ON skill_forks
    FOR SELECT TO denju_app
    USING (denju_actor_can_read_resource(resource_id));
CREATE POLICY skill_forks_write ON skill_forks
    FOR ALL TO denju_app
    USING (denju_actor_can_manage_resource(resource_id))
    WITH CHECK (denju_actor_can_manage_resource(resource_id));
CREATE POLICY skill_forks_worker_read ON skill_forks
    FOR SELECT TO denju_worker USING (true);

ALTER TABLE skill_proposals ENABLE ROW LEVEL SECURITY;
CREATE POLICY skill_proposals_read ON skill_proposals
    FOR SELECT TO denju_app
    USING (
        proposer_user_id = denju_actor_user_id()
        OR denju_actor_can_manage_resource(target_resource_id)
    );
CREATE POLICY skill_proposals_insert ON skill_proposals
    FOR INSERT TO denju_app
    WITH CHECK (
        proposer_user_id = denju_actor_user_id()
        AND denju_actor_can_manage_resource(source_resource_id)
    );
CREATE POLICY skill_proposals_update ON skill_proposals
    FOR UPDATE TO denju_app
    USING (
        proposer_user_id = denju_actor_user_id()
        OR denju_actor_can_manage_resource(target_resource_id)
    )
    WITH CHECK (
        proposer_user_id = denju_actor_user_id()
        OR denju_actor_can_manage_resource(target_resource_id)
    );
REVOKE DELETE ON skill_proposals FROM denju_app;

ALTER TABLE skill_proposal_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY skill_proposal_operations_actor ON skill_proposal_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());

-- Pack membership and immutable pack history inherit the pack resource's visibility. This blocks
-- guessed private-pack IDs from becoming a side channel for member/revision enumeration.
ALTER TABLE pack_state ENABLE ROW LEVEL SECURITY;
CREATE POLICY pack_state_read ON pack_state
    FOR SELECT TO denju_app USING (denju_actor_can_read_resource(resource_id));
CREATE POLICY pack_state_write ON pack_state
    FOR ALL TO denju_app
    USING (denju_actor_can_manage_resource(resource_id))
    WITH CHECK (denju_actor_can_manage_resource(resource_id));
CREATE POLICY pack_state_worker ON pack_state
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE pack_members ENABLE ROW LEVEL SECURITY;
CREATE POLICY pack_members_read ON pack_members
    FOR SELECT TO denju_app USING (denju_actor_can_read_resource(pack_resource_id));
CREATE POLICY pack_members_write ON pack_members
    FOR ALL TO denju_app
    USING (denju_actor_can_manage_resource(pack_resource_id))
    WITH CHECK (denju_actor_can_manage_resource(pack_resource_id));
CREATE POLICY pack_members_worker ON pack_members
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE pack_revisions ENABLE ROW LEVEL SECURITY;
CREATE POLICY pack_revisions_read ON pack_revisions
    FOR SELECT TO denju_app USING (denju_actor_can_read_resource(pack_resource_id));
CREATE POLICY pack_revisions_write ON pack_revisions
    FOR ALL TO denju_app
    USING (denju_actor_can_manage_resource(pack_resource_id))
    WITH CHECK (denju_actor_can_manage_resource(pack_resource_id));
CREATE POLICY pack_revisions_worker ON pack_revisions
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE pack_revision_members ENABLE ROW LEVEL SECURITY;
CREATE POLICY pack_revision_members_read ON pack_revision_members
    FOR SELECT TO denju_app USING (denju_actor_can_read_resource(pack_resource_id));
CREATE POLICY pack_revision_members_write ON pack_revision_members
    FOR ALL TO denju_app
    USING (denju_actor_can_manage_resource(pack_resource_id))
    WITH CHECK (denju_actor_can_manage_resource(pack_resource_id));
CREATE POLICY pack_revision_members_worker ON pack_revision_members
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE pack_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY pack_operations_actor ON pack_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());
CREATE POLICY pack_operations_worker ON pack_operations
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

-- Team membership is visible to members of that team, while invite hashes are visible only to
-- their creators/team members or to the exact invite hash bound in the current join transaction.
-- Application authorization remains stricter (for example maintainer-vs-owner role changes).
ALTER TABLE team_memberships ENABLE ROW LEVEL SECURITY;
CREATE POLICY team_memberships_read ON team_memberships
    FOR SELECT TO denju_app
    USING (
        user_id = denju_actor_user_id()
        OR denju_actor_has_namespace_access(team_namespace_id)
    );
CREATE POLICY team_memberships_insert ON team_memberships
    FOR INSERT TO denju_app
    WITH CHECK (denju_actor_can_add_team_member(team_namespace_id, user_id));
CREATE POLICY team_memberships_update ON team_memberships
    FOR UPDATE TO denju_app
    USING (
        user_id = denju_actor_user_id()
        OR denju_actor_is_team_owner(team_namespace_id)
    )
    WITH CHECK (
        user_id = denju_actor_user_id()
        OR denju_actor_is_team_owner(team_namespace_id)
    );
CREATE POLICY team_memberships_delete ON team_memberships
    FOR DELETE TO denju_app
    USING (
        user_id = denju_actor_user_id()
        OR denju_actor_is_team_owner(team_namespace_id)
    );
CREATE POLICY team_memberships_worker_read ON team_memberships
    FOR SELECT TO denju_worker USING (true);

REVOKE DELETE ON team_invites FROM denju_app;
ALTER TABLE team_invites ENABLE ROW LEVEL SECURITY;
CREATE POLICY team_invites_read ON team_invites
    FOR SELECT TO denju_app
    USING (
        created_by_user_id = denju_actor_user_id()
        OR denju_actor_has_namespace_access(team_namespace_id)
        OR id = denju_bound_team_invite_id()
    );
CREATE POLICY team_invites_insert ON team_invites
    FOR INSERT TO denju_app
    WITH CHECK (
        created_by_user_id = denju_actor_user_id()
        AND denju_actor_has_namespace_access(team_namespace_id)
    );
CREATE POLICY team_invites_update ON team_invites
    FOR UPDATE TO denju_app
    USING (
        created_by_user_id = denju_actor_user_id()
        OR denju_actor_is_team_owner(team_namespace_id)
        OR id = denju_bound_team_invite_id()
    )
    WITH CHECK (
        created_by_user_id = denju_actor_user_id()
        OR denju_actor_is_team_owner(team_namespace_id)
        OR (
            id = denju_bound_team_invite_id()
            AND (used_by_user_id IS NULL OR used_by_user_id = denju_actor_user_id())
        )
    );

ALTER TABLE team_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY team_operations_actor ON team_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());

ALTER TABLE resource_transfer_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY resource_transfer_operations_actor ON resource_transfer_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());

-- Reports are write-only to ordinary users. Operator authentication binds the same transaction
-- before report inspection, so raw report reasons cannot be enumerated with the app credential.
REVOKE UPDATE, DELETE ON resource_reports FROM denju_app;
ALTER TABLE resource_reports ENABLE ROW LEVEL SECURITY;
CREATE POLICY resource_reports_insert ON resource_reports
    FOR INSERT TO denju_app
    WITH CHECK (reporter_user_id = denju_actor_user_id());
CREATE POLICY resource_reports_operator_read ON resource_reports
    FOR SELECT TO denju_app
    USING (denju_active_operator_id() IS NOT NULL);
CREATE POLICY resource_reports_reporter_update ON resource_reports
    FOR UPDATE TO denju_app
    USING (reporter_user_id = denju_actor_user_id())
    WITH CHECK (reporter_user_id IS NULL OR reporter_user_id = denju_actor_user_id());
GRANT UPDATE (reporter_user_id) ON resource_reports TO denju_app;

-- Individual star activity is private by default. The public aggregate lives on resources and is
-- refreshed only through denju_refresh_resource_star_count; raw star rows remain actor-owned.
ALTER TABLE resource_stars ENABLE ROW LEVEL SECURITY;
CREATE POLICY resource_stars_actor ON resource_stars
    FOR SELECT TO denju_app
    USING (user_id = denju_actor_user_id());
CREATE POLICY resource_stars_insert ON resource_stars
    FOR INSERT TO denju_app
    WITH CHECK (
        user_id = denju_actor_user_id()
        AND EXISTS(
            SELECT 1 FROM resources resource
            WHERE resource.id = resource_stars.resource_id
              AND resource.visibility = 'public'
              AND resource.deleted_at IS NULL
        )
    );
CREATE POLICY resource_stars_delete ON resource_stars
    FOR DELETE TO denju_app
    USING (user_id = denju_actor_user_id());
REVOKE UPDATE ON resource_stars FROM denju_app;

-- A follow edge is visible when the viewer owns either side or either public profile surface
-- explicitly exposes that edge. If both users hide the corresponding lists, direct request SQL
-- cannot use the base table to recover the hidden relation/count.
ALTER TABLE user_follows ENABLE ROW LEVEL SECURITY;
CREATE POLICY user_follows_read ON user_follows
    FOR SELECT TO denju_app
    USING (
        follower_user_id = denju_actor_user_id()
        OR followed_user_id = denju_actor_user_id()
        OR EXISTS(
            SELECT 1 FROM users follower
            WHERE follower.id = user_follows.follower_user_id
              AND follower.following_visible
        )
        OR EXISTS(
            SELECT 1 FROM users followed
            WHERE followed.id = user_follows.followed_user_id
              AND followed.followers_visible
        )
    );
CREATE POLICY user_follows_insert ON user_follows
    FOR INSERT TO denju_app
    WITH CHECK (follower_user_id = denju_actor_user_id());
CREATE POLICY user_follows_delete ON user_follows
    FOR DELETE TO denju_app
    USING (
        follower_user_id = denju_actor_user_id()
        OR followed_user_id = denju_actor_user_id()
    );
REVOKE UPDATE ON user_follows FROM denju_app;

ALTER TABLE social_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY social_operations_actor ON social_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());

-- Subscription desired state and its idempotency records are private actor state. Installation
-- credentials and user sessions get distinct transaction-local actor IDs, so the shared request
-- role cannot enumerate another installation/account's watch set or operation hashes.
ALTER TABLE installation_subscriptions ENABLE ROW LEVEL SECURITY;
CREATE POLICY installation_subscriptions_actor ON installation_subscriptions
    FOR ALL TO denju_app
    USING (
        installation_id = denju_actor_installation_id()
        OR EXISTS(
            SELECT 1 FROM installations installation
            WHERE installation.id = installation_subscriptions.installation_id
              AND installation.user_id = denju_actor_user_id()
        )
    )
    WITH CHECK (
        installation_id = denju_actor_installation_id()
        OR EXISTS(
            SELECT 1 FROM installations installation
            WHERE installation.id = installation_subscriptions.installation_id
              AND installation.user_id = denju_actor_user_id()
        )
    );
CREATE POLICY installation_subscriptions_worker ON installation_subscriptions
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE subscription_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY subscription_operations_actor ON subscription_operations
    FOR ALL TO denju_app
    USING (installation_id = denju_actor_installation_id())
    WITH CHECK (installation_id = denju_actor_installation_id());

ALTER TABLE account_subscriptions ENABLE ROW LEVEL SECURITY;
CREATE POLICY account_subscriptions_actor ON account_subscriptions
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());
CREATE POLICY account_subscriptions_worker ON account_subscriptions
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE account_subscription_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY account_subscription_operations_actor ON account_subscription_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());

-- Bearer verifier columns are not readable by the request role. Exact credential authentication
-- goes through the SECURITY DEFINER functions above; after authentication, ordinary metadata and
-- mutation access is restricted to the installation/user bound to the transaction.
REVOKE SELECT ON installations FROM denju_app;
GRANT SELECT (id, author_principal_id, created_at, user_id, revoked_at) ON installations TO denju_app;
ALTER TABLE installations ENABLE ROW LEVEL SECURITY;
CREATE POLICY installations_read ON installations
    FOR SELECT TO denju_app
    USING (
        id = denju_actor_installation_id()
        OR user_id = denju_actor_user_id()
    );
CREATE POLICY installations_insert ON installations
    FOR INSERT TO denju_app
    WITH CHECK (user_id IS NULL AND revoked_at IS NULL);
CREATE POLICY installations_update ON installations
    FOR UPDATE TO denju_app
    USING (
        id = denju_actor_installation_id()
        OR user_id = denju_actor_user_id()
    )
    WITH CHECK (
        id = denju_actor_installation_id()
        OR user_id = denju_actor_user_id()
    );
REVOKE DELETE ON installations FROM denju_app;

REVOKE SELECT ON users FROM denju_app;
GRANT SELECT (
    id,
    namespace_id,
    author_principal_id,
    created_at,
    deleted_at,
    bio,
    followers_visible,
    following_visible
) ON users TO denju_app;
ALTER TABLE users ENABLE ROW LEVEL SECURITY;
CREATE POLICY users_read ON users
    FOR SELECT TO denju_app USING (true);
CREATE POLICY users_insert ON users
    FOR INSERT TO denju_app
    WITH CHECK (
        namespace_id IS NOT NULL
        AND password_hash IS NOT NULL
        AND recovery_secret_hash IS NOT NULL
        AND deleted_at IS NULL
    );
CREATE POLICY users_update ON users
    FOR UPDATE TO denju_app
    USING (id = denju_actor_user_id())
    WITH CHECK (
        id = denju_actor_user_id()
        AND (
            (deleted_at IS NULL AND namespace_id IS NOT NULL)
            OR (deleted_at IS NOT NULL AND namespace_id IS NULL)
        )
    );
REVOKE DELETE ON users FROM denju_app;

REVOKE SELECT ON sessions FROM denju_app;
GRANT SELECT (id, user_id, installation_id, device_name, created_at, revoked_at) ON sessions TO denju_app;
ALTER TABLE sessions ENABLE ROW LEVEL SECURITY;
CREATE POLICY sessions_actor ON sessions
    FOR SELECT TO denju_app
    USING (user_id = denju_actor_user_id());
CREATE POLICY sessions_insert ON sessions
    FOR INSERT TO denju_app
    WITH CHECK (user_id = denju_actor_user_id());
CREATE POLICY sessions_update ON sessions
    FOR UPDATE TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());
REVOKE DELETE ON sessions FROM denju_app;

REVOKE SELECT ON automation_tokens FROM denju_app;
GRANT SELECT (id, user_id, scopes, expires_at, created_at, revoked_at) ON automation_tokens TO denju_app;
ALTER TABLE automation_tokens ENABLE ROW LEVEL SECURITY;
CREATE POLICY automation_tokens_actor ON automation_tokens
    FOR SELECT TO denju_app
    USING (user_id = denju_actor_user_id());
CREATE POLICY automation_tokens_insert ON automation_tokens
    FOR INSERT TO denju_app
    WITH CHECK (user_id = denju_actor_user_id());
CREATE POLICY automation_tokens_update ON automation_tokens
    FOR UPDATE TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());
REVOKE DELETE ON automation_tokens FROM denju_app;

ALTER TABLE identity_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY identity_operations_actor ON identity_operations
    FOR ALL TO denju_app
    USING (
        (actor_kind = 'user' AND actor_id = denju_actor_user_id())
        OR (
            actor_kind = 'installation'
            AND actor_id = denju_actor_installation_id()
        )
    )
    WITH CHECK (
        (actor_kind = 'user' AND actor_id = denju_actor_user_id())
        OR (
            actor_kind = 'installation'
            AND actor_id = denju_actor_installation_id()
        )
    );

ALTER TABLE skill_lifecycle_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY skill_lifecycle_operations_actor ON skill_lifecycle_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());
CREATE POLICY skill_lifecycle_operations_worker ON skill_lifecycle_operations
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE skill_release_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY skill_release_operations_actor ON skill_release_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());

ALTER TABLE skill_restore_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY skill_restore_operations_actor ON skill_restore_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());

ALTER TABLE skill_release_tags ENABLE ROW LEVEL SECURITY;
CREATE POLICY skill_release_tags_read ON skill_release_tags
    FOR SELECT TO denju_app
    USING (
        EXISTS(
            SELECT 1 FROM skill_releases release
            WHERE release.resource_id = skill_release_tags.resource_id
              AND release.version = skill_release_tags.version
              AND denju_actor_can_read_revision_snapshot(release.resource_id, release.revision_id)
        )
    );
CREATE POLICY skill_release_tags_insert ON skill_release_tags
    FOR INSERT TO denju_app
    WITH CHECK (denju_actor_can_manage_resource(resource_id));
REVOKE UPDATE, DELETE ON skill_release_tags FROM denju_app;
CREATE POLICY skill_release_tags_worker ON skill_release_tags
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

-- Durable event/outbox/GC tables are internal coordination state and frequently contain private
-- resource IDs or object hashes. Request transactions may append the exact records their typed
-- mutation emits, but cannot use these tables as a side channel to enumerate cross-tenant state.
REVOKE SELECT, UPDATE, DELETE ON authority_events FROM denju_app;
GRANT SELECT (id) ON authority_events TO denju_app;
REVOKE SELECT, UPDATE, DELETE ON outbox_events FROM denju_app;
REVOKE ALL ON pack_release_event_completions FROM denju_app;
REVOKE SELECT, DELETE ON canonical_blob_gc FROM denju_app;

-- Admin mutations are request-role SQL only after an operator bearer has been authenticated in
-- the same transaction. Ordinary application traffic may read quarantine state so it can enforce
-- the security decision, but it cannot create/lift quarantine rows or rewrite audit/idempotency
-- history merely because it holds the general request database credential.
REVOKE UPDATE, DELETE ON admin_operations FROM denju_app;
REVOKE SELECT, UPDATE, DELETE ON operator_audit_log FROM denju_app;
REVOKE DELETE ON resource_quarantines FROM denju_app;

ALTER TABLE admin_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY admin_operations_read ON admin_operations
    FOR SELECT TO denju_app
    USING (operator_id = denju_active_operator_id());
CREATE POLICY admin_operations_insert ON admin_operations
    FOR INSERT TO denju_app
    WITH CHECK (operator_id = denju_active_operator_id());

ALTER TABLE operator_audit_log ENABLE ROW LEVEL SECURITY;
CREATE POLICY operator_audit_log_insert ON operator_audit_log
    FOR INSERT TO denju_app
    WITH CHECK (operator_id = denju_active_operator_id());

ALTER TABLE resource_quarantines ENABLE ROW LEVEL SECURITY;
CREATE POLICY resource_quarantines_read ON resource_quarantines
    FOR SELECT TO denju_app USING (true);
CREATE POLICY resource_quarantines_insert ON resource_quarantines
    FOR INSERT TO denju_app
    WITH CHECK (created_by_operator_id = denju_active_operator_id());
CREATE POLICY resource_quarantines_update ON resource_quarantines
    FOR UPDATE TO denju_app
    USING (denju_active_operator_id() IS NOT NULL)
    WITH CHECK (
        created_by_operator_id IS NOT NULL
        AND (
            lifted_by_operator_id IS NULL
            OR lifted_by_operator_id = denju_active_operator_id()
        )
    );
CREATE POLICY resource_quarantines_worker_read ON resource_quarantines
    FOR SELECT TO denju_worker USING (true);

-- Private workspaces are row-owner scoped. Personal share recipients and proposal reviewers get
-- only the explicitly designed live-workspace read capability; team membership by itself never
-- reveals another maintainer's unpublished workspace.
ALTER TABLE skill_private_workspaces ENABLE ROW LEVEL SECURITY;
CREATE POLICY skill_private_workspaces_read ON skill_private_workspaces
    FOR SELECT TO denju_app
    USING (denju_actor_can_read_workspace(resource_id, workspace_user_id));
CREATE POLICY skill_private_workspaces_write ON skill_private_workspaces
    FOR ALL TO denju_app
    USING (
        workspace_user_id = denju_actor_user_id()
        AND denju_actor_can_manage_resource(resource_id)
    )
    WITH CHECK (
        workspace_user_id = denju_actor_user_id()
        AND denju_actor_can_manage_resource(resource_id)
    );
CREATE POLICY skill_private_workspaces_worker ON skill_private_workspaces
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE skill_workspace_conflicts ENABLE ROW LEVEL SECURITY;
CREATE POLICY skill_workspace_conflicts_read ON skill_workspace_conflicts
    FOR SELECT TO denju_app
    USING (workspace_user_id = denju_actor_user_id());
CREATE POLICY skill_workspace_conflicts_write ON skill_workspace_conflicts
    FOR ALL TO denju_app
    USING (
        workspace_user_id = denju_actor_user_id()
        AND denju_actor_can_manage_resource(resource_id)
    )
    WITH CHECK (
        workspace_user_id = denju_actor_user_id()
        AND denju_actor_can_manage_resource(resource_id)
    );
CREATE POLICY skill_workspace_conflicts_worker ON skill_workspace_conflicts
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE private_skill_shares ENABLE ROW LEVEL SECURITY;
CREATE POLICY private_skill_shares_read ON private_skill_shares
    FOR SELECT TO denju_app
    USING (
        recipient_user_id = denju_actor_user_id()
        OR denju_actor_can_manage_resource(resource_id)
    );
CREATE POLICY private_skill_shares_insert ON private_skill_shares
    FOR INSERT TO denju_app
    WITH CHECK (denju_actor_can_manage_resource(resource_id));
CREATE POLICY private_skill_shares_update ON private_skill_shares
    FOR UPDATE TO denju_app
    USING (denju_actor_can_manage_resource(resource_id))
    WITH CHECK (denju_actor_can_manage_resource(resource_id));
CREATE POLICY private_skill_shares_delete ON private_skill_shares
    FOR DELETE TO denju_app
    USING (denju_actor_can_manage_resource(resource_id));

ALTER TABLE private_share_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY private_share_operations_actor ON private_share_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());

ALTER TABLE private_import_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY private_import_operations_actor ON private_import_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (
        user_id = denju_actor_user_id()
        AND denju_actor_can_publish_namespace(namespace_id)
    );

ALTER TABLE private_revision_operations ENABLE ROW LEVEL SECURITY;
CREATE POLICY private_revision_operations_actor ON private_revision_operations
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (
        user_id = denju_actor_user_id()
        AND denju_actor_can_publish_namespace(namespace_id)
    );
CREATE POLICY private_revision_operations_worker ON private_revision_operations
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE private_import_staging ENABLE ROW LEVEL SECURITY;
CREATE POLICY private_import_staging_actor ON private_import_staging
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());

ALTER TABLE private_revision_staging ENABLE ROW LEVEL SECURITY;
CREATE POLICY private_revision_staging_actor ON private_revision_staging
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());

ALTER TABLE private_revision_operation_parents ENABLE ROW LEVEL SECURITY;
CREATE POLICY private_revision_operation_parents_actor ON private_revision_operation_parents
    FOR ALL TO denju_app
    USING (user_id = denju_actor_user_id())
    WITH CHECK (user_id = denju_actor_user_id());

ALTER TABLE team_pack_assignments ENABLE ROW LEVEL SECURITY;
CREATE POLICY team_pack_assignments_read ON team_pack_assignments
    FOR SELECT TO denju_app
    USING (denju_actor_has_namespace_access(team_namespace_id));
CREATE POLICY team_pack_assignments_write ON team_pack_assignments
    FOR ALL TO denju_app
    USING (
        EXISTS(
            SELECT 1 FROM team_memberships membership
            WHERE membership.team_namespace_id = team_pack_assignments.team_namespace_id
              AND membership.user_id = denju_actor_user_id()
              AND membership.role = 'owner'
        )
    )
    WITH CHECK (
        EXISTS(
            SELECT 1 FROM team_memberships membership
            WHERE membership.team_namespace_id = team_pack_assignments.team_namespace_id
              AND membership.user_id = denju_actor_user_id()
              AND membership.role = 'owner'
        )
    );

ALTER TABLE team_owner_transfers ENABLE ROW LEVEL SECURITY;
CREATE POLICY team_owner_transfers_actor ON team_owner_transfers
    FOR ALL TO denju_app
    USING (
        from_user_id = denju_actor_user_id()
        OR to_user_id = denju_actor_user_id()
        OR denju_actor_has_namespace_access(team_namespace_id)
    )
    WITH CHECK (denju_actor_has_namespace_access(team_namespace_id));

ALTER TABLE resource_revision_snapshots ENABLE ROW LEVEL SECURITY;
CREATE POLICY resource_revision_snapshots_read ON resource_revision_snapshots
    FOR SELECT TO denju_app
    USING (denju_actor_can_read_revision_snapshot(resource_id, revision_id));
CREATE POLICY resource_revision_snapshots_write ON resource_revision_snapshots
    FOR ALL TO denju_app
    USING (denju_actor_can_manage_resource(resource_id))
    WITH CHECK (denju_actor_can_manage_resource(resource_id));
CREATE POLICY resource_revision_snapshots_worker ON resource_revision_snapshots
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE resource_blob_reachability ENABLE ROW LEVEL SECURITY;
CREATE POLICY resource_blob_reachability_read ON resource_blob_reachability
    FOR SELECT TO denju_app
    USING (denju_actor_can_manage_resource(resource_id));
CREATE POLICY resource_blob_reachability_write ON resource_blob_reachability
    FOR ALL TO denju_app
    USING (denju_actor_can_manage_resource(resource_id))
    WITH CHECK (denju_actor_can_manage_resource(resource_id));
CREATE POLICY resource_blob_reachability_worker ON resource_blob_reachability
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE namespace_blob_reachability ENABLE ROW LEVEL SECURITY;
CREATE POLICY namespace_blob_reachability_read ON namespace_blob_reachability
    FOR SELECT TO denju_app
    USING (denju_actor_has_namespace_access(namespace_id));
CREATE POLICY namespace_blob_reachability_write ON namespace_blob_reachability
    FOR ALL TO denju_app
    USING (denju_actor_can_publish_namespace(namespace_id))
    WITH CHECK (denju_actor_can_publish_namespace(namespace_id));
CREATE POLICY namespace_blob_reachability_worker ON namespace_blob_reachability
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);

ALTER TABLE canonical_blobs ENABLE ROW LEVEL SECURITY;
CREATE POLICY canonical_blobs_read ON canonical_blobs
    FOR SELECT TO denju_app
    USING (
        EXISTS(
            SELECT 1 FROM namespace_blob_reachability reachability
            WHERE reachability.blob_id = canonical_blobs.blob_id
        )
    );
CREATE POLICY canonical_blobs_worker ON canonical_blobs
    FOR ALL TO denju_worker USING (true) WITH CHECK (true);
