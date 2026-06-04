CREATE TABLE fleet_alert_states (
    alert_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    muted_until_unix BIGINT,
    escalation_level INTEGER NOT NULL DEFAULT 0,
    reason TEXT,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (state IN ('open', 'acknowledged', 'muted', 'escalated')),
    CHECK (escalation_level >= 0),
    CHECK (
        (state = 'muted' AND muted_until_unix IS NOT NULL)
        OR state <> 'muted'
    )
);

CREATE INDEX fleet_alert_states_state_idx
    ON fleet_alert_states (state, updated_at DESC);
