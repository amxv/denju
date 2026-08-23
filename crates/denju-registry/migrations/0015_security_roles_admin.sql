-- Phase 16: typed operator authority, auditable quarantine, and security hardening.

CREATE TABLE operator_tokens (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);

CREATE TABLE resource_quarantines (
    id UUID PRIMARY KEY,
    resource_id UUID NOT NULL REFERENCES resources(id),
    release_version BIGINT CHECK (release_version IS NULL OR release_version > 0),
    reason TEXT NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 500),
    created_by_operator_id UUID NOT NULL REFERENCES operator_tokens(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lifted_by_operator_id UUID REFERENCES operator_tokens(id),
    lifted_at TIMESTAMPTZ,
    CHECK ((lifted_at IS NULL) = (lifted_by_operator_id IS NULL))
);

CREATE UNIQUE INDEX resource_quarantines_active_resource_idx
    ON resource_quarantines (resource_id)
    WHERE release_version IS NULL AND lifted_at IS NULL;

CREATE UNIQUE INDEX resource_quarantines_active_release_idx
    ON resource_quarantines (resource_id, release_version)
    WHERE release_version IS NOT NULL AND lifted_at IS NULL;

CREATE INDEX resource_quarantines_active_lookup_idx
    ON resource_quarantines (resource_id, release_version)
    WHERE lifted_at IS NULL;

CREATE TABLE admin_operations (
    operator_id UUID NOT NULL REFERENCES operator_tokens(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    operation_kind TEXT NOT NULL CHECK (operation_kind IN ('quarantine', 'unquarantine')),
    outcome_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (operator_id, operation_id)
);

CREATE TABLE operator_audit_log (
    id BIGSERIAL PRIMARY KEY,
    operator_id UUID NOT NULL REFERENCES operator_tokens(id),
    action TEXT NOT NULL,
    resource_id UUID REFERENCES resources(id),
    release_version BIGINT,
    report_id UUID REFERENCES resource_reports(id),
    detail_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX operator_audit_log_created_idx
    ON operator_audit_log (created_at DESC, id DESC);

-- The migration login owns schema changes. Request SQL and durable background work authenticate
-- directly as distinct restricted login roles with independently managed passwords. Runtime
-- connections are never allowed to log in as this migration owner and then SET ROLE: preserving
-- a privileged session_user would make RLS a cosmetic boundary because the connection could
-- regain its owner/bypass role. Deployment sets/rotates the role passwords outside migrations.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'denju_app') THEN
        CREATE ROLE denju_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
    ELSE
        ALTER ROLE denju_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'denju_worker') THEN
        CREATE ROLE denju_worker LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
    ELSE
        ALTER ROLE denju_worker LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
    END IF;
END
$$;

REVOKE ALL ON ALL TABLES IN SCHEMA public FROM PUBLIC;
REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO denju_app, denju_worker;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO denju_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO denju_app;

-- Operator bearer material is deliberately outside the ordinary request-role authority. The
-- HTTP process can validate an operator bearer only through the narrow SECURITY DEFINER function
-- below; bootstrap/revoke run with the separately supplied migration-owner connection. In
-- particular, compromising arbitrary SQL under denju_app must not allow minting or revoking an
-- operator credential by writing this table directly.
REVOKE ALL ON operator_tokens FROM denju_app, denju_worker;

GRANT SELECT, INSERT, UPDATE, DELETE ON
    authority_events,
    outbox_events,
    pack_release_event_completions,
    pack_members,
    pack_revisions,
    pack_revision_members,
    pack_state,
    resources,
    namespaces,
    resource_redirects,
    author_principals,
    merkle_trees,
    tree_entries,
    revisions,
    revision_parents,
    skill_releases,
    skill_lifecycle_operations,
    skill_private_workspaces,
    skill_workspace_conflicts,
    private_revision_operations,
    canonical_blobs,
    canonical_blob_gc,
    revision_blob_reachability,
    resource_blob_reachability,
    namespace_blob_reachability,
    resource_revision_snapshots,
    resource_search_documents
