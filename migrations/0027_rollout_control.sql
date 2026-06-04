ALTER TABLE agent_update_rollouts
    ADD COLUMN automation_paused BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN automation_pause_reason TEXT,
    ADD COLUMN automation_health_gate TEXT NOT NULL DEFAULT 'heartbeat_verified',
    ADD COLUMN automation_lease_owner TEXT,
    ADD COLUMN automation_lease_expires_at TIMESTAMPTZ;

CREATE INDEX agent_update_rollouts_automation_lease_idx
    ON agent_update_rollouts (automation_lease_expires_at, automation_paused, id);
