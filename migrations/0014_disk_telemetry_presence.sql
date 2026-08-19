-- Disk telemetry before the versioned persistent-filesystem contract mixed
-- mount semantics and had no way to distinguish collection failure from an
-- explicit zero-capacity inventory. Keep raw payloads for forensics, but do
-- not carry those unversioned values into authoritative rollups.
ALTER TABLE telemetry_samples
    ALTER COLUMN disk_total_bytes DROP NOT NULL,
    ALTER COLUMN disk_available_bytes DROP NOT NULL;

ALTER TABLE telemetry_rollups
    ADD COLUMN disk_sample_count INTEGER NOT NULL DEFAULT 0,
    ADD CHECK (disk_sample_count BETWEEN 0 AND sample_count);

ALTER TABLE telemetry_resource_latest
    ADD COLUMN disk_sample_count INTEGER NOT NULL DEFAULT 0,
    ADD CHECK (disk_sample_count BETWEEN 0 AND sample_count);
