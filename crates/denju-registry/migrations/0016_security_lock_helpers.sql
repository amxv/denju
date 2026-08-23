-- Quarantine blocks content authority, not the minimum stable metadata an already-authorized
-- actor needs to remove a local projection and explain why it disappeared. Resource rows therefore
-- remain subject to their ordinary audience ACL above even while quarantined, while releases,
-- workspaces, snapshots, and object reachability stay quarantine-gated below. This helper exposes
-- only whether the current user already owns a private workspace; it deliberately returns no
-- manifest, revision, snapshot key, or object location.
CREATE FUNCTION denju_actor_has_own_workspace(target_resource_id UUID) RETURNS BOOLEAN
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT denju_actor_user_id() IS NOT NULL
       AND EXISTS(
           SELECT 1 FROM skill_private_workspaces workspace
           WHERE workspace.resource_id = target_resource_id
             AND workspace.workspace_user_id = denju_actor_user_id()
       )
$$;

-- Authored pack mutation must serialize/catch up semantic skill-release events that committed
-- before the edit. Those durable event rows can contain unrelated private resource IDs and
-- payloads, so the request role cannot read `authority_events` directly. Expose only the ordered
-- release events applicable to one pack the active actor can already manage.
CREATE FUNCTION denju_pack_pending_release_events(
    target_pack_id UUID,
    target_limit BIGINT
) RETURNS TABLE(
    event_id BIGINT,
    resource_id UUID,
    payload_json JSONB
)
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT event.id,event.resource_id,event.payload_json
    FROM public.authority_events event
    JOIN public.pack_members member ON member.skill_resource_id=event.resource_id
    WHERE session_user='denju_app'
      AND denju_actor_can_manage_resource(target_pack_id)
      AND member.pack_resource_id=target_pack_id
      AND member.pinned_release_version IS NULL
      AND event.event_kind='skill_release_published'
      AND member.follow_after_event_id < event.id
      AND NOT EXISTS(
          SELECT 1 FROM public.pack_revisions revision
          WHERE revision.pack_resource_id=target_pack_id
            AND revision.source_release_event_id=event.id
      )
    ORDER BY event.id
    LIMIT LEAST(GREATEST(target_limit,0),1024)
