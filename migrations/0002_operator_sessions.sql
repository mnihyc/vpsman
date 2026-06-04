CREATE TABLE operator_sessions (
    id UUID PRIMARY KEY,
    operator_id UUID NOT NULL REFERENCES operators(id) ON DELETE CASCADE,
    access_token_hash TEXT NOT NULL UNIQUE,
    refresh_token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    refresh_expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX operator_sessions_operator_id_idx ON operator_sessions(operator_id);
CREATE INDEX operator_sessions_access_token_hash_idx ON operator_sessions(access_token_hash);
CREATE INDEX operator_sessions_refresh_token_hash_idx ON operator_sessions(refresh_token_hash);
