-- Phase 7: immutable skill releases, pinned direct subscriptions, durable revision history,
-- and disposable realtime wake hints backed by an authoritative transactional outbox.

ALTER TABLE installation_subscriptions
    ADD COLUMN pinned_release_version BIGINT CHECK (pinned_release_version IS NULL OR pinned_release_version > 0);

ALTER TABLE account_subscriptions
    ADD COLUMN pinned_release_version BIGINT CHECK (pinned_release_version IS NULL OR pinned_release_version > 0);

ALTER TABLE subscription_operations
    ADD COLUMN pinned_release_version BIGINT CHECK (pinned_release_version IS NULL OR pinned_release_version > 0);

ALTER TABLE account_subscription_operations
    ADD COLUMN pinned_release_version BIGINT CHECK (pinned_release_version IS NULL OR pinned_release_version > 0);

ALTER TABLE skill_releases
    ADD COLUMN message TEXT;

CREATE TABLE skill_release_tags (
    resource_id UUID NOT NULL,
    version BIGINT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (resource_id, tag),
    FOREIGN KEY (resource_id, version)
        REFERENCES skill_releases(resource_id, version) ON DELETE CASCADE
);

CREATE TABLE resource_revision_snapshots (
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    revision_id BYTEA NOT NULL REFERENCES revisions(revision_id),
    manifest_json JSONB NOT NULL,
    snapshot_key TEXT NOT NULL,
    snapshot_sha256 BYTEA NOT NULL CHECK (octet_length(snapshot_sha256) = 32),
    snapshot_size BIGINT NOT NULL CHECK (snapshot_size >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (resource_id, revision_id)
);

-- Existing greenfield state only needs the currently reachable workspace/release snapshots
-- backfilled. New imports and private revisions insert every revision from this migration on.
INSERT INTO resource_revision_snapshots
    (resource_id, revision_id, manifest_json, snapshot_key, snapshot_sha256, snapshot_size)
SELECT resource_id, revision_id, manifest_json, snapshot_key, snapshot_sha256, snapshot_size
FROM skill_private_workspaces
ON CONFLICT DO NOTHING;

INSERT INTO resource_revision_snapshots
    (resource_id, revision_id, manifest_json, snapshot_key, snapshot_sha256, snapshot_size)
SELECT sr.resource_id, sr.revision_id, sr.manifest_json, sr.snapshot_key, sr.snapshot_sha256, sr.snapshot_size
FROM skill_releases sr
JOIN revisions revision ON revision.revision_id = sr.revision_id
ON CONFLICT DO NOTHING;

CREATE TABLE skill_release_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    outcome_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);

CREATE TABLE skill_restore_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    outcome_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);

CREATE TABLE authority_events (
    id BIGSERIAL PRIMARY KEY,
    event_kind TEXT NOT NULL,
    resource_id UUID REFERENCES resources(id) ON DELETE CASCADE,
    resource_generation BIGINT CHECK (resource_generation IS NULL OR resource_generation > 0),
    payload_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE outbox_events (
    event_id BIGINT PRIMARY KEY REFERENCES authority_events(id) ON DELETE CASCADE,
    event_kind TEXT NOT NULL,
    payload_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    dispatched_at TIMESTAMPTZ
);

CREATE INDEX outbox_events_pending_idx
    ON outbox_events (event_id) WHERE dispatched_at IS NULL;

CREATE INDEX authority_events_resource_idx
    ON authority_events (resource_id, id) WHERE resource_id IS NOT NULL;

CREATE INDEX resource_revision_snapshots_resource_created_idx
    ON resource_revision_snapshots (resource_id, created_at, revision_id);
