CREATE TABLE agent_update_rollout_delegated_proofs (
    id UUID PRIMARY KEY,
    rollout_id UUID NOT NULL REFERENCES agent_update_rollouts(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    action TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    rollback_sha256_hex TEXT,
    envelope JSONB NOT NULL,
    proof_expires_unix BIGINT NOT NULL,
    status TEXT NOT NULL,
    dispatch_job_id UUID REFERENCES jobs(id),
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (rollout_id, client_id, action, payload_hash)
);

CREATE INDEX agent_update_rollout_delegated_ready_idx
    ON agent_update_rollout_delegated_proofs (status, proof_expires_unix, rollout_id, client_id);

CREATE INDEX agent_update_rollout_delegated_dispatch_idx
    ON agent_update_rollout_delegated_proofs (dispatch_job_id, status);
