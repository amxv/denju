CREATE TABLE private_revision_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    namespace_id UUID NOT NULL REFERENCES namespaces(id),
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    expected_generation BIGINT NOT NULL CHECK (expected_generation > 0),
    parent_revision_id BYTEA NOT NULL CHECK (octet_length(parent_revision_id) = 32),
    revision_id BYTEA NOT NULL CHECK (octet_length(revision_id) = 32),
    root_tree_id BYTEA NOT NULL CHECK (octet_length(root_tree_id) = 32),
    manifest_json JSONB NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('prepared', 'committed')),
    outcome_json JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);

CREATE TABLE private_revision_staging (
    user_id UUID NOT NULL,
    operation_id UUID NOT NULL,
    blob_id BYTEA NOT NULL CHECK (octet_length(blob_id) = 32),
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    staging_key TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id, blob_id),
    FOREIGN KEY (user_id, operation_id)
        REFERENCES private_revision_operations(user_id, operation_id) ON DELETE CASCADE
);

CREATE INDEX private_revision_operations_resource_idx
    ON private_revision_operations (resource_id, state, expected_generation, created_at);
