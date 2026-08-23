-- Phase 15: profiles, social graph, metadata-only search/ranking, and private reports.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

ALTER TABLE users
    ADD COLUMN bio TEXT,
    ADD COLUMN followers_visible BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN following_visible BOOLEAN NOT NULL DEFAULT TRUE,
    ADD CONSTRAINT users_bio_length_check CHECK (bio IS NULL OR char_length(bio) <= 500);

ALTER TABLE resources
    ADD COLUMN license TEXT,
    ADD COLUMN compatibility TEXT,
    ADD COLUMN discovery_topics TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN star_count BIGINT NOT NULL DEFAULT 0,
    ADD CONSTRAINT resources_star_count_check CHECK (star_count >= 0),
    ADD CONSTRAINT resources_discovery_topics_count_check CHECK (cardinality(discovery_topics) <= 12);

ALTER TABLE skill_private_workspaces
    ADD COLUMN license TEXT,
    ADD COLUMN compatibility TEXT;

CREATE TABLE user_follows (
    follower_user_id UUID NOT NULL REFERENCES users(id),
    followed_user_id UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (follower_user_id, followed_user_id),
    CHECK (follower_user_id <> followed_user_id)
);

CREATE INDEX user_follows_followed_idx
    ON user_follows (followed_user_id, follower_user_id);

CREATE TABLE resource_stars (
    user_id UUID NOT NULL REFERENCES users(id),
    resource_id UUID NOT NULL REFERENCES resources(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, resource_id)
);

CREATE INDEX resource_stars_resource_idx
    ON resource_stars (resource_id, user_id);

CREATE TABLE social_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN ('profile_update', 'follow', 'unfollow', 'star', 'unstar', 'topics', 'report')
    ),
    outcome_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);

CREATE TABLE resource_reports (
    id UUID PRIMARY KEY,
    reporter_user_id UUID REFERENCES users(id),
    resource_id UUID NOT NULL REFERENCES resources(id),
    reason TEXT NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 64),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX resource_reports_operator_idx
    ON resource_reports (created_at, id);

-- Search documents are derived metadata only. They intentionally contain no manifest JSON,
-- SKILL.md body, script content, blob identity, or object-store location.
CREATE TABLE resource_search_documents (
    resource_id UUID PRIMARY KEY REFERENCES resources(id) ON DELETE CASCADE,
    owner_slug TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('skill', 'pack')),
    resource_slug TEXT NOT NULL,
    description TEXT NOT NULL,
    license TEXT,
    compatibility TEXT,
    topics TEXT[] NOT NULL DEFAULT '{}',
    fork_upstream_locator TEXT,
    pack_membership_text TEXT NOT NULL DEFAULT '',
    star_count BIGINT NOT NULL CHECK (star_count >= 0),
    search_text TEXT NOT NULL,
    search_vector TSVECTOR NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX resource_search_documents_fts_idx
    ON resource_search_documents USING GIN (search_vector);

CREATE INDEX resource_search_documents_trgm_idx
    ON resource_search_documents USING GIN (search_text gin_trgm_ops);

CREATE INDEX resource_search_documents_topics_idx
    ON resource_search_documents USING GIN (topics);

CREATE INDEX resource_search_documents_stars_idx
    ON resource_search_documents (star_count DESC, owner_slug, resource_slug, resource_id);

CREATE INDEX resources_social_access_idx
    ON resources (visibility, kind, owner_namespace_id, id)
    WHERE deleted_at IS NULL;

CREATE OR REPLACE FUNCTION denju_refresh_search_document(target_resource_id UUID)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    row_record RECORD;
    combined_text TEXT;
