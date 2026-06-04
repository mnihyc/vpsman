CREATE TABLE client_key_revocations (
    id UUID PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    public_key_sha256_hex TEXT NOT NULL,
    reason TEXT,
    revoked_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (client_id, public_key_sha256_hex),
    CONSTRAINT client_key_revocations_sha256_hex_valid
        CHECK (public_key_sha256_hex ~ '^[0-9a-f]{64}$')
);

CREATE INDEX client_key_revocations_client_created_idx
    ON client_key_revocations (client_id, created_at DESC);
