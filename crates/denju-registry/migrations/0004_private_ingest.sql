ALTER TABLE resources
    ALTER COLUMN latest_release_version DROP NOT NULL;

ALTER TABLE resources
    ADD CONSTRAINT resources_release_visibility_check CHECK (
        (visibility = 'public' AND latest_release_version IS NOT NULL)
        OR visibility = 'private'
    );

CREATE TABLE canonical_blobs (
    blob_id BYTEA PRIMARY KEY CHECK (octet_length(blob_id) = 32),
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    object_key TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE merkle_trees (
    tree_id BYTEA PRIMARY KEY CHECK (octet_length(tree_id) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE tree_entries (
    tree_id BYTEA NOT NULL REFERENCES merkle_trees(tree_id),
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('file', 'directory', 'symlink')),
    blob_id BYTEA REFERENCES canonical_blobs(blob_id),
    child_tree_id BYTEA REFERENCES merkle_trees(tree_id),
    symlink_target TEXT,
    executable BOOLEAN,
    PRIMARY KEY (tree_id, name),
    CHECK (
        (kind = 'file' AND blob_id IS NOT NULL AND child_tree_id IS NULL AND symlink_target IS NULL AND executable IS NOT NULL)
        OR
        (kind = 'directory' AND blob_id IS NULL AND child_tree_id IS NOT NULL AND symlink_target IS NULL AND executable IS NULL)
        OR
        (kind = 'symlink' AND blob_id IS NULL AND child_tree_id IS NULL AND symlink_target IS NOT NULL AND executable IS NULL)
    )
);

CREATE TABLE revisions (
    revision_id BYTEA PRIMARY KEY CHECK (octet_length(revision_id) = 32),
    root_tree_id BYTEA NOT NULL REFERENCES merkle_trees(tree_id),
    author_principal_id UUID NOT NULL REFERENCES author_principals(id),
    operation_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE revision_parents (
    revision_id BYTEA NOT NULL REFERENCES revisions(revision_id) ON DELETE CASCADE,
    parent_revision_id BYTEA NOT NULL CHECK (octet_length(parent_revision_id) = 32),
    ordinal SMALLINT NOT NULL CHECK (ordinal IN (0, 1)),
    PRIMARY KEY (revision_id, ordinal),
    UNIQUE (revision_id, parent_revision_id)
);

CREATE TABLE revision_blob_reachability (
    revision_id BYTEA NOT NULL REFERENCES revisions(revision_id) ON DELETE CASCADE,
    blob_id BYTEA NOT NULL REFERENCES canonical_blobs(blob_id),
    PRIMARY KEY (revision_id, blob_id)
);

CREATE TABLE resource_blob_reachability (
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    blob_id BYTEA NOT NULL REFERENCES canonical_blobs(blob_id),
    reference_count BIGINT NOT NULL CHECK (reference_count > 0),
    PRIMARY KEY (resource_id, blob_id)
);

CREATE TABLE namespace_blob_reachability (
    namespace_id UUID NOT NULL REFERENCES namespaces(id) ON DELETE CASCADE,
    blob_id BYTEA NOT NULL REFERENCES canonical_blobs(blob_id),
    reference_count BIGINT NOT NULL CHECK (reference_count > 0),
    PRIMARY KEY (namespace_id, blob_id)
);

CREATE TABLE skill_private_workspaces (
    resource_id UUID PRIMARY KEY REFERENCES resources(id) ON DELETE CASCADE,
    revision_id BYTEA NOT NULL REFERENCES revisions(revision_id),
    generation BIGINT NOT NULL CHECK (generation > 0),
    manifest_json JSONB NOT NULL,
    snapshot_key TEXT NOT NULL,
    snapshot_sha256 BYTEA NOT NULL CHECK (octet_length(snapshot_sha256) = 32),
    snapshot_size BIGINT NOT NULL CHECK (snapshot_size >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE private_import_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    namespace_id UUID NOT NULL REFERENCES namespaces(id),
    resource_id UUID NOT NULL,
    slug TEXT NOT NULL,
    expected_generation BIGINT NOT NULL CHECK (expected_generation >= 0),
    revision_id BYTEA NOT NULL CHECK (octet_length(revision_id) = 32),
    root_tree_id BYTEA NOT NULL CHECK (octet_length(root_tree_id) = 32),
    manifest_json JSONB NOT NULL,
    snapshot_sha256 BYTEA NOT NULL CHECK (octet_length(snapshot_sha256) = 32),
    snapshot_size BIGINT NOT NULL CHECK (snapshot_size >= 0),
    state TEXT NOT NULL CHECK (state IN ('prepared', 'committed')),
    outcome_json JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);

CREATE UNIQUE INDEX private_import_operations_prepared_locator_idx
    ON private_import_operations (namespace_id, slug)
    WHERE state = 'prepared';

CREATE TABLE private_import_staging (
    user_id UUID NOT NULL,
    operation_id UUID NOT NULL,
    blob_id BYTEA NOT NULL CHECK (octet_length(blob_id) = 32),
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    staging_key TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id, blob_id),
    FOREIGN KEY (user_id, operation_id)
        REFERENCES private_import_operations(user_id, operation_id) ON DELETE CASCADE
);

CREATE INDEX namespace_blob_reachability_usage_idx
    ON namespace_blob_reachability (namespace_id, blob_id);

CREATE INDEX resources_owner_private_idx
    ON resources (owner_namespace_id, kind, visibility, slug, id);
