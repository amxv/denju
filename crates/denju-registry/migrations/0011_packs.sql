-- Phase 12: reproducible pack revisions, authored member intent, generic pack
-- subscriptions, and durable ordered follow-latest advancement.

CREATE TABLE pack_state (
    resource_id UUID PRIMARY KEY REFERENCES resources(id) ON DELETE CASCADE,
    current_version BIGINT NOT NULL CHECK (current_version > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE pack_members (
    pack_resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    skill_resource_id UUID NOT NULL REFERENCES resources(id),
    pinned_release_version BIGINT CHECK (pinned_release_version IS NULL OR pinned_release_version > 0),
    follow_after_event_id BIGINT NOT NULL CHECK (follow_after_event_id >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (pack_resource_id, skill_resource_id),
    CHECK (pack_resource_id <> skill_resource_id)
);

CREATE INDEX pack_members_follow_latest_idx
    ON pack_members (skill_resource_id, follow_after_event_id, pack_resource_id)
    WHERE pinned_release_version IS NULL;

CREATE TABLE pack_revisions (
    pack_resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    version BIGINT NOT NULL CHECK (version > 0),
    source_release_event_id BIGINT REFERENCES authority_events(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (pack_resource_id, version)
);

CREATE UNIQUE INDEX pack_revisions_release_event_idx
    ON pack_revisions (pack_resource_id, source_release_event_id)
    WHERE source_release_event_id IS NOT NULL;

CREATE TABLE pack_revision_members (
    pack_resource_id UUID NOT NULL,
    pack_version BIGINT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    skill_resource_id UUID NOT NULL REFERENCES resources(id),
    pinned_release_version BIGINT CHECK (pinned_release_version IS NULL OR pinned_release_version > 0),
    resolved_release_version BIGINT CHECK (resolved_release_version IS NULL OR resolved_release_version > 0),
    resolved_revision_id BYTEA NOT NULL REFERENCES revisions(revision_id),
    PRIMARY KEY (pack_resource_id, pack_version, skill_resource_id),
    UNIQUE (pack_resource_id, pack_version, ordinal),
    FOREIGN KEY (pack_resource_id, pack_version)
        REFERENCES pack_revisions(pack_resource_id, version) ON DELETE CASCADE
);

CREATE INDEX pack_revision_members_skill_idx
    ON pack_revision_members (skill_resource_id, pack_resource_id, pack_version);

CREATE TABLE pack_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    resource_id UUID NOT NULL REFERENCES resources(id),
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN ('create', 'add', 'remove', 'publish', 'rename', 'unpublish', 'delete')
    ),
    outcome_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);

-- An authority release event is complete only after every pack that was following
-- that skill at the time of publication has its own exact next immutable version.
-- Individual per-pack completion is represented by pack_revisions.source_release_event_id,
-- so a crash can resume a partially drained high-fanout event without duplicating history.
CREATE TABLE pack_release_event_completions (
    event_id BIGINT PRIMARY KEY REFERENCES authority_events(id) ON DELETE CASCADE,
    completed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX pack_release_event_completions_completed_idx
    ON pack_release_event_completions (event_id);

-- Skill subscription replay can be reconstructed exactly from immutable request fields,
-- but pack subscription responses also contain the pack generation/version observed by the
-- original mutation. Preserve that committed response so a later exact retry cannot drift
-- after the live pack has advanced.
ALTER TABLE subscription_operations
    ADD COLUMN pack_outcome_json JSONB;

ALTER TABLE account_subscription_operations
    ADD COLUMN pack_outcome_json JSONB;

-- Packs did not exist before this migration. Historical release events must not be
-- replayed into packs created later; new follow-latest intent records its own event
-- boundary and every future release event carries exact release metadata.
INSERT INTO pack_release_event_completions (event_id)
SELECT id FROM authority_events WHERE event_kind = 'skill_release_published'
ON CONFLICT DO NOTHING;
