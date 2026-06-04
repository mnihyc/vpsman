CREATE TABLE restore_plans (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    source_backup_request_id UUID NOT NULL REFERENCES backup_requests(id) ON DELETE CASCADE,
    source_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    target_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    paths TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    include_config BOOLEAN NOT NULL DEFAULT FALSE,
    destination_root TEXT,
    status TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    proof_scope TEXT NOT NULL,
    proof_command_id UUID,
    proof_expires_unix BIGINT,
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX restore_plans_source_created_idx
    ON restore_plans (source_backup_request_id, created_at DESC, id DESC);

CREATE INDEX restore_plans_target_created_idx
    ON restore_plans (target_client_id, created_at DESC, id DESC);

CREATE INDEX restore_plans_status_created_idx
    ON restore_plans (status, created_at DESC, id DESC);
