ALTER TABLE telemetry_rollups
    ADD COLUMN disk_total_bytes_max BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN disk_available_bytes_avg BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN disk_available_bytes_min BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN network_rx_bytes_max BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN network_tx_bytes_max BIGINT NOT NULL DEFAULT 0;
