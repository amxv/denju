CREATE TABLE author_principals (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('installation', 'user', 'deleted_user')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE installations (
    id UUID PRIMARY KEY,
    author_principal_id UUID NOT NULL UNIQUE REFERENCES author_principals(id),
    credential_hash BYTEA NOT NULL CHECK (octet_length(credential_hash) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX installations_credential_hash_idx
    ON installations (credential_hash);

CREATE TABLE bootstrap_operations (
    operation_id UUID PRIMARY KEY,
    request_hash BYTEA NOT NULL CHECK (octet_length(request_hash) = 32),
    installation_id UUID NOT NULL REFERENCES installations(id),
    author_principal_id UUID NOT NULL REFERENCES author_principals(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
