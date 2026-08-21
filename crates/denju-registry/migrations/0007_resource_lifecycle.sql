-- Phase 8: stable-ID lifecycle, direct retain-on-delete, usage/pruning, and delayed GC.

ALTER TABLE resources
    DROP CONSTRAINT resources_owner_namespace_id_kind_slug_key;

ALTER TABLE resources
    DROP CONSTRAINT resources_owner_namespace_id_fkey,
    ALTER COLUMN owner_namespace_id DROP NOT NULL;

ALTER TABLE resources
    ADD CONSTRAINT resources_owner_namespace_id_fkey
        FOREIGN KEY (owner_namespace_id) REFERENCES namespaces(id) ON DELETE SET NULL;

ALTER TABLE resources
    ADD COLUMN deleted_at TIMESTAMPTZ,
    ADD COLUMN deleted_owner_slug TEXT,
    ADD COLUMN tombstone_release_version BIGINT CHECK (
        tombstone_release_version IS NULL OR tombstone_release_version > 0
    ),
    ADD COLUMN deprecated_at TIMESTAMPTZ,
    ADD COLUMN deprecation_replacement_resource_id UUID REFERENCES resources(id);

ALTER TABLE resources
    ADD CONSTRAINT resources_active_owner_check CHECK (
        deleted_at IS NOT NULL OR owner_namespace_id IS NOT NULL
    );

CREATE UNIQUE INDEX resources_active_locator_idx
    ON resources (owner_namespace_id, kind, slug)
    WHERE deleted_at IS NULL;

CREATE INDEX resources_owner_active_idx
    ON resources (owner_namespace_id, kind, slug, id)
    WHERE deleted_at IS NULL;

CREATE TABLE resource_redirects (
    namespace_id UUID NOT NULL REFERENCES namespaces(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('skill', 'pack')),
    old_slug TEXT NOT NULL,
    target_resource_id UUID NOT NULL REFERENCES resources(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (namespace_id, kind, old_slug)
);

CREATE INDEX resource_redirects_target_idx
    ON resource_redirects (target_resource_id);

ALTER TABLE installation_subscriptions
    ADD COLUMN retain_on_delete BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE account_subscriptions
    ADD COLUMN retain_on_delete BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE subscription_operations
    ADD COLUMN retain_on_delete BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE account_subscription_operations
    ADD COLUMN retain_on_delete BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE skill_lifecycle_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    resource_id UUID NOT NULL REFERENCES resources(id),
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN ('rename', 'unpublish', 'delete', 'deprecate', 'history_prune')
    ),
    outcome_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);

CREATE TABLE canonical_blob_gc (
    blob_id BYTEA PRIMARY KEY REFERENCES canonical_blobs(blob_id) ON DELETE CASCADE,
    marked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    eligible_after TIMESTAMPTZ NOT NULL
);

CREATE INDEX canonical_blob_gc_eligible_idx
    ON canonical_blob_gc (eligible_after, blob_id);

-- Merkle tree records may outlive pruned content as small immutable transcript metadata.
-- Physical blob metadata is independently collectible once no revision/resource/namespace
-- reachability remains, so tree entries intentionally keep the semantic BlobId without an FK.
ALTER TABLE tree_entries
    DROP CONSTRAINT tree_entries_blob_id_fkey;
