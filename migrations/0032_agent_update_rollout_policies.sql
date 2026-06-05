CREATE TABLE agent_update_rollout_policies (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    scope_kind TEXT NOT NULL,
    scope_value TEXT,
    channel TEXT,
    canary_count INTEGER,
    automation_health_gate TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    notes TEXT,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (scope_kind IN ('global', 'tag', 'provider')),
    CHECK (
        (scope_kind = 'global' AND scope_value IS NULL)
        OR (scope_kind <> 'global' AND scope_value IS NOT NULL)
    ),
    CHECK (canary_count IS NULL OR canary_count BETWEEN 0 AND 10000),
    CHECK (
        automation_health_gate IS NULL
        OR automation_health_gate IN ('heartbeat_verified', 'manual_after_canary', 'manual_only')
    )
);

CREATE INDEX agent_update_rollout_policies_match_idx
    ON agent_update_rollout_policies (
        enabled,
        channel,
        scope_kind,
        scope_value,
        priority DESC,
        updated_at DESC
    );

ALTER TABLE agent_update_rollouts
    ADD COLUMN rollout_policy_id UUID REFERENCES agent_update_rollout_policies(id),
    ADD COLUMN rollout_policy_name TEXT;
