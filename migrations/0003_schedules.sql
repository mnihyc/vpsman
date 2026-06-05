CREATE TABLE schedules (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    name TEXT NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    operation JSONB NOT NULL,
    target_clients TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    target_tags TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    interval_secs BIGINT NOT NULL CHECK (interval_secs BETWEEN 1 AND 31536000),
    next_run_at TIMESTAMPTZ NOT NULL,
    last_run_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX schedules_due_idx ON schedules(enabled, next_run_at);
