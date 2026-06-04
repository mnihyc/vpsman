ALTER TABLE agent_update_rollouts
    ADD COLUMN automation_status TEXT NOT NULL DEFAULT 'unreconciled',
    ADD COLUMN automation_next_action TEXT,
    ADD COLUMN automation_blocker TEXT,
    ADD COLUMN automation_targets TEXT[] NOT NULL DEFAULT '{}',
    ADD COLUMN automation_updated_at TIMESTAMPTZ;

CREATE INDEX agent_update_rollouts_automation_idx
    ON agent_update_rollouts (automation_status, automation_updated_at DESC, id DESC);
