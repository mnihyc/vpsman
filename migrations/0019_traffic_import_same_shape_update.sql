-- A normal vnStat reimport keeps the same dense minute keys and changes only
-- imported counters/lineage.  The raw UPDATE still needs PostgreSQL's normal
-- MVCC/index work, but rebuilding the already-valid hourly ledger for a
-- source-class-preserving update is unnecessary.  The application opts into
-- this path only after its locked dense-shape proof; this trigger remains
-- fail-closed and falls through to the ordinary refresh for every mismatch.
CREATE OR REPLACE FUNCTION refresh_traffic_counter_hourly_usage_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
    observed_values TIMESTAMPTZ[];
    changed_count BIGINT;
    changed_stream_count BIGINT;
    updated_stream_count BIGINT;
    lineage_only BOOLEAN := FALSE;
BEGIN
    IF current_setting('vpsman.traffic_import_same_shape_update', true) = 'on'
       AND EXISTS (SELECT 1 FROM new_traffic_counter_samples) THEN
        -- The raw UPDATE may rotate a UUID suffix, but hourly accounting only
        -- depends on these fields.  A full outer join also rejects a changed
        -- primary key, a missing row, a changed counter/epoch, an inbound
        -- promotion change, or an import/live class change.  Any such case
        -- falls through to the existing exact refresh below.
        SELECT NOT EXISTS (
            SELECT 1
            FROM old_traffic_counter_samples old_sample
            FULL OUTER JOIN new_traffic_counter_samples new_sample
              ON new_sample.client_id = old_sample.client_id
             AND new_sample.source_kind = old_sample.source_kind
             AND new_sample.interface = old_sample.interface
             AND new_sample.observed_at = old_sample.observed_at
            WHERE old_sample.client_id IS NULL
               OR new_sample.client_id IS NULL
               OR NOT starts_with(old_sample.sample_source, 'vnstat_import:')
               OR NOT starts_with(new_sample.sample_source, 'vnstat_import:')
               OR old_sample.rx_bytes IS DISTINCT FROM new_sample.rx_bytes
               OR old_sample.tx_bytes IS DISTINCT FROM new_sample.tx_bytes
               OR old_sample.rx_counter_epoch IS DISTINCT FROM new_sample.rx_counter_epoch
               OR old_sample.tx_counter_epoch IS DISTINCT FROM new_sample.tx_counter_epoch
               OR old_sample.inbound_promoted IS DISTINCT FROM new_sample.inbound_promoted
               OR starts_with(old_sample.sample_source, 'vnstat_import:')
                    IS DISTINCT FROM starts_with(new_sample.sample_source, 'vnstat_import:')
        )
        INTO lineage_only;

        IF lineage_only THEN
            -- A dirty or missing marker must never be declared clean merely
            -- because the raw accounting projection is unchanged.  The block
            -- is a PL/pgSQL subtransaction: if one stream is absent/dirty,
            -- all partial marker updates roll back before the full refresh.
            BEGIN
                WITH changed_streams AS MATERIALIZED (
                    SELECT DISTINCT client_id, source_kind, interface
                    FROM new_traffic_counter_samples
                )
                UPDATE traffic_counter_hourly_usage_streams streams
                SET
                    source_revision = streams.source_revision + 1,
                    materialized_revision = streams.source_revision + 1,
                    updated_at = now()
                FROM changed_streams changed
                WHERE streams.client_id = changed.client_id
                  AND streams.source_kind = changed.source_kind
                  AND streams.interface = changed.interface
                  AND streams.source_revision = streams.materialized_revision;
                GET DIAGNOSTICS updated_stream_count = ROW_COUNT;

                SELECT count(*)::bigint
                INTO changed_stream_count
                FROM (
                    SELECT DISTINCT client_id, source_kind, interface
                    FROM new_traffic_counter_samples
                ) changed;
                IF updated_stream_count IS DISTINCT FROM changed_stream_count THEN
                    RAISE EXCEPTION
                        'traffic import same-shape update encountered a missing or dirty hourly marker'
                        USING ERRCODE = 'PZ001';
                END IF;
            EXCEPTION
                WHEN SQLSTATE 'PZ001' THEN
                    lineage_only := FALSE;
            END;
            IF lineage_only THEN
                RETURN NULL;
            END IF;
        END IF;
    END IF;

    SELECT count(*) INTO changed_count
    FROM (
        SELECT client_id, source_kind, interface, observed_at
        FROM old_traffic_counter_samples
        UNION
        SELECT client_id, source_kind, interface, observed_at
        FROM new_traffic_counter_samples
    ) changed;
    IF changed_count > 4096 THEN
        WITH changed AS (
            SELECT client_id, source_kind, interface, observed_at
            FROM old_traffic_counter_samples
            UNION
            SELECT client_id, source_kind, interface, observed_at
            FROM new_traffic_counter_samples
        )
        SELECT
            array_agg(client_id ORDER BY client_id, source_kind, interface),
            array_agg(source_kind ORDER BY client_id, source_kind, interface),
            array_agg(interface ORDER BY client_id, source_kind, interface),
            array_agg(observed_at ORDER BY client_id, source_kind, interface)
        INTO client_ids, source_kinds, interfaces, observed_values
        FROM (
            SELECT
                client_id,
                source_kind,
                interface,
                MIN(observed_at) AS observed_at
            FROM changed
            GROUP BY client_id, source_kind, interface
        ) changed_streams;
        PERFORM refresh_traffic_counter_hourly_usage(
            client_ids, source_kinds, interfaces, observed_values, TRUE
        );
        RETURN NULL;
    END IF;
    WITH changed AS (
        SELECT client_id, source_kind, interface, observed_at
        FROM old_traffic_counter_samples
        UNION
        SELECT client_id, source_kind, interface, observed_at
        FROM new_traffic_counter_samples
    )
    SELECT
        array_agg(client_id ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(source_kind ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(interface ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(observed_at ORDER BY client_id, source_kind, interface, observed_at)
    INTO client_ids, source_kinds, interfaces, observed_values
    FROM changed;
    PERFORM refresh_traffic_counter_hourly_usage(
        client_ids, source_kinds, interfaces, observed_values
    );
    RETURN NULL;
END;
$$;
