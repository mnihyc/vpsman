CREATE TABLE telemetry_ingest_watermarks (
    client_id TEXT PRIMARY KEY REFERENCES clients(id) ON DELETE CASCADE,
    process_incarnation_id UUID NOT NULL,
    telemetry_seq BIGINT NOT NULL CHECK (telemetry_seq > 0),
    reported_observed_unix BIGINT NOT NULL CHECK (reported_observed_unix >= 0),
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX telemetry_rollups_client_latest_idx
    ON telemetry_rollups (client_id, bucket_start DESC, bucket_secs);

CREATE INDEX telemetry_network_rates_client_latest_idx
    ON telemetry_network_rates (client_id, interface, bucket_start DESC, bucket_secs);

CREATE INDEX telemetry_rollups_retention_idx
    ON telemetry_rollups (bucket_start);

CREATE INDEX telemetry_network_rates_retention_idx
    ON telemetry_network_rates (bucket_start);
