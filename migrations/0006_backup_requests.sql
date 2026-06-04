CREATE TABLE backup_requests (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    paths TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    include_config BOOLEAN NOT NULL DEFAULT FALSE,
    status TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    proof_scope TEXT NOT NULL,
    proof_command_id UUID,
    proof_expires_unix BIGINT,
    artifact_id UUID REFERENCES backup_artifacts(id),
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX backup_requests_client_created_idx
    ON backup_requests (client_id, created_at DESC, id DESC);

CREATE INDEX backup_requests_status_created_idx
    ON backup_requests (status, created_at DESC, id DESC);
