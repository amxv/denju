-- Phase 10: immutable fork provenance and direct private read grants.

ALTER TABLE private_import_operations
    ADD COLUMN revision_author_principal_id UUID REFERENCES author_principals(id),
    ADD COLUMN fork_upstream_resource_id UUID REFERENCES resources(id),
    ADD COLUMN fork_upstream_revision_id BYTEA CHECK (
        fork_upstream_revision_id IS NULL OR octet_length(fork_upstream_revision_id) = 32
    ),
    ADD COLUMN fork_replace_subscription BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN fork_promotion_head_revision_id BYTEA CHECK (
        fork_promotion_head_revision_id IS NULL OR octet_length(fork_promotion_head_revision_id) = 32
    ),
    ADD COLUMN historical_skill_name TEXT,
    ADD CONSTRAINT private_import_operations_fork_shape_check CHECK (
        (fork_upstream_resource_id IS NULL AND fork_upstream_revision_id IS NULL AND NOT fork_replace_subscription
            AND fork_promotion_head_revision_id IS NULL AND historical_skill_name IS NULL)
        OR (fork_upstream_resource_id IS NOT NULL AND fork_upstream_revision_id IS NOT NULL)
    );

ALTER TABLE private_revision_operations
    ADD COLUMN revision_author_principal_id UUID REFERENCES author_principals(id),
    ADD COLUMN fork_sync_base_revision_id BYTEA CHECK (
        fork_sync_base_revision_id IS NULL OR octet_length(fork_sync_base_revision_id) = 32
    ),
    ADD COLUMN fork_sync_upstream_revision_id BYTEA CHECK (
        fork_sync_upstream_revision_id IS NULL OR octet_length(fork_sync_upstream_revision_id) = 32
    ),
    ADD COLUMN historical_skill_name TEXT,
    ADD CONSTRAINT private_revision_operations_fork_sync_shape_check CHECK (
        (fork_sync_base_revision_id IS NULL AND fork_sync_upstream_revision_id IS NULL)
        OR (fork_sync_base_revision_id IS NOT NULL AND fork_sync_upstream_revision_id IS NOT NULL)
    );

CREATE TABLE skill_forks (
    resource_id UUID PRIMARY KEY REFERENCES resources(id) ON DELETE CASCADE,
    upstream_resource_id UUID NOT NULL REFERENCES resources(id),
    created_from_revision_id BYTEA NOT NULL REFERENCES revisions(revision_id),
    sync_base_revision_id BYTEA NOT NULL REFERENCES revisions(revision_id),
    promotion_head_revision_id BYTEA CHECK (
        promotion_head_revision_id IS NULL OR octet_length(promotion_head_revision_id) = 32
    ),
    promotion_pending BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (resource_id <> upstream_resource_id)
);

CREATE INDEX skill_forks_upstream_idx
    ON skill_forks (upstream_resource_id, resource_id);

CREATE TABLE private_skill_shares (
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    recipient_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (resource_id, recipient_user_id)
);

CREATE INDEX private_skill_shares_recipient_idx
    ON private_skill_shares (recipient_user_id, resource_id);

CREATE TABLE private_share_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    recipient_user_id UUID NOT NULL REFERENCES users(id),
    shared BOOLEAN NOT NULL,
    outcome_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);
