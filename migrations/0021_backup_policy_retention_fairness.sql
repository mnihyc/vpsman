ALTER TABLE backup_policies
    ADD COLUMN retention_scanned_at TIMESTAMPTZ;

CREATE INDEX backup_policies_retention_scan_idx
    ON backup_policies (retention_scanned_at ASC NULLS FIRST, schedule_id ASC);
