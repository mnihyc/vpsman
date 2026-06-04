CREATE TABLE telemetry_rollups (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    bucket_start TIMESTAMPTZ NOT NULL,
    bucket_secs INTEGER NOT NULL,
    sample_count INTEGER NOT NULL,
    cpu_load_1_avg DOUBLE PRECISION NOT NULL,
    cpu_load_1_max DOUBLE PRECISION NOT NULL,
    memory_total_bytes_max BIGINT NOT NULL,
    memory_available_bytes_avg BIGINT NOT NULL,
    memory_available_bytes_min BIGINT NOT NULL,
    latest_observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, bucket_secs, bucket_start)
);

CREATE INDEX telemetry_rollups_latest_idx
    ON telemetry_rollups (bucket_secs, bucket_start DESC, client_id);
