-- Phase 14: enforced team pack requirements and explicit ownership succession.

CREATE TABLE team_pack_assignments (
    team_namespace_id UUID NOT NULL REFERENCES teams(namespace_id) ON DELETE CASCADE,
    pack_resource_id UUID NOT NULL REFERENCES resources(id),
    assigned_by_user_id UUID NOT NULL REFERENCES users(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (team_namespace_id, pack_resource_id)
);

CREATE INDEX team_pack_assignments_pack_idx
    ON team_pack_assignments (pack_resource_id, team_namespace_id);

CREATE TABLE team_owner_transfers (
    id UUID PRIMARY KEY,
    team_namespace_id UUID NOT NULL REFERENCES teams(namespace_id) ON DELETE CASCADE,
    from_user_id UUID NOT NULL REFERENCES users(id),
    to_user_id UUID NOT NULL REFERENCES users(id),
    code_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(code_hash) = 32),
    state TEXT NOT NULL CHECK (state IN ('pending', 'accepted', 'cancelled')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    accepted_at TIMESTAMPTZ,
    CHECK (from_user_id <> to_user_id),
    CHECK ((state = 'accepted') = (accepted_at IS NOT NULL))
);

CREATE UNIQUE INDEX team_owner_transfers_one_pending_idx
    ON team_owner_transfers (team_namespace_id)
    WHERE state = 'pending';

ALTER TABLE team_operations
    ADD COLUMN secret_verifier TEXT;

ALTER TABLE team_operations
    DROP CONSTRAINT team_operations_operation_kind_check;

ALTER TABLE team_operations
    ADD CONSTRAINT team_operations_operation_kind_check CHECK (
        operation_kind IN (
            'create',
            'invite',
            'invite_revoke',
            'join',
            'member_role',
            'member_remove',
            'settings',
            'pack_assign',
            'pack_unassign',
            'leave',
            'owner_transfer',
            'owner_transfer_accept',
            'delete'
        )
    );

-- Team namespaces are deliberately reusable after deletion. Historical operation journals that
-- are meaningful only while that namespace exists must therefore disappear with the namespace,
-- rather than pinning the teams/namespaces rows forever through restrictive foreign keys.
ALTER TABLE resource_transfer_operations
    DROP CONSTRAINT resource_transfer_operations_destination_namespace_id_fkey;

ALTER TABLE resource_transfer_operations
    ADD CONSTRAINT resource_transfer_operations_destination_namespace_id_fkey
    FOREIGN KEY (destination_namespace_id) REFERENCES teams(namespace_id) ON DELETE CASCADE;

-- Team maintainer saves use the same private revision operation table as personal workspaces.
-- The resource history itself is retained through resource tombstones; the mutation replay row is
-- namespace-scoped and must not prevent the team namespace from being deleted/reused.
ALTER TABLE private_revision_operations
    DROP CONSTRAINT private_revision_operations_namespace_id_fkey;

ALTER TABLE private_revision_operations
    ADD CONSTRAINT private_revision_operations_namespace_id_fkey
    FOREIGN KEY (namespace_id) REFERENCES namespaces(id) ON DELETE CASCADE;

-- Imports can target a team namespace directly (`denju import ... --to @team`). Their replay and
-- staging rows are authority-operation state, not immutable resource history, so they must not
-- pin a deleted team namespace either. Staging already cascades through the import operation.
ALTER TABLE private_import_operations
    DROP CONSTRAINT private_import_operations_namespace_id_fkey;

ALTER TABLE private_import_operations
    ADD CONSTRAINT private_import_operations_namespace_id_fkey
    FOREIGN KEY (namespace_id) REFERENCES namespaces(id) ON DELETE CASCADE;
