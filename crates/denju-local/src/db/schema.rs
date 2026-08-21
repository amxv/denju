pub(super) const MIGRATION_V1: &str = r#"
CREATE TABLE IF NOT EXISTS installation (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    registry_origin TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    author_principal_id TEXT NOT NULL,
    credential_backend TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS harness_config (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    codex_root TEXT NOT NULL,
    claude_root TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS operation_journal (
    operation_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('planned', 'staged', 'verified', 'switched', 'complete')),
    payload_json TEXT NOT NULL,
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS service_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    kind TEXT NOT NULL,
    persistent INTEGER NOT NULL CHECK (persistent IN (0, 1)),
    running INTEGER NOT NULL CHECK (running IN (0, 1)),
    detail TEXT
);

CREATE TABLE IF NOT EXISTS work_leases (
    resource_key TEXT PRIMARY KEY,
    holder TEXT NOT NULL,
    expires_at_unix_ms INTEGER NOT NULL
);

PRAGMA user_version = 1;
"#;

pub(super) const MIGRATION_V2: &str = r#"
CREATE TABLE IF NOT EXISTS subscriptions (
    resource_id TEXT PRIMARY KEY,
    locator TEXT NOT NULL UNIQUE,
    owner TEXT NOT NULL,
    skill_name TEXT NOT NULL,
    resource_generation INTEGER NOT NULL,
    release_version INTEGER NOT NULL,
    desired_revision_id TEXT NOT NULL,
    harness_name TEXT,
    materialized_revision_id TEXT,
    updated_at_unix_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS subscriptions_skill_name_idx
    ON subscriptions (skill_name, resource_id);

PRAGMA user_version = 2;
"#;

pub(super) const MIGRATION_V3: &str = r#"
CREATE TABLE IF NOT EXISTS identity_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    user_id TEXT NOT NULL,
    namespace_id TEXT NOT NULL,
    username TEXT NOT NULL,
    session_id TEXT,
    session_backend TEXT,
    updated_at_unix_ms INTEGER NOT NULL
);

PRAGMA user_version = 3;
"#;

pub(super) const MIGRATION_V4: &str = r#"
CREATE TABLE IF NOT EXISTS owned_skills (
    resource_id TEXT PRIMARY KEY,
    locator TEXT NOT NULL UNIQUE,
    owner TEXT NOT NULL,
    skill_name TEXT NOT NULL,
    resource_generation INTEGER NOT NULL,
    desired_revision_id TEXT NOT NULL,
    harness_name TEXT,
    materialized_revision_id TEXT,
    updated_at_unix_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS owned_skills_skill_name_idx
    ON owned_skills (skill_name, resource_id);

PRAGMA user_version = 4;
"#;

pub(super) const MIGRATION_V5: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE identity_state ADD COLUMN author_principal_id TEXT;

CREATE TABLE IF NOT EXISTS workspace_state (
    resource_id TEXT PRIMARY KEY REFERENCES owned_skills(resource_id) ON DELETE CASCADE,
    base_generation INTEGER NOT NULL CHECK (base_generation > 0),
    base_revision_id TEXT NOT NULL,
    local_head_revision_id TEXT NOT NULL,
    valid_root_tree_id TEXT NOT NULL,
    working_generation_path TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('clean', 'queued', 'paused_validation', 'pending_rename', 'conflict', 'quota')),
    error_message TEXT,
    pending_rename TEXT,
    updated_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS workspace_file_index (
    resource_id TEXT NOT NULL REFERENCES owned_skills(resource_id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('file', 'directory', 'symlink')),
    size_bytes INTEGER,
    mtime_ns INTEGER,
    executable INTEGER CHECK (executable IS NULL OR executable IN (0, 1)),
    blob_id TEXT,
    symlink_target TEXT,
    PRIMARY KEY (resource_id, path),
    CHECK (
        (kind = 'file' AND size_bytes IS NOT NULL AND mtime_ns IS NOT NULL AND executable IS NOT NULL AND blob_id IS NOT NULL AND symlink_target IS NULL)
        OR (kind = 'directory' AND size_bytes IS NULL AND mtime_ns IS NULL AND executable IS NULL AND blob_id IS NULL AND symlink_target IS NULL)
        OR (kind = 'symlink' AND size_bytes IS NULL AND mtime_ns IS NULL AND executable IS NULL AND blob_id IS NULL AND symlink_target IS NOT NULL)
    )
);

CREATE TABLE IF NOT EXISTS local_revisions (
    operation_id TEXT PRIMARY KEY,
    resource_id TEXT NOT NULL REFERENCES owned_skills(resource_id) ON DELETE CASCADE,
    revision_id TEXT NOT NULL UNIQUE,
    parent_revision_id TEXT NOT NULL,
    expected_generation INTEGER NOT NULL CHECK (expected_generation > 0),
    root_tree_id TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('queued', 'synced')),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS local_revisions_queue_idx
    ON local_revisions (resource_id, state, expected_generation, created_at_unix_ms);

CREATE TABLE IF NOT EXISTS derived_projection_state (
    resource_id TEXT PRIMARY KEY REFERENCES owned_skills(resource_id) ON DELETE CASCADE,
    harness_name TEXT NOT NULL,
    baseline_root_tree_id TEXT NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL
);
PRAGMA user_version = 5;
COMMIT;
"#;

pub(super) const MIGRATION_V6: &str = r#"
ALTER TABLE subscriptions ADD COLUMN retain_on_delete INTEGER NOT NULL DEFAULT 0 CHECK (retain_on_delete IN (0, 1));
ALTER TABLE subscriptions ADD COLUMN retained_after_delete INTEGER NOT NULL DEFAULT 0 CHECK (retained_after_delete IN (0, 1));
PRAGMA user_version = 6;
"#;

pub(super) const MIGRATION_V7: &str = r#"
BEGIN IMMEDIATE;

-- Preserve the existing Phase-8 local revision insert shape. parent_revision_id remains the CAS
-- expected head and first ancestry parent; merge revisions add only the optional second parent.
ALTER TABLE local_revisions ADD COLUMN merge_parent_revision_id TEXT;

CREATE TABLE workspace_content_conflicts (
    conflict_id TEXT PRIMARY KEY,
    resource_id TEXT NOT NULL UNIQUE REFERENCES owned_skills(resource_id) ON DELETE CASCADE,
    base_revision_id TEXT NOT NULL,
    head_a_revision_id TEXT NOT NULL,
    head_b_revision_id TEXT NOT NULL,
    active_revision_id TEXT NOT NULL,
    remote_generation INTEGER NOT NULL CHECK (remote_generation > 0),
    working_root_tree_id TEXT NOT NULL,
    resolution_required INTEGER NOT NULL DEFAULT 0 CHECK (resolution_required IN (0, 1)),
    conflict_paths_json TEXT NOT NULL CHECK (json_valid(conflict_paths_json)),
    created_at_unix_ms INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    CHECK (head_a_revision_id <> head_b_revision_id)
);

PRAGMA user_version = 7;
COMMIT;
"#;
