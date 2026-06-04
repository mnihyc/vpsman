CREATE TABLE agent_update_rollouts (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL UNIQUE REFERENCES jobs(id) ON DELETE CASCADE,
    actor_id UUID REFERENCES operators(id),
    status TEXT NOT NULL,
    artifact_sha256_hex TEXT NOT NULL,
    artifact_signature_provided BOOLEAN NOT NULL DEFAULT FALSE,
    artifact_signing_key_sha256_hex TEXT,
    target_count INTEGER NOT NULL,
    canary_count INTEGER NOT NULL DEFAULT 0,
    activation_policy TEXT NOT NULL,
    heartbeat_timeout_secs INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE agent_update_rollout_targets (
    rollout_id UUID NOT NULL REFERENCES agent_update_rollouts(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    exit_code INTEGER,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (rollout_id, client_id)
);

CREATE INDEX agent_update_rollouts_status_created_idx
    ON agent_update_rollouts (status, created_at DESC, id DESC);

CREATE INDEX agent_update_rollout_targets_client_idx
    ON agent_update_rollout_targets (client_id, updated_at DESC);
