CREATE TABLE users (
    id UUID PRIMARY KEY,
    namespace_id UUID UNIQUE REFERENCES namespaces(id),
    author_principal_id UUID NOT NULL UNIQUE REFERENCES author_principals(id),
    password_hash TEXT,
    recovery_secret_hash BYTEA CHECK (recovery_secret_hash IS NULL OR octet_length(recovery_secret_hash) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CHECK (
        (deleted_at IS NULL AND namespace_id IS NOT NULL AND password_hash IS NOT NULL AND recovery_secret_hash IS NOT NULL)
        OR
        (deleted_at IS NOT NULL AND namespace_id IS NULL AND password_hash IS NULL AND recovery_secret_hash IS NULL)
    )
);

ALTER TABLE installations
    ADD COLUMN user_id UUID REFERENCES users(id),
    ADD COLUMN revoked_at TIMESTAMPTZ;

CREATE INDEX installations_user_id_idx ON installations (user_id) WHERE user_id IS NOT NULL;

CREATE TABLE author_principal_users (
    author_principal_id UUID PRIMARY KEY REFERENCES author_principals(id),
    user_id UUID NOT NULL REFERENCES users(id),
    linked_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE account_subscriptions (
    user_id UUID NOT NULL REFERENCES users(id),
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, resource_id)
);

CREATE TABLE account_subscription_operations (
    user_id UUID NOT NULL REFERENCES users(id),
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    action TEXT NOT NULL CHECK (action IN ('subscribe', 'unsubscribe')),
    resource_id UUID NOT NULL REFERENCES resources(id) ON DELETE CASCADE,
    subscribed BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, operation_id)
);

CREATE TABLE sessions (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id),
    installation_id UUID NOT NULL REFERENCES installations(id),
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    device_name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);

CREATE INDEX sessions_user_active_idx ON sessions (user_id, created_at, id) WHERE revoked_at IS NULL;

CREATE TABLE automation_tokens (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id),
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    scopes JSONB NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);

CREATE INDEX automation_tokens_user_active_idx
    ON automation_tokens (user_id, expires_at, id)
    WHERE revoked_at IS NULL;

CREATE TABLE identity_operations (
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('installation', 'user')),
    actor_id UUID NOT NULL,
    operation_id UUID NOT NULL,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    secret_verifier TEXT,
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN (
            'claim',
            'login',
            'recovery_reset',
            'identity_backup',
            'device_revoke',
            'token_create',
            'token_revoke',
            'account_delete'
        )
    ),
    outcome_json JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (actor_kind, actor_id, operation_id)
);
