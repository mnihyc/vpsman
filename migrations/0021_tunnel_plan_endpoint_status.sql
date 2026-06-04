ALTER TABLE tunnel_plans
    ADD COLUMN left_status TEXT NOT NULL DEFAULT 'planned',
    ADD COLUMN right_status TEXT NOT NULL DEFAULT 'planned',
    ADD COLUMN last_apply_job_id UUID REFERENCES jobs(id),
    ADD COLUMN last_rollback_job_id UUID REFERENCES jobs(id),
    ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE INDEX tunnel_plans_status_idx ON tunnel_plans(status, updated_at DESC);
