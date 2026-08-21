-- Phase 9: preserve divergent private heads, separate CAS authority from immutable ancestry,
-- and make unresolved content conflicts durable without moving the current workspace ref.

ALTER TABLE private_revision_operations
    DROP CONSTRAINT private_revision_operations_state_check;

ALTER TABLE private_revision_operations
    RENAME COLUMN parent_revision_id TO expected_head_revision_id;

UPDATE private_revision_operations
SET state = 'advanced',
    outcome_json = CASE
        WHEN outcome_json IS NULL THEN NULL
        ELSE jsonb_build_object('state', 'advanced', 'revision', outcome_json)
    END
WHERE state = 'committed';

ALTER TABLE private_revision_operations
    ADD CONSTRAINT private_revision_operations_state_check
        CHECK (state IN ('prepared', 'advanced', 'diverged'));

CREATE TABLE private_revision_operation_parents (
    user_id UUID NOT NULL,
    operation_id UUID NOT NULL,
    ordinal SMALLINT NOT NULL CHECK (ordinal IN (0, 1)),
    parent_revision_id BYTEA NOT NULL CHECK (octet_length(parent_revision_id) = 32),
    PRIMARY KEY (user_id, operation_id, ordinal),
    UNIQUE (user_id, operation_id, parent_revision_id),
    FOREIGN KEY (user_id, operation_id)
        REFERENCES private_revision_operations(user_id, operation_id) ON DELETE CASCADE
);

INSERT INTO private_revision_operation_parents
    (user_id, operation_id, ordinal, parent_revision_id)
SELECT user_id, operation_id, 0, expected_head_revision_id
FROM private_revision_operations;

CREATE TABLE skill_workspace_conflicts (
    conflict_id UUID PRIMARY KEY,
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    base_revision_id BYTEA NOT NULL REFERENCES revisions(revision_id),
    head_a_revision_id BYTEA NOT NULL REFERENCES revisions(revision_id),
    head_b_revision_id BYTEA NOT NULL REFERENCES revisions(revision_id),
    active_revision_id BYTEA NOT NULL REFERENCES revisions(revision_id),
    detected_generation BIGINT NOT NULL CHECK (detected_generation > 0),
    resolution_revision_id BYTEA REFERENCES revisions(revision_id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    CHECK (head_a_revision_id <> head_b_revision_id),
    CHECK (
        (resolved_at IS NULL AND resolution_revision_id IS NULL)
        OR (resolved_at IS NOT NULL AND resolution_revision_id IS NOT NULL)
    )
);

CREATE INDEX skill_workspace_conflicts_unresolved_idx
    ON skill_workspace_conflicts (resource_id, created_at, conflict_id)
    WHERE resolved_at IS NULL;

CREATE UNIQUE INDEX skill_workspace_conflicts_active_pair_idx
    ON skill_workspace_conflicts (resource_id, head_a_revision_id, head_b_revision_id)
    WHERE resolved_at IS NULL;

CREATE UNIQUE INDEX skill_workspace_conflicts_one_active_idx
    ON skill_workspace_conflicts (resource_id)
    WHERE resolved_at IS NULL;

DROP INDEX private_revision_operations_resource_idx;
CREATE INDEX private_revision_operations_resource_idx
    ON private_revision_operations (resource_id, state, expected_generation, created_at);
