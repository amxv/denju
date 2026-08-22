-- Phase 13: team namespaces, durable role/invite state, maintainer-private workspaces,
-- and stable personal-resource transfers into teams.

CREATE TABLE teams (
    namespace_id UUID PRIMARY KEY REFERENCES namespaces(id) ON DELETE CASCADE,
    members_can_publish BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE team_memberships (
    team_namespace_id UUID NOT NULL REFERENCES teams(namespace_id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('owner', 'maintainer', 'member')),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (team_namespace_id, user_id)
);

CREATE UNIQUE INDEX team_memberships_one_owner_idx
    ON team_memberships (team_namespace_id)
    WHERE role = 'owner';

CREATE INDEX team_memberships_user_idx
    ON team_memberships (user_id, team_namespace_id);

CREATE TABLE team_invites (
    id UUID PRIMARY KEY,
    team_namespace_id UUID NOT NULL REFERENCES teams(namespace_id) ON DELETE CASCADE,
    created_by_user_id UUID NOT NULL REFERENCES users(id),
    role TEXT NOT NULL CHECK (role IN ('maintainer', 'member')),
    code_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(code_hash) = 32),
    expires_at TIMESTAMPTZ NOT NULL,
    used_by_user_id UUID REFERENCES users(id),
    used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (used_at IS NULL AND used_by_user_id IS NULL)
        OR (used_at IS NOT NULL AND used_by_user_id IS NOT NULL)
    ),
    CHECK (NOT (used_at IS NOT NULL AND revoked_at IS NOT NULL))
);

CREATE INDEX team_invites_team_active_idx
    ON team_invites (team_namespace_id, expires_at, id)
    WHERE used_at IS NULL AND revoked_at IS NULL;

CREATE TABLE team_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    team_namespace_id UUID REFERENCES teams(namespace_id) ON DELETE CASCADE,
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN (
            'create',
            'invite',
            'invite_revoke',
            'join',
            'member_role',
            'member_remove',
            'settings'
        )
    ),
    outcome_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);

CREATE TABLE resource_transfer_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    destination_namespace_id UUID NOT NULL REFERENCES teams(namespace_id),
    outcome_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);

-- Personal resources previously had exactly one private workspace. Generalize the same
-- durable workspace machinery to one private workspace per editing user. Existing rows are
-- all personal resources because team state did not exist before this migration.
ALTER TABLE skill_private_workspaces
    ADD COLUMN workspace_user_id UUID REFERENCES users(id),
    ADD COLUMN description TEXT;

UPDATE skill_private_workspaces workspace
SET workspace_user_id = owner_user.id,
    description = resource.description
FROM resources resource
JOIN users owner_user ON owner_user.namespace_id = resource.owner_namespace_id
WHERE workspace.resource_id = resource.id;

ALTER TABLE skill_private_workspaces
    ALTER COLUMN workspace_user_id SET NOT NULL,
    ALTER COLUMN description SET NOT NULL;

ALTER TABLE skill_private_workspaces
    DROP CONSTRAINT skill_private_workspaces_pkey;

ALTER TABLE skill_private_workspaces
    ADD PRIMARY KEY (resource_id, workspace_user_id);

CREATE INDEX skill_private_workspaces_resource_idx
    ON skill_private_workspaces (resource_id, workspace_user_id);

-- Conflicts are private-workspace state too. Scope them to the maintainer whose draft raced;
-- different maintainers must never share one unresolved private conflict record.
ALTER TABLE skill_workspace_conflicts
    ADD COLUMN workspace_user_id UUID REFERENCES users(id);

UPDATE skill_workspace_conflicts conflict
SET workspace_user_id = owner_user.id
FROM resources resource
JOIN users owner_user ON owner_user.namespace_id = resource.owner_namespace_id
WHERE conflict.resource_id = resource.id;

ALTER TABLE skill_workspace_conflicts
    ALTER COLUMN workspace_user_id SET NOT NULL;

DROP INDEX skill_workspace_conflicts_unresolved_idx;
DROP INDEX skill_workspace_conflicts_active_pair_idx;
DROP INDEX skill_workspace_conflicts_one_active_idx;

CREATE INDEX skill_workspace_conflicts_unresolved_idx
    ON skill_workspace_conflicts (workspace_user_id, resource_id, created_at, conflict_id)
    WHERE resolved_at IS NULL;

CREATE UNIQUE INDEX skill_workspace_conflicts_active_pair_idx
    ON skill_workspace_conflicts (workspace_user_id, resource_id, head_a_revision_id, head_b_revision_id)
    WHERE resolved_at IS NULL;

CREATE UNIQUE INDEX skill_workspace_conflicts_one_active_idx
    ON skill_workspace_conflicts (workspace_user_id, resource_id)
    WHERE resolved_at IS NULL;
