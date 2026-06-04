ALTER TABLE backup_requests
    ADD COLUMN source_job_id UUID REFERENCES jobs(id),
    ADD COLUMN source_schedule_id UUID REFERENCES schedules(id);

CREATE INDEX backup_requests_source_schedule_created_idx
    ON backup_requests (source_schedule_id, client_id, created_at DESC, id DESC)
    WHERE source_schedule_id IS NOT NULL;
