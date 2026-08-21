-- Latest fleet snapshots select at most two physical observations per host
-- stream. Retained tiers can overlap after a guarded promotion conflict, so
-- bucket start order is not an exact substitute for observation time.
CREATE INDEX telemetry_network_rates_client_effective_idx
    ON telemetry_network_rates (
        client_id,
        interface,
        latest_observed_at DESC,
        bucket_start DESC
    )
    INCLUDE (bucket_secs);
