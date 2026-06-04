CREATE TABLE backup_policies (
    schedule_id UUID PRIMARY KEY REFERENCES schedules(id) ON DELETE CASCADE,
    retention_days INTEGER NOT NULL DEFAULT 30 CHECK (retention_days BETWEEN 1 AND 3650),
    keep_last INTEGER NOT NULL DEFAULT 7 CHECK (keep_last BETWEEN 1 AND 1000),
    rotation_generation TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

