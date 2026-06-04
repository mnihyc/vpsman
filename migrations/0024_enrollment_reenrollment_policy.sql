ALTER TABLE enrollment_tokens
    ADD COLUMN purpose TEXT NOT NULL DEFAULT 'provision',
    ADD COLUMN allowed_client_id TEXT REFERENCES clients(id),
    ADD COLUMN requires_existing_client BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN preserve_existing_assignments BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN expected_old_public_key_sha256_hex TEXT;

CREATE INDEX enrollment_tokens_allowed_client_idx
    ON enrollment_tokens (allowed_client_id, created_at DESC);
