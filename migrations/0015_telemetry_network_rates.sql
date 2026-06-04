CREATE TABLE telemetry_network_rates (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    interface TEXT NOT NULL,
    bucket_start TIMESTAMPTZ NOT NULL,
    bucket_secs INTEGER NOT NULL,
    sample_count INTEGER NOT NULL,
    rx_bytes_delta BIGINT NOT NULL,
    tx_bytes_delta BIGINT NOT NULL,
    rx_bps_avg DOUBLE PRECISION NOT NULL,
    tx_bps_avg DOUBLE PRECISION NOT NULL,
    first_observed_at TIMESTAMPTZ NOT NULL,
    latest_observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, interface, bucket_secs, bucket_start)
);

CREATE INDEX telemetry_network_rates_latest_idx
    ON telemetry_network_rates (bucket_secs, bucket_start DESC, client_id, interface);
