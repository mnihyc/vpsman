CREATE TABLE enrollment_tokens (
    id UUID PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    token_prefix TEXT NOT NULL,
    created_by UUID REFERENCES operators(id),
    default_tags JSONB NOT NULL DEFAULT '[]',
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    used_by_client_id TEXT REFERENCES clients(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX enrollment_tokens_unused_expires_idx
ON enrollment_tokens (expires_at)
WHERE used_at IS NULL;