TO denju_worker;

GRANT SELECT ON users, teams, team_memberships, skill_forks, resource_quarantines TO denju_worker;

-- Canonical blob rows are global content-addressed metadata, so an ordinary tenant cannot be
-- given table-wide INSERT/SELECT just to atomically record a newly verified blob. This narrow
-- SECURITY DEFINER capability accepts only the deterministic object key for the supplied digest,
-- performs an idempotent upsert, and returns whether the stored metadata is exactly consistent.
-- Request callers must have transaction-local actor context; the separate worker login may use
-- it for trusted maintenance/dev paths without making the app login capable of SET ROLE.
CREATE FUNCTION denju_persist_canonical_blob(
    target_blob_id BYTEA,
    target_size_bytes BIGINT,
    target_object_key TEXT
) RETURNS BOOLEAN
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    expected_hex TEXT;
    expected_key TEXT;
BEGIN
    IF octet_length(target_blob_id) <> 32 OR target_size_bytes < 0 THEN
        RAISE EXCEPTION 'invalid canonical blob metadata';
    END IF;
    expected_hex := encode(target_blob_id, 'hex');
    expected_key := 'blobs/sha256/' || substring(expected_hex FROM 1 FOR 2) || '/' || expected_hex;
    IF target_object_key <> expected_key THEN
        RAISE EXCEPTION 'canonical object key does not match blob identity';
    END IF;
    IF session_user = 'denju_app' AND denju_actor_user_id() IS NULL THEN
        RAISE EXCEPTION 'canonical blob persistence requires actor context';
    END IF;
    IF session_user NOT IN ('denju_app', 'denju_worker') THEN
        RAISE EXCEPTION 'canonical blob persistence is unavailable to this database role';
    END IF;

    INSERT INTO public.canonical_blobs (blob_id,size_bytes,object_key)
    VALUES (target_blob_id,target_size_bytes,target_object_key)
    ON CONFLICT(blob_id) DO NOTHING;

    RETURN EXISTS(
        SELECT 1 FROM public.canonical_blobs
        WHERE blob_id=target_blob_id
          AND size_bytes=target_size_bytes
          AND object_key=target_object_key
    );
