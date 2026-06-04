ALTER TABLE jobs
    ADD COLUMN operation JSONB,
    ADD COLUMN source_schedule_id UUID REFERENCES schedules(id);

CREATE INDEX jobs_scheduled_approval_idx
    ON jobs(status, source_schedule_id)
    WHERE source_schedule_id IS NOT NULL;