BEGIN
    SELECT r.id,
           n.slug AS owner_slug,
           r.kind AS resource_kind,
           r.slug AS resource_slug,
           r.description,
           r.license,
           r.compatibility,
           r.discovery_topics AS topics,
           r.star_count,
           CASE
               WHEN f.resource_id IS NULL THEN NULL
               ELSE '@' || COALESCE(upstream_owner.slug, upstream.deleted_owner_slug) || '/' || upstream.slug
           END AS fork_upstream_locator,
           COALESCE((
               SELECT string_agg(
                   '@' || COALESCE(member_owner.slug, member.deleted_owner_slug) || '/' || member.slug,
                   ' ' ORDER BY COALESCE(member_owner.slug, member.deleted_owner_slug), member.slug, member.id
               )
               FROM pack_members pm
               JOIN resources member ON member.id=pm.skill_resource_id AND member.deleted_at IS NULL
               LEFT JOIN namespaces member_owner ON member_owner.id=member.owner_namespace_id
               WHERE pm.pack_resource_id=r.id
           ), '') AS pack_membership_text
      INTO row_record
      FROM resources r
      JOIN namespaces n ON n.id=r.owner_namespace_id
      LEFT JOIN skill_forks f ON f.resource_id=r.id
      LEFT JOIN resources upstream ON upstream.id=f.upstream_resource_id
      LEFT JOIN namespaces upstream_owner ON upstream_owner.id=upstream.owner_namespace_id
     WHERE r.id=target_resource_id
       AND r.deleted_at IS NULL
       AND COALESCE(f.promotion_pending,FALSE)=FALSE;

    IF NOT FOUND THEN
        DELETE FROM resource_search_documents WHERE resource_id=target_resource_id;
        RETURN;
    END IF;

    combined_text := row_record.owner_slug || ' ' || row_record.resource_slug || ' ' || row_record.description || ' ' ||
        COALESCE(row_record.license, '') || ' ' || COALESCE(row_record.compatibility, '') || ' ' ||
        array_to_string(row_record.topics, ' ') || ' ' || COALESCE(row_record.fork_upstream_locator, '') || ' ' ||
        row_record.pack_membership_text;

    INSERT INTO resource_search_documents (
        resource_id,owner_slug,resource_kind,resource_slug,description,license,compatibility,topics,
        fork_upstream_locator,pack_membership_text,star_count,search_text,search_vector,updated_at
    ) VALUES (
        row_record.id,row_record.owner_slug,row_record.resource_kind,row_record.resource_slug,row_record.description,
        row_record.license,row_record.compatibility,row_record.topics,row_record.fork_upstream_locator,
        row_record.pack_membership_text,row_record.star_count,combined_text,to_tsvector('simple', combined_text),now()
    )
    ON CONFLICT(resource_id) DO UPDATE SET
        owner_slug=excluded.owner_slug,
        resource_kind=excluded.resource_kind,
        resource_slug=excluded.resource_slug,
        description=excluded.description,
        license=excluded.license,
        compatibility=excluded.compatibility,
        topics=excluded.topics,
        fork_upstream_locator=excluded.fork_upstream_locator,
        pack_membership_text=excluded.pack_membership_text,
        star_count=excluded.star_count,
        search_text=excluded.search_text,
        search_vector=excluded.search_vector,
        updated_at=now();
END;
$$;

CREATE OR REPLACE FUNCTION denju_resource_search_trigger()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    dependent UUID;
    target_id UUID;
BEGIN
    target_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.id ELSE NEW.id END;
    PERFORM denju_refresh_search_document(target_id);

    -- A renamed/transferred upstream changes public fork provenance metadata.
    FOR dependent IN
        SELECT sf.resource_id FROM skill_forks sf WHERE sf.upstream_resource_id=target_id
    LOOP
        PERFORM denju_refresh_search_document(dependent);
    END LOOP;

    -- A renamed/transferred member changes the metadata-only member labels of containing packs.
    FOR dependent IN
        SELECT pm.pack_resource_id FROM pack_members pm WHERE pm.skill_resource_id=target_id
    LOOP
        PERFORM denju_refresh_search_document(dependent);
    END LOOP;
    RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END;
$$;

CREATE TRIGGER resources_search_document_refresh
AFTER INSERT OR UPDATE OF owner_namespace_id,slug,kind,description,license,compatibility,discovery_topics,star_count,deleted_at
ON resources
FOR EACH ROW EXECUTE FUNCTION denju_resource_search_trigger();

CREATE OR REPLACE FUNCTION denju_pack_member_search_trigger()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP <> 'INSERT' AND OLD.pack_resource_id IS NOT NULL THEN
        PERFORM denju_refresh_search_document(OLD.pack_resource_id);
    END IF;
    IF TG_OP <> 'DELETE' AND NEW.pack_resource_id IS NOT NULL AND
       (TG_OP = 'INSERT' OR NEW.pack_resource_id <> OLD.pack_resource_id) THEN
        PERFORM denju_refresh_search_document(NEW.pack_resource_id);
    END IF;
    RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END;
$$;

CREATE TRIGGER pack_members_search_document_refresh
AFTER INSERT OR UPDATE OR DELETE ON pack_members
FOR EACH ROW EXECUTE FUNCTION denju_pack_member_search_trigger();

CREATE OR REPLACE FUNCTION denju_fork_search_trigger()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM denju_refresh_search_document(OLD.resource_id);
        RETURN OLD;
    ELSE
        PERFORM denju_refresh_search_document(NEW.resource_id);
        RETURN NEW;
    END IF;
END;
$$;

CREATE TRIGGER skill_forks_search_document_refresh
AFTER INSERT OR UPDATE OR DELETE ON skill_forks
FOR EACH ROW EXECUTE FUNCTION denju_fork_search_trigger();

-- Existing pre-release rows receive a derived document immediately. License/compatibility were
-- not persisted before this phase and therefore begin NULL on old disposable development rows.
SELECT denju_refresh_search_document(id) FROM resources;