END
$$;
REVOKE ALL ON FUNCTION denju_persist_canonical_blob(BYTEA,BIGINT,TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_persist_canonical_blob(BYTEA,BIGINT,TEXT) TO denju_app, denju_worker;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO denju_worker;

-- `tree_entries` is global content-addressed structure. The request role deliberately has no
-- table-wide SELECT because path names/blob IDs from another tenant's private tree are sensitive.
-- PostgreSQL nevertheless requires read privilege on an `ON CONFLICT` target, so expose only the
-- semantic idempotent insert Denju needs while building a verified manifest. A caller cannot use
-- this function to enumerate existing entries: it returns only whether the exact supplied row is
-- now present.
CREATE FUNCTION denju_persist_tree_entry(
    target_tree_id BYTEA,
    target_name TEXT,
    target_kind TEXT,
    target_blob_id BYTEA,
    target_child_tree_id BYTEA,
    target_executable BOOLEAN,
    target_symlink_target TEXT
) RETURNS BOOLEAN
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF session_user = 'denju_app' AND denju_actor_user_id() IS NULL THEN
        RAISE EXCEPTION 'tree persistence requires actor context'
            USING ERRCODE = '42501';
    END IF;
    IF session_user NOT IN ('denju_app', 'denju_worker') THEN
        RAISE EXCEPTION 'tree persistence is unavailable to this database role'
            USING ERRCODE = '42501';
    END IF;
    IF octet_length(target_tree_id) <> 32
       OR target_name IS NULL OR target_name = ''
       OR target_kind NOT IN ('file','directory','symlink') THEN
        RAISE EXCEPTION 'invalid tree entry metadata'
            USING ERRCODE = '22023';
    END IF;
    IF (target_kind = 'file' AND (
            target_blob_id IS NULL OR octet_length(target_blob_id) <> 32
            OR target_child_tree_id IS NOT NULL OR target_symlink_target IS NOT NULL
            OR target_executable IS NULL
        ))
       OR (target_kind = 'directory' AND (
            target_blob_id IS NOT NULL OR target_child_tree_id IS NULL
            OR octet_length(target_child_tree_id) <> 32
            OR target_symlink_target IS NOT NULL OR target_executable IS NOT NULL
        ))
       OR (target_kind = 'symlink' AND (
            target_blob_id IS NOT NULL OR target_child_tree_id IS NOT NULL
            OR target_symlink_target IS NULL OR target_executable IS NOT NULL
        )) THEN
        RAISE EXCEPTION 'invalid tree entry shape'
            USING ERRCODE = '22023';
    END IF;

    INSERT INTO public.tree_entries
        (tree_id,name,kind,blob_id,child_tree_id,executable,symlink_target)
    VALUES
        (target_tree_id,target_name,target_kind,target_blob_id,target_child_tree_id,
         target_executable,target_symlink_target)
    ON CONFLICT(tree_id,name) DO NOTHING;

    RETURN EXISTS(
        SELECT 1 FROM public.tree_entries entry
        WHERE entry.tree_id = target_tree_id
          AND entry.name = target_name
          AND entry.kind = target_kind
          AND entry.blob_id IS NOT DISTINCT FROM target_blob_id
          AND entry.child_tree_id IS NOT DISTINCT FROM target_child_tree_id
          AND entry.executable IS NOT DISTINCT FROM target_executable
          AND entry.symlink_target IS NOT DISTINCT FROM target_symlink_target
    );
END
$$;
REVOKE ALL ON FUNCTION denju_persist_tree_entry(BYTEA,TEXT,TEXT,BYTEA,BYTEA,BOOLEAN,TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_persist_tree_entry(BYTEA,TEXT,TEXT,BYTEA,BYTEA,BOOLEAN,TEXT)
    TO denju_app, denju_worker;

-- A freshly-created revision is not yet reachable from a resource snapshot, so the ordinary
-- revision SELECT policy cannot authorize PostgreSQL's read side of `ON CONFLICT`. Keep the base
-- table private and expose only exact semantic idempotency. Request callers may persist a revision
-- only for an author principal already linked to their transaction-local user actor.
CREATE FUNCTION denju_persist_revision(
    target_revision_id BYTEA,
    target_root_tree_id BYTEA,
    target_author_principal_id UUID,
    target_operation_id UUID
) RETURNS BOOLEAN
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF octet_length(target_revision_id) <> 32 OR octet_length(target_root_tree_id) <> 32 THEN
        RAISE EXCEPTION 'invalid revision identity metadata'
            USING ERRCODE = '22023';
    END IF;
    IF session_user = 'denju_app' THEN
        IF denju_actor_user_id() IS NULL OR NOT EXISTS(
            SELECT 1 FROM public.author_principal_users link
            WHERE link.author_principal_id = target_author_principal_id
              AND link.user_id = denju_actor_user_id()
        ) THEN
            RAISE EXCEPTION 'revision author is not linked to the active actor'
                USING ERRCODE = '42501';
        END IF;
    ELSIF session_user <> 'denju_worker' THEN
        RAISE EXCEPTION 'revision persistence is unavailable to this database role'
            USING ERRCODE = '42501';
    END IF;

    INSERT INTO public.revisions
        (revision_id,root_tree_id,author_principal_id,operation_id)
    VALUES
        (target_revision_id,target_root_tree_id,target_author_principal_id,target_operation_id)
    ON CONFLICT(revision_id) DO NOTHING;

    RETURN EXISTS(
        SELECT 1 FROM public.revisions revision
        WHERE revision.revision_id = target_revision_id
          AND revision.root_tree_id = target_root_tree_id
          AND revision.author_principal_id = target_author_principal_id
          AND revision.operation_id = target_operation_id
    );
END
$$;
REVOKE ALL ON FUNCTION denju_persist_revision(BYTEA,BYTEA,UUID,UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_persist_revision(BYTEA,BYTEA,UUID,UUID)
    TO denju_app, denju_worker;

-- Do not grant future migration-created tables to the request role implicitly. Each later
-- migration must make its runtime privilege decision deliberately; this prevents a future admin
-- or secret-bearing table from silently inheriting request-role DML.

CREATE FUNCTION denju_actor_user_id() RETURNS UUID
LANGUAGE SQL STABLE PARALLEL SAFE
AS $$
    SELECT NULLIF(current_setting('denju.actor_user_id', true), '')::uuid
$$;

CREATE FUNCTION denju_actor_installation_id() RETURNS UUID
LANGUAGE SQL STABLE PARALLEL SAFE
AS $$
    SELECT NULLIF(current_setting('denju.actor_installation_id', true), '')::uuid
$$;

CREATE FUNCTION denju_authenticate_operator(target_hash BYTEA)
RETURNS TABLE(operator_id UUID, operator_name TEXT)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF session_user <> 'denju_app' THEN
        RAISE EXCEPTION 'operator authentication is unavailable to this database role'
            USING ERRCODE = '42501';
    END IF;
    IF octet_length(target_hash) <> 32 THEN
        RAISE EXCEPTION 'operator credential hash is invalid'
            USING ERRCODE = '22023';
    END IF;

    -- Bind subsequent operator-only RLS checks in this transaction to the presented bearer
    -- digest. The bearer itself is never copied into PostgreSQL state or a transaction setting.
    PERFORM set_config('denju.operator_token_hash', encode(target_hash, 'hex'), true);
    RETURN QUERY
        SELECT token.id, token.name
        FROM public.operator_tokens token
        WHERE token.token_hash = target_hash
          AND token.revoked_at IS NULL;
END
$$;
REVOKE ALL ON FUNCTION denju_authenticate_operator(BYTEA) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_authenticate_operator(BYTEA) TO denju_app;

CREATE FUNCTION denju_active_operator_id() RETURNS UUID
LANGUAGE plpgsql STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    hash_hex TEXT;
    decoded_hash BYTEA;
    active_id UUID;
BEGIN
    IF session_user <> 'denju_app' THEN
        RETURN NULL;
    END IF;
    hash_hex := current_setting('denju.operator_token_hash', true);
    IF hash_hex IS NULL OR hash_hex !~ '^[0-9a-f]{64}$' THEN
        RETURN NULL;
    END IF;
    decoded_hash := decode(hash_hex, 'hex');
    SELECT token.id INTO active_id
    FROM public.operator_tokens token
    WHERE token.token_hash = decoded_hash
      AND token.revoked_at IS NULL;
    RETURN active_id;
END
$$;
REVOKE ALL ON FUNCTION denju_active_operator_id() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_active_operator_id() TO denju_app;

-- Runtime credentials are never table-scan capabilities. Authentication supplies an exact
-- SHA-256 bearer digest to these owner-executed lookups; the request role receives only the
-- authority metadata needed by Rust and never SELECT privilege on the stored credential column.
CREATE FUNCTION denju_authenticate_installation(target_hash BYTEA) RETURNS UUID
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT installation.id
    FROM installations installation
    WHERE session_user = 'denju_app'
      AND octet_length(target_hash) = 32
      AND installation.credential_hash = target_hash
      AND installation.revoked_at IS NULL
$$;
REVOKE ALL ON FUNCTION denju_authenticate_installation(BYTEA) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_authenticate_installation(BYTEA) TO denju_app;

CREATE FUNCTION denju_lookup_installation_any(target_hash BYTEA) RETURNS UUID
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT installation.id
    FROM installations installation
    WHERE session_user = 'denju_app'
      AND octet_length(target_hash) = 32
      AND installation.credential_hash = target_hash
$$;
REVOKE ALL ON FUNCTION denju_lookup_installation_any(BYTEA) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_lookup_installation_any(BYTEA) TO denju_app;

CREATE FUNCTION denju_authenticate_session(target_hash BYTEA)
RETURNS TABLE(session_id UUID, user_id UUID, installation_id UUID)
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT session.id, session.user_id, session.installation_id
    FROM sessions session
    WHERE session_user = 'denju_app'
      AND octet_length(target_hash) = 32
      AND session.token_hash = target_hash
      AND session.revoked_at IS NULL
$$;
REVOKE ALL ON FUNCTION denju_authenticate_session(BYTEA) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_authenticate_session(BYTEA) TO denju_app;

CREATE FUNCTION denju_lookup_session_user_any(target_hash BYTEA) RETURNS UUID
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT session.user_id
    FROM sessions session
    WHERE session_user = 'denju_app'
      AND octet_length(target_hash) = 32
      AND session.token_hash = target_hash
$$;
REVOKE ALL ON FUNCTION denju_lookup_session_user_any(BYTEA) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_lookup_session_user_any(BYTEA) TO denju_app;

CREATE FUNCTION denju_authenticate_automation(target_hash BYTEA)
RETURNS TABLE(user_id UUID, scopes JSONB)
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT token.user_id, token.scopes
    FROM automation_tokens token
    WHERE session_user = 'denju_app'
      AND octet_length(target_hash) = 32
      AND token.token_hash = target_hash
      AND token.revoked_at IS NULL
      AND token.expires_at > now()
$$;
REVOKE ALL ON FUNCTION denju_authenticate_automation(BYTEA) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_authenticate_automation(BYTEA) TO denju_app;

CREATE FUNCTION denju_login_candidate(target_username TEXT)
RETURNS TABLE(user_id UUID, namespace_id UUID, password_hash TEXT, author_principal_id UUID)
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT user_row.id, user_row.namespace_id, user_row.password_hash, user_row.author_principal_id
    FROM users user_row
    JOIN namespaces namespace ON namespace.id = user_row.namespace_id
    WHERE session_user = 'denju_app'
      AND namespace.slug = target_username
      AND user_row.deleted_at IS NULL
$$;
REVOKE ALL ON FUNCTION denju_login_candidate(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_login_candidate(TEXT) TO denju_app;

CREATE FUNCTION denju_recovery_candidate(target_username TEXT, target_hash BYTEA)
RETURNS TABLE(user_id UUID, namespace_id UUID, author_principal_id UUID)
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT user_row.id, user_row.namespace_id, user_row.author_principal_id
    FROM users user_row
    JOIN namespaces namespace ON namespace.id = user_row.namespace_id
    WHERE session_user = 'denju_app'
      AND octet_length(target_hash) = 32
      AND namespace.slug = target_username
      AND user_row.deleted_at IS NULL
      AND user_row.recovery_secret_hash = target_hash
$$;
REVOKE ALL ON FUNCTION denju_recovery_candidate(TEXT,BYTEA) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_recovery_candidate(TEXT,BYTEA) TO denju_app;

CREATE FUNCTION denju_actor_password_hash(target_user_id UUID) RETURNS TEXT
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT user_row.password_hash
    FROM users user_row
    WHERE session_user = 'denju_app'
      AND target_user_id = denju_actor_user_id()
      AND user_row.id = target_user_id
      AND user_row.deleted_at IS NULL
$$;
REVOKE ALL ON FUNCTION denju_actor_password_hash(UUID) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_actor_password_hash(UUID) TO denju_app;

CREATE FUNCTION denju_cancel_blob_gc(target_blob BYTEA) RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF session_user <> 'denju_app' OR octet_length(target_blob) <> 32 THEN
        RAISE EXCEPTION 'blob GC cancellation is unavailable'
            USING ERRCODE = '42501';
    END IF;
    DELETE FROM canonical_blob_gc WHERE blob_id = target_blob;
END
$$;
REVOKE ALL ON FUNCTION denju_cancel_blob_gc(BYTEA) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION denju_cancel_blob_gc(BYTEA) TO denju_app;

CREATE FUNCTION denju_actor_has_namespace_access(target_namespace_id UUID) RETURNS BOOLEAN
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT EXISTS(
        SELECT 1 FROM users u
        WHERE u.id = denju_actor_user_id()
          AND u.deleted_at IS NULL
          AND u.namespace_id = target_namespace_id
    ) OR EXISTS(
        SELECT 1 FROM team_memberships tm
        WHERE tm.user_id = denju_actor_user_id()
          AND tm.team_namespace_id = target_namespace_id
    )
$$;

CREATE FUNCTION denju_actor_can_publish_namespace(target_namespace_id UUID) RETURNS BOOLEAN
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT EXISTS(
        SELECT 1 FROM users u
        WHERE u.id = denju_actor_user_id()
          AND u.deleted_at IS NULL
          AND u.namespace_id = target_namespace_id
    ) OR EXISTS(
        SELECT 1 FROM team_memberships tm
        JOIN teams team ON team.namespace_id = tm.team_namespace_id
        WHERE tm.user_id = denju_actor_user_id()
          AND tm.team_namespace_id = target_namespace_id
          AND (
              tm.role IN ('owner', 'maintainer')
              OR (tm.role = 'member' AND team.members_can_publish)
          )
    )
$$;

CREATE FUNCTION denju_actor_can_manage_resource(target_resource_id UUID) RETURNS BOOLEAN
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT EXISTS(
        SELECT 1 FROM resources r
        WHERE r.id = target_resource_id
          AND denju_actor_can_publish_namespace(r.owner_namespace_id)
          AND NOT EXISTS(
              SELECT 1 FROM resource_quarantines quarantine
              WHERE quarantine.resource_id = r.id
                AND quarantine.release_version IS NULL
                AND quarantine.lifted_at IS NULL
          )
    )
$$;

CREATE FUNCTION denju_actor_can_review_source_resource(target_resource_id UUID) RETURNS BOOLEAN
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT EXISTS(
        SELECT 1
        FROM skill_proposals proposal
        JOIN resources source ON source.id = proposal.source_resource_id
        JOIN resources target ON target.id = proposal.target_resource_id
        WHERE proposal.source_resource_id = target_resource_id
          AND source.deleted_at IS NULL
          AND target.deleted_at IS NULL
          AND NOT EXISTS(
              SELECT 1 FROM resource_quarantines quarantine
              WHERE quarantine.resource_id = source.id
                AND quarantine.release_version IS NULL
                AND quarantine.lifted_at IS NULL
          )
          AND (
              proposal.proposer_user_id = denju_actor_user_id()
              OR denju_actor_can_publish_namespace(target.owner_namespace_id)
          )
    )
$$;

CREATE FUNCTION denju_actor_can_read_resource(target_resource_id UUID) RETURNS BOOLEAN
LANGUAGE SQL STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT EXISTS(
        SELECT 1 FROM resources r
        WHERE r.id = target_resource_id
          AND (
              (r.visibility = 'public' AND r.deleted_at IS NULL)
              OR denju_actor_has_namespace_access(r.owner_namespace_id)
              OR EXISTS(
                  SELECT 1 FROM private_skill_shares share
                  WHERE share.resource_id = r.id
                    AND share.recipient_user_id = denju_actor_user_id()
                    AND r.deleted_at IS NULL
              )
              OR (r.deleted_at IS NULL AND denju_actor_can_review_source_resource(r.id))
          )
    ) OR EXISTS(
        SELECT 1 FROM account_subscriptions subscription
        JOIN resources r ON r.id = subscription.resource_id
        WHERE subscription.user_id = denju_actor_user_id()
          AND subscription.resource_id = target_resource_id
          AND subscription.retain_on_delete
          AND r.deleted_at IS NOT NULL
    ) OR EXISTS(
        SELECT 1 FROM installation_subscriptions subscription
        JOIN resources r ON r.id = subscription.resource_id
        WHERE subscription.installation_id = denju_actor_installation_id()
          AND subscription.resource_id = target_resource_id
          AND subscription.retain_on_delete
          AND r.deleted_at IS NOT NULL
    )
$$;
