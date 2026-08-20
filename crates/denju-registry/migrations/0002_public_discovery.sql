CREATE TABLE namespaces (
    id UUID PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('user', 'team')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (slug <> '' AND slug = lower(slug))
);

CREATE TABLE resources (
    id UUID PRIMARY KEY,
    owner_namespace_id UUID NOT NULL REFERENCES namespaces(id),
    slug TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('skill', 'pack')),
    visibility TEXT NOT NULL CHECK (visibility IN ('public', 'private')),
    description TEXT NOT NULL,
    generation BIGINT NOT NULL CHECK (generation > 0),
    latest_release_version BIGINT NOT NULL CHECK (latest_release_version > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (owner_namespace_id, kind, slug)
);

CREATE INDEX resources_public_skill_search_idx
    ON resources (visibility, kind, owner_namespace_id, slug, id);

CREATE TABLE skill_releases (
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    version BIGINT NOT NULL CHECK (version > 0),
    revision_id BYTEA NOT NULL CHECK (octet_length(revision_id) = 32),
    root_tree_id BYTEA NOT NULL CHECK (octet_length(root_tree_id) = 32),
    manifest_json JSONB NOT NULL,
    snapshot_key TEXT NOT NULL,
    snapshot_sha256 BYTEA NOT NULL CHECK (octet_length(snapshot_sha256) = 32),
    snapshot_size BIGINT NOT NULL CHECK (snapshot_size >= 0),
    author_principal_id UUID NOT NULL REFERENCES author_principals(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (resource_id, version),
    UNIQUE (resource_id, revision_id)
);

CREATE TABLE installation_subscriptions (
    installation_id UUID NOT NULL REFERENCES installations(id) ON DELETE CASCADE,
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (installation_id, resource_id)
);

CREATE TABLE subscription_operations (
    installation_id UUID NOT NULL REFERENCES installations(id) ON DELETE CASCADE,
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    action TEXT NOT NULL CHECK (action IN ('subscribe', 'unsubscribe')),
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    subscribed BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (installation_id, operation_id)
);
