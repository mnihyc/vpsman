-- Unbounded traffic cycles aggregate the first/last endpoint of each
-- independently tracked counter epoch without scanning retained minute rows.
CREATE INDEX traffic_counter_samples_rx_epoch_lookup_idx
    ON traffic_counter_samples (
        client_id,
        source_kind,
        interface,
        rx_counter_epoch,
        observed_at
    ) INCLUDE (rx_bytes, sample_source);

CREATE INDEX traffic_counter_samples_tx_epoch_lookup_idx
    ON traffic_counter_samples (
        client_id,
        source_kind,
        interface,
        tx_counter_epoch,
        observed_at
    ) INCLUDE (tx_bytes, sample_source);