$$;
REVOKE ALL ON FUNCTION denju_pack_pending_release_events(UUID,BIGINT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_pack_pending_release_events(UUID,BIGINT) TO denju_app;

-- A subscription transaction needs to serialize against concurrent pack publication/rename,
-- but `SELECT .. FOR UPDATE` under the request role also evaluates UPDATE RLS and would therefore
-- require giving subscribers pack-write authority. Keep that authority separated: this function
-- locks only a pack the current actor can already read (or already subscribes to for cleanup) and
-- returns the small stable-metadata tuple needed by the subscription CAS.
CREATE FUNCTION denju_lock_pack_subscription_target(target_resource_id UUID)
RETURNS TABLE(
    generation BIGINT,
    visibility TEXT,
    deleted BOOLEAN,
    owner_slug TEXT,
    resource_slug TEXT,
    current_version BIGINT,
    owner_namespace_id UUID
)
LANGUAGE SQL VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT r.generation,
           r.visibility,
           r.deleted_at IS NOT NULL,
           n.slug,
           r.slug,
           ps.current_version,
           r.owner_namespace_id
    FROM resources r
    LEFT JOIN namespaces n ON n.id = r.owner_namespace_id
    JOIN pack_state ps ON ps.resource_id = r.id
    WHERE session_user = 'denju_app'
      AND r.id = target_resource_id
      AND r.kind = 'pack'
      AND (
          (
              r.deleted_at IS NULL
              AND (
                  (denju_actor_installation_id() IS NOT NULL AND r.visibility = 'public')
                  OR (
                      denju_actor_user_id() IS NOT NULL
                      AND (
                          r.visibility = 'public'
                          OR denju_actor_has_namespace_access(r.owner_namespace_id)
                      )
                  )
              )
          )
          OR EXISTS(
              SELECT 1 FROM installation_subscriptions subscription
              WHERE subscription.installation_id = denju_actor_installation_id()
                AND subscription.resource_id = r.id
          )
          OR EXISTS(
              SELECT 1 FROM account_subscriptions subscription
              WHERE subscription.user_id = denju_actor_user_id()
                AND subscription.resource_id = r.id
          )
      )
    FOR UPDATE OF r, ps
$$;

REVOKE ALL ON FUNCTION denju_lock_pack_subscription_target(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_lock_pack_subscription_target(UUID) TO denju_app;

-- Social mutations also need narrow serialization without turning a read relationship into base
-- table UPDATE authority. Follow locks the target account against deletion; star/report lock the
-- target resource against concurrent lifecycle changes. These functions expose only public/stable
-- metadata and, for unstar cleanup, permit a resource the current actor already has a star on.
CREATE FUNCTION denju_lock_live_social_user(target_user_id UUID) RETURNS TABLE(username TEXT)
LANGUAGE SQL VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT namespace.slug
    FROM users account
    JOIN namespaces namespace ON namespace.id = account.namespace_id
    WHERE session_user = 'denju_app'
      AND account.id = target_user_id
      AND account.deleted_at IS NULL
      AND namespace.kind = 'user'
    FOR SHARE OF account
$$;

REVOKE ALL ON FUNCTION denju_lock_live_social_user(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_lock_live_social_user(UUID) TO denju_app;

CREATE FUNCTION denju_lock_social_resource(target_resource_id UUID)
RETURNS TABLE(
    owner_slug TEXT,
    resource_slug TEXT,
    resource_kind TEXT,
    visibility TEXT,
    deleted BOOLEAN,
    released BOOLEAN,
    star_count BIGINT
)
LANGUAGE SQL VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT COALESCE(namespace.slug, resource.deleted_owner_slug),
           resource.slug,
           resource.kind,
           resource.visibility,
           resource.deleted_at IS NOT NULL,
           EXISTS(
               SELECT 1 FROM skill_releases release
               WHERE release.resource_id = resource.id
                 AND release.version = resource.latest_release_version
                 AND NOT EXISTS(
                     SELECT 1 FROM resource_quarantines quarantine
                     WHERE quarantine.resource_id = resource.id
                       AND quarantine.lifted_at IS NULL
                       AND (
                           quarantine.release_version IS NULL
                           OR quarantine.release_version = release.version
                       )
                 )
           ),
           resource.star_count
    FROM resources resource
    LEFT JOIN namespaces namespace ON namespace.id = resource.owner_namespace_id
    WHERE session_user = 'denju_app'
      AND resource.id = target_resource_id
      AND (
          (resource.deleted_at IS NULL AND resource.visibility = 'public')
          OR EXISTS(
              SELECT 1 FROM resource_stars star
              WHERE star.user_id = denju_actor_user_id()
                AND star.resource_id = resource.id
          )
      )
    FOR UPDATE OF resource
$$;

REVOKE ALL ON FUNCTION denju_lock_social_resource(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_lock_social_resource(UUID) TO denju_app;

CREATE FUNCTION denju_lock_fork_source(target_resource_id UUID)
RETURNS TABLE(owner_slug TEXT, resource_slug TEXT)
LANGUAGE SQL VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT owner.slug, resource.slug
    FROM resources resource
    JOIN namespaces owner ON owner.id = resource.owner_namespace_id
    WHERE session_user = 'denju_app'
      AND resource.id = target_resource_id
      AND resource.kind = 'skill'
      AND resource.deleted_at IS NULL
      AND denju_actor_can_read_resource(resource.id)
      AND NOT EXISTS(
          SELECT 1 FROM resource_quarantines quarantine
          WHERE quarantine.resource_id = resource.id
            AND quarantine.lifted_at IS NULL
            AND (
                quarantine.release_version IS NULL
                OR quarantine.release_version = resource.latest_release_version
            )
      )
    FOR SHARE OF resource
$$;

REVOKE ALL ON FUNCTION denju_lock_fork_source(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_lock_fork_source(UUID) TO denju_app;

CREATE FUNCTION denju_lock_team_assignment_pack(target_resource_id UUID, target_team_id UUID)
RETURNS TABLE(
    owner_slug TEXT,
    resource_slug TEXT,
    visibility TEXT,
    owner_namespace_id UUID
)
LANGUAGE SQL VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT COALESCE(owner.slug, resource.deleted_owner_slug),
           resource.slug,
           resource.visibility,
           resource.owner_namespace_id
    FROM resources resource
    LEFT JOIN namespaces owner ON owner.id = resource.owner_namespace_id
    WHERE session_user = 'denju_app'
      AND EXISTS(
          SELECT 1 FROM team_memberships membership
          WHERE membership.team_namespace_id = target_team_id
            AND membership.user_id = denju_actor_user_id()
            AND membership.role = 'owner'
      )
      AND resource.id = target_resource_id
      AND resource.kind = 'pack'
      AND (
          (
              resource.deleted_at IS NULL
              AND (
                  resource.visibility = 'public'
                  OR resource.owner_namespace_id = target_team_id
              )
          )
          OR EXISTS(
              SELECT 1 FROM team_pack_assignments assignment
              WHERE assignment.team_namespace_id = target_team_id
                AND assignment.pack_resource_id = resource.id
          )
      )
    FOR UPDATE OF resource
$$;

REVOKE ALL ON FUNCTION denju_lock_team_assignment_pack(UUID,UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_lock_team_assignment_pack(UUID,UUID) TO denju_app;

CREATE FUNCTION denju_lock_proposal_rows(target_proposal_id UUID) RETURNS TABLE(locked BOOLEAN)
LANGUAGE SQL VOLATILE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT TRUE
    FROM skill_proposals proposal
    JOIN resources source ON source.id = proposal.source_resource_id
    JOIN resources target ON target.id = proposal.target_resource_id
    JOIN skill_private_workspaces source_workspace
      ON source_workspace.resource_id = source.id
     AND source_workspace.workspace_user_id = proposal.proposer_user_id
    JOIN skill_forks fork ON fork.resource_id = source.id
    WHERE session_user = 'denju_app'
      AND proposal.id = target_proposal_id
      AND (
          proposal.proposer_user_id = denju_actor_user_id()
          OR denju_actor_can_manage_resource(target.id)
      )
    FOR UPDATE OF proposal, source, target, source_workspace, fork
$$;

REVOKE ALL ON FUNCTION denju_lock_proposal_rows(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_lock_proposal_rows(UUID) TO denju_app;

CREATE FUNCTION denju_actor_can_read_workspace(
    target_resource_id UUID,
    target_workspace_user_id UUID
) RETURNS BOOLEAN
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT NOT EXISTS(
               SELECT 1 FROM resource_quarantines quarantine
               WHERE quarantine.resource_id = target_resource_id
                 AND quarantine.release_version IS NULL
                 AND quarantine.lifted_at IS NULL
           )
       AND (
           target_workspace_user_id = denju_actor_user_id()
           OR EXISTS(
            SELECT 1
            FROM resources resource
            JOIN namespaces owner ON owner.id = resource.owner_namespace_id
            JOIN private_skill_shares share ON share.resource_id = resource.id
            WHERE resource.id = target_resource_id
              AND resource.deleted_at IS NULL
              AND owner.kind = 'user'
              AND share.recipient_user_id = denju_actor_user_id()
           )
           OR denju_actor_can_review_source_resource(target_resource_id)
       )
$$;

CREATE FUNCTION denju_actor_can_read_revision_snapshot(
    target_resource_id UUID,
    target_revision_id BYTEA
) RETURNS BOOLEAN
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT EXISTS(
        SELECT 1
        FROM resources resource
        JOIN namespaces owner ON owner.id = resource.owner_namespace_id
        WHERE resource.id = target_resource_id
          AND resource.deleted_at IS NULL
          AND NOT EXISTS(
              SELECT 1 FROM resource_quarantines quarantine
              WHERE quarantine.resource_id = resource.id
                AND quarantine.release_version IS NULL
                AND quarantine.lifted_at IS NULL
          )
          AND (
              -- Immutable released revisions are readable by the current public audience and by
              -- the stable resource's currently authorized private/team audience.
              (
                  EXISTS(
                      SELECT 1 FROM skill_releases release
                      WHERE release.resource_id = resource.id
                        AND release.revision_id = target_revision_id
                        AND NOT EXISTS(
                            SELECT 1 FROM resource_quarantines quarantine
                            WHERE quarantine.resource_id = release.resource_id
                              AND quarantine.lifted_at IS NULL
                              AND (
                                  quarantine.release_version IS NULL
                                  OR quarantine.release_version = release.version
                              )
                        )
                  )
                  AND (
                      resource.visibility = 'public'
                      OR denju_actor_has_namespace_access(resource.owner_namespace_id)
                      OR EXISTS(
                          SELECT 1 FROM private_skill_shares share
                          WHERE share.resource_id = resource.id
                            AND share.recipient_user_id = denju_actor_user_id()
                      )
                  )
              )
              -- A personal owner or personal private-share recipient can inspect that personal
              -- resource's private revision history, matching the typed history surface.
              OR (
                  owner.kind = 'user'
                  AND (
                      denju_actor_has_namespace_access(resource.owner_namespace_id)
                      OR EXISTS(
                          SELECT 1 FROM private_skill_shares share
                          WHERE share.resource_id = resource.id
                            AND share.recipient_user_id = denju_actor_user_id()
                      )
                  )
              )
              -- Team drafts are maintainer-private. A publisher can read only revisions in the
              -- ancestry of their own workspace, never another maintainer's unpublished head.
              OR (
                  owner.kind = 'team'
                  AND EXISTS(
                      WITH RECURSIVE ancestry(revision_id) AS (
                          SELECT workspace.revision_id
                          FROM skill_private_workspaces workspace
                          WHERE workspace.resource_id = resource.id
                            AND workspace.workspace_user_id = denju_actor_user_id()
                          UNION
                          SELECT parent.parent_revision_id
                          FROM revision_parents parent
                          JOIN ancestry current ON current.revision_id = parent.revision_id
                      )
                      SELECT 1 FROM ancestry WHERE revision_id = target_revision_id
                  )
              )
              -- A proposal intentionally grants target publishers review access to the source
              -- fork. Application authorization still limits which proposal/revision is exposed;
              -- this RLS branch prevents the proposal workflow from being broken by tenant RLS.
              OR denju_actor_can_review_source_resource(resource.id)
          )
    ) OR EXISTS(
        -- Retain-on-delete is a frozen entitlement to exactly the tombstoned final release. It
        -- does not make arbitrary deleted history readable, and quarantine still overrides it.
        SELECT 1
        FROM resources resource
        JOIN skill_releases release
          ON release.resource_id = resource.id
         AND release.version = resource.tombstone_release_version
        WHERE resource.id = target_resource_id
          AND resource.deleted_at IS NOT NULL
          AND resource.tombstone_release_version IS NOT NULL
          AND release.revision_id = target_revision_id
          AND NOT EXISTS(
              SELECT 1 FROM resource_quarantines quarantine
              WHERE quarantine.resource_id = resource.id
                AND quarantine.lifted_at IS NULL
                AND (
                    quarantine.release_version IS NULL
                    OR quarantine.release_version = release.version
                )
          )
          AND (
              EXISTS(
                  SELECT 1 FROM account_subscriptions subscription
                  WHERE subscription.user_id = denju_actor_user_id()
                    AND subscription.resource_id = resource.id
                    AND subscription.retain_on_delete
              )
              OR EXISTS(
                  SELECT 1 FROM installation_subscriptions subscription
                  WHERE subscription.installation_id = denju_actor_installation_id()
                    AND subscription.resource_id = resource.id
                    AND subscription.retain_on_delete
              )
          )
    )
$$;

CREATE FUNCTION denju_actor_can_read_revision(target_revision_id BYTEA) RETURNS BOOLEAN
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT EXISTS(
        SELECT 1
        FROM resource_revision_snapshots snapshot
        WHERE snapshot.revision_id = target_revision_id
          AND denju_actor_can_read_revision_snapshot(snapshot.resource_id, snapshot.revision_id)
    ) OR EXISTS(
        SELECT 1
        FROM revisions revision
        JOIN author_principal_users link
          ON link.author_principal_id = revision.author_principal_id
        WHERE revision.revision_id = target_revision_id
          AND link.user_id = denju_actor_user_id()
    )
$$;

CREATE FUNCTION denju_actor_is_team_owner(target_team_namespace_id UUID) RETURNS BOOLEAN
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT EXISTS(
        SELECT 1 FROM team_memberships membership
        WHERE membership.team_namespace_id = target_team_namespace_id
          AND membership.user_id = denju_actor_user_id()
          AND membership.role = 'owner'
    )
$$;

CREATE FUNCTION denju_bind_team_invite(target_hash BYTEA) RETURNS UUID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    invite_id UUID;
BEGIN
    IF session_user <> 'denju_app' OR denju_actor_user_id() IS NULL THEN
        RAISE EXCEPTION 'team invite authentication requires user actor context'
            USING ERRCODE = '42501';
    END IF;
    IF octet_length(target_hash) <> 32 THEN
        RAISE EXCEPTION 'team invite hash is invalid'
            USING ERRCODE = '22023';
    END IF;
    SELECT invite.id INTO invite_id
    FROM team_invites invite
    WHERE invite.code_hash = target_hash;
    IF invite_id IS NOT NULL THEN
        PERFORM set_config('denju.team_invite_hash', encode(target_hash, 'hex'), true);
    END IF;
    RETURN invite_id;
END
$$;
REVOKE ALL ON FUNCTION denju_bind_team_invite(BYTEA) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_bind_team_invite(BYTEA) TO denju_app;

CREATE FUNCTION denju_bound_team_invite_id() RETURNS UUID
LANGUAGE plpgsql STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    hash_hex TEXT;
    invite_id UUID;
BEGIN
    IF session_user <> 'denju_app' THEN
        RETURN NULL;
    END IF;
    hash_hex := current_setting('denju.team_invite_hash', true);
    IF hash_hex IS NULL OR hash_hex !~ '^[0-9a-f]{64}$' THEN
        RETURN NULL;
    END IF;
    SELECT invite.id INTO invite_id
    FROM team_invites invite
    WHERE invite.code_hash = decode(hash_hex, 'hex');
    RETURN invite_id;
END
$$;
REVOKE ALL ON FUNCTION denju_bound_team_invite_id() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_bound_team_invite_id() TO denju_app;

CREATE FUNCTION denju_actor_can_add_team_member(
    target_team_namespace_id UUID,
    target_user_id UUID
) RETURNS BOOLEAN
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT denju_actor_is_team_owner(target_team_namespace_id)
       OR (
           target_user_id = denju_actor_user_id()
           AND NOT EXISTS(
               SELECT 1 FROM team_memberships existing
               WHERE existing.team_namespace_id = target_team_namespace_id
           )
       )
       OR (
           target_user_id = denju_actor_user_id()
           AND EXISTS(
               SELECT 1 FROM team_invites invite
               WHERE invite.id = denju_bound_team_invite_id()
                 AND invite.team_namespace_id = target_team_namespace_id
           )
       )
$$;

CREATE FUNCTION denju_refresh_resource_star_count(target_resource_id UUID) RETURNS BIGINT
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    refreshed BIGINT;
BEGIN
    IF session_user <> 'denju_app' OR denju_actor_user_id() IS NULL THEN
        RAISE EXCEPTION 'star-count refresh requires user actor context'
            USING ERRCODE = '42501';
    END IF;
    IF NOT EXISTS(
        SELECT 1 FROM resources resource
        WHERE resource.id = target_resource_id
          AND resource.visibility = 'public'
          AND resource.deleted_at IS NULL
    ) THEN
        RAISE EXCEPTION 'star-count refresh target is unavailable'
            USING ERRCODE = '42501';
    END IF;

    SELECT count(*) INTO refreshed
    FROM resource_stars star
    WHERE star.resource_id = target_resource_id;
    UPDATE resources SET star_count = refreshed WHERE id = target_resource_id;
    RETURN refreshed;
END
$$;
REVOKE ALL ON FUNCTION denju_refresh_resource_star_count(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_refresh_resource_star_count(UUID) TO denju_app;

CREATE FUNCTION denju_remove_actor_stars() RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    actor UUID;
BEGIN
    IF session_user <> 'denju_app' THEN
        RAISE EXCEPTION 'star cleanup is unavailable to this database role'
            USING ERRCODE = '42501';
    END IF;
    actor := denju_actor_user_id();
    IF actor IS NULL THEN
        RAISE EXCEPTION 'star cleanup requires user actor context'
            USING ERRCODE = '42501';
    END IF;

    WITH removed AS (
        DELETE FROM resource_stars WHERE user_id = actor RETURNING resource_id
    ), affected AS (
        SELECT DISTINCT resource_id FROM removed
    )
    UPDATE resources resource
    SET star_count = (
        SELECT count(*) FROM resource_stars star WHERE star.resource_id = resource.id
    )
    WHERE resource.id IN (SELECT resource_id FROM affected);
END
$$;
REVOKE ALL ON FUNCTION denju_remove_actor_stars() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_remove_actor_stars() TO denju_app;

-- Membership removal/demotion must discard the target user's team workspaces atomically with the
-- membership mutation, but the app role must not gain SELECT/UPDATE access to another
-- maintainer's draft. Expose only this destructive capability, and only to the target themself or
-- the current team owner. No draft columns are returned across the boundary.
CREATE FUNCTION denju_remove_team_workspaces_for_user(
    target_team_namespace_id UUID,
    target_user_id UUID
) RETURNS BIGINT
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    actor_user_id UUID;
    allowed BOOLEAN;
    conflicts_deleted BIGINT;
    workspaces_deleted BIGINT;
BEGIN
    IF session_user <> 'denju_app' THEN
        RAISE EXCEPTION 'team workspace cleanup is unavailable to this database role'
            USING ERRCODE = '42501';
    END IF;
    actor_user_id := public.denju_actor_user_id();
    IF actor_user_id IS NULL THEN
        RAISE EXCEPTION 'team workspace cleanup requires actor context'
            USING ERRCODE = '42501';
    END IF;
    SELECT EXISTS(
        SELECT 1 FROM public.team_memberships membership
        WHERE membership.team_namespace_id = target_team_namespace_id
          AND membership.user_id = actor_user_id
          AND (actor_user_id = target_user_id OR membership.role = 'owner')
    ) INTO allowed;
    IF NOT allowed THEN
        RAISE EXCEPTION 'team workspace cleanup is unauthorized'
            USING ERRCODE = '42501';
    END IF;

    DELETE FROM public.skill_workspace_conflicts conflict
    USING public.resources resource
    WHERE conflict.workspace_user_id = target_user_id
      AND conflict.resource_id = resource.id
      AND resource.owner_namespace_id = target_team_namespace_id
      AND resource.kind = 'skill';
    GET DIAGNOSTICS conflicts_deleted = ROW_COUNT;

    DELETE FROM public.skill_private_workspaces workspace
    USING public.resources resource
    WHERE workspace.workspace_user_id = target_user_id
      AND workspace.resource_id = resource.id
      AND resource.owner_namespace_id = target_team_namespace_id
      AND resource.kind = 'skill';
    GET DIAGNOSTICS workspaces_deleted = ROW_COUNT;

    RETURN conflicts_deleted + workspaces_deleted;
END
$$;
REVOKE ALL ON FUNCTION denju_remove_team_workspaces_for_user(UUID,UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_remove_team_workspaces_for_user(UUID,UUID) TO denju_app;
