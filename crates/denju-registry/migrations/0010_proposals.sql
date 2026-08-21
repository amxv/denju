CREATE TABLE skill_proposals (
    id UUID PRIMARY KEY,
    proposer_user_id UUID NOT NULL REFERENCES users(id),
    source_resource_id UUID NOT NULL REFERENCES resources(id),
    target_resource_id UUID NOT NULL REFERENCES resources(id),
    generation BIGINT NOT NULL CHECK (generation > 0),
    state TEXT NOT NULL CHECK (state IN ('open', 'accepted', 'rejected', 'withdrawn')),
    message TEXT CHECK (message IS NULL OR char_length(message) <= 500),
    closed_revision_id BYTEA REFERENCES revisions(revision_id),
    closed_source_generation BIGINT CHECK (closed_source_generation IS NULL OR closed_source_generation > 0),
    closed_by_user_id UUID REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at TIMESTAMPTZ,
    CHECK (source_resource_id <> target_resource_id),
    CHECK (
        (state = 'open' AND closed_revision_id IS NULL AND closed_source_generation IS NULL AND closed_by_user_id IS NULL AND closed_at IS NULL)
        OR
        (state <> 'open' AND closed_revision_id IS NOT NULL AND closed_source_generation IS NOT NULL AND closed_at IS NOT NULL)
    )
);

CREATE UNIQUE INDEX skill_proposals_one_open_per_fork_idx
    ON skill_proposals (source_resource_id, target_resource_id)
    WHERE state = 'open';

CREATE INDEX skill_proposals_proposer_idx
    ON skill_proposals (proposer_user_id, created_at DESC, id);

CREATE INDEX skill_proposals_target_idx
    ON skill_proposals (target_resource_id, created_at DESC, id);

CREATE TABLE skill_proposal_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    action TEXT NOT NULL CHECK (action IN ('create', 'accept', 'reject', 'withdraw')),
    proposal_id UUID NOT NULL REFERENCES skill_proposals(id) ON DELETE CASCADE,
    outcome_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);
