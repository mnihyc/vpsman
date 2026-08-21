-- Whole-stream hourly-ledger repairs used to materialize every complete raw
-- sample (including columns the accounting oracle never reads) before the
-- hourly aggregate could consume it. A bounded vnStat raw tail therefore
-- spilled the same stream while sorting and again while materializing it.
-- Preserve the exact revision and transition semantics while scanning each
-- changed stream independently in primary-key order and carrying only the
-- columns required by the hourly accounting oracle. This migration replaces
-- only executable function code; it does not rewrite retained rows.
CREATE OR REPLACE FUNCTION refresh_traffic_counter_hourly_usage(
    changed_client_ids TEXT[],
    changed_source_kinds TEXT[],
    changed_interfaces TEXT[],
    changed_observed_at TIMESTAMPTZ[],
    rebuild_entire_streams BOOLEAN DEFAULT FALSE
) RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    coverage_requires_rebuild BOOLEAN;
BEGIN
    IF COALESCE(array_length(changed_client_ids, 1), 0) = 0 THEN
        RETURN;
    END IF;
    IF array_length(changed_client_ids, 1)
            IS DISTINCT FROM array_length(changed_source_kinds, 1)
       OR array_length(changed_client_ids, 1)
            IS DISTINCT FROM array_length(changed_interfaces, 1)
       OR array_length(changed_client_ids, 1)
            IS DISTINCT FROM array_length(changed_observed_at, 1) THEN
        RAISE EXCEPTION 'traffic hourly refresh arrays must have equal lengths';
    END IF;

    -- Client deletion cascades through raw samples and both derived tables.
    -- The raw AFTER DELETE trigger must not recreate a coverage row for an
    -- identity that is already absent in this transaction.
    IF NOT EXISTS (
        SELECT 1
        FROM UNNEST(changed_client_ids) AS changed(client_id)
        JOIN clients ON clients.id = changed.client_id
    ) THEN
        RETURN;
    END IF;

    -- A missing stream marker may be a brand-new stream or an explicitly
    -- damaged cache. A mismatched revision is definitely incomplete. Rebuild
    -- the complete changed streams in either case before declaring them
    -- healthy; subsequent ordinary mutations may use the narrower repair.
    WITH changed_streams AS (
        SELECT DISTINCT client_id, source_kind, interface
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_observed_at
        ) AS item(client_id, source_kind, interface, observed_at)
    )
    SELECT COALESCE(bool_or(
        streams.client_id IS NULL
        OR streams.source_revision <> streams.materialized_revision
    ), FALSE)
    INTO coverage_requires_rebuild
    FROM changed_streams changed
    JOIN clients ON clients.id = changed.client_id
    LEFT JOIN traffic_counter_hourly_usage_streams streams
      ON streams.client_id = changed.client_id
     AND streams.source_kind = changed.source_kind
     AND streams.interface = changed.interface;

    INSERT INTO traffic_counter_hourly_usage_streams (
        client_id,
        source_kind,
        interface,
        source_revision,
        materialized_revision,
        updated_at
    )
    SELECT DISTINCT
        changed.client_id,
        changed.source_kind,
        changed.interface,
        1,
        0,
        now()
    FROM UNNEST(
        changed_client_ids,
        changed_source_kinds,
        changed_interfaces,
        changed_observed_at
    ) AS changed(client_id, source_kind, interface, observed_at)
    JOIN clients ON clients.id = changed.client_id
    ON CONFLICT (client_id, source_kind, interface) DO UPDATE SET
        source_revision =
            traffic_counter_hourly_usage_streams.source_revision + 1,
        updated_at = now();

    -- Large imports and whole-stream epoch rewrites use one exact-key ordered
    -- scan per changed stream. The LATERAL boundary keeps each window local to
    -- one primary-key range, while the narrow projection avoids retaining a
    -- second full-row copy of the stream.
    IF rebuild_entire_streams
       OR coverage_requires_rebuild
       OR array_length(changed_client_ids, 1) > 4096 THEN
        WITH changed_streams AS (
            SELECT DISTINCT client_id, source_kind, interface
            FROM UNNEST(
                changed_client_ids,
                changed_source_kinds,
                changed_interfaces,
                changed_observed_at
            ) AS item(client_id, source_kind, interface, observed_at)
        )
        DELETE FROM traffic_counter_hourly_usage usage
        USING changed_streams changed
        WHERE usage.client_id = changed.client_id
          AND usage.source_kind = changed.source_kind
          AND usage.interface = changed.interface;

        WITH changed_streams AS MATERIALIZED (
            SELECT DISTINCT client_id, source_kind, interface
            FROM UNNEST(
                changed_client_ids,
                changed_source_kinds,
                changed_interfaces,
                changed_observed_at
            ) AS item(client_id, source_kind, interface, observed_at)
        )
        INSERT INTO traffic_counter_hourly_usage (
            client_id,
            source_kind,
            interface,
            bucket_start,
            rx_bytes,
            tx_bytes,
            rx_reset_count,
            tx_reset_count,
            sample_count,
            first_observed_at,
            latest_observed_at,
            updated_at
        )
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            hourly.bucket_start,
            hourly.rx_bytes,
            hourly.tx_bytes,
            hourly.rx_reset_count,
            hourly.tx_reset_count,
            hourly.sample_count,
            hourly.first_observed_at,
            hourly.latest_observed_at,
            now()
        FROM changed_streams changed
        CROSS JOIN LATERAL (
            WITH sequenced AS (
                SELECT
                    sample.observed_at,
                    sample.rx_bytes,
                    sample.tx_bytes,
                    sample.rx_counter_epoch,
                    sample.tx_counter_epoch,
                    sample.sample_source,
                    LAG(sample.rx_bytes) OVER ordered
                        AS previous_rx_bytes,
                    LAG(sample.tx_bytes) OVER ordered
                        AS previous_tx_bytes,
                    LAG(sample.rx_counter_epoch) OVER ordered
                        AS previous_rx_counter_epoch,
                    LAG(sample.tx_counter_epoch) OVER ordered
                        AS previous_tx_counter_epoch,
                    LAG(sample.sample_source) OVER ordered
                        AS previous_sample_source
                FROM traffic_counter_samples sample
                WHERE sample.client_id = changed.client_id
                  AND sample.source_kind = changed.source_kind
                  AND sample.interface = changed.interface
                  AND sample.observed_at >= '-infinity'::timestamptz
                  AND sample.observed_at <= 'infinity'::timestamptz
                WINDOW ordered AS (ORDER BY sample.observed_at)
            )
            SELECT
                date_bin(
                    interval '1 hour',
                    observed_at,
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) AS bucket_start,
                COALESCE(SUM(CASE
                    WHEN rx_counter_epoch = previous_rx_counter_epoch
                     AND rx_bytes >= previous_rx_bytes
                    THEN rx_bytes - previous_rx_bytes ELSE 0 END
                ), 0)::bigint AS rx_bytes,
                COALESCE(SUM(CASE
                    WHEN tx_counter_epoch = previous_tx_counter_epoch
                     AND tx_bytes >= previous_tx_bytes
                    THEN tx_bytes - previous_tx_bytes ELSE 0 END
                ), 0)::bigint AS tx_bytes,
                COUNT(*) FILTER (
                    WHERE previous_rx_counter_epoch IS NOT NULL
                      AND rx_counter_epoch <> previous_rx_counter_epoch
                      AND NOT (
                          previous_sample_source LIKE 'vnstat_import:%'
                          AND sample_source NOT LIKE 'vnstat_import:%'
                      )
                )::integer AS rx_reset_count,
                COUNT(*) FILTER (
                    WHERE previous_tx_counter_epoch IS NOT NULL
                      AND tx_counter_epoch <> previous_tx_counter_epoch
                      AND NOT (
                          previous_sample_source LIKE 'vnstat_import:%'
                          AND sample_source NOT LIKE 'vnstat_import:%'
                      )
                )::integer AS tx_reset_count,
                COUNT(*)::integer AS sample_count,
                MIN(observed_at) AS first_observed_at,
                MAX(observed_at) AS latest_observed_at
            FROM sequenced
            GROUP BY date_bin(
                interval '1 hour',
                observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            )
        ) hourly;

        UPDATE traffic_counter_hourly_usage_streams streams
        SET
            materialized_revision = streams.source_revision,
            updated_at = now()
        FROM (
            SELECT DISTINCT client_id, source_kind, interface
            FROM UNNEST(
                changed_client_ids,
                changed_source_kinds,
                changed_interfaces,
                changed_observed_at
            ) AS item(client_id, source_kind, interface, observed_at)
        ) changed
        WHERE streams.client_id = changed.client_id
          AND streams.source_kind = changed.source_kind
          AND streams.interface = changed.interface;
        RETURN;
    END IF;

    -- Updating a sample changes the transition attributed to that sample and
    -- to its immediate successor. Rebuild the hours containing both. For a
    -- multi-row import/update, DISTINCT collapses repeated work per hour.
    WITH changed AS MATERIALIZED (
        SELECT DISTINCT *
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_observed_at
        ) AS item(client_id, source_kind, interface, observed_at)
    ), changed_with_next AS MATERIALIZED (
        SELECT
            changed.*,
            LEAD(observed_at) OVER (
                PARTITION BY client_id, source_kind, interface
                ORDER BY observed_at
            ) AS next_changed_at
        FROM changed
    ), affected AS MATERIALIZED (
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            date_bin(
                interval '1 hour',
                changed.observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) AS bucket_start
        FROM changed_with_next changed
        UNION
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            date_bin(
                interval '1 hour',
                successor.observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) AS bucket_start
        FROM changed_with_next changed
        JOIN LATERAL (
            SELECT sample.observed_at
            FROM traffic_counter_samples sample
            WHERE sample.client_id = changed.client_id
              AND sample.source_kind = changed.source_kind
              AND sample.interface = changed.interface
              AND sample.observed_at > changed.observed_at
            ORDER BY sample.observed_at ASC
            LIMIT 1
        ) successor ON TRUE
        -- Samples are minute-aligned. Consecutive changed minutes are each
        -- other's successor and their hours are already present above, so a
        -- large telemetry import needs only one boundary lookup per gap.
        WHERE changed.next_changed_at IS NULL
           OR changed.next_changed_at > changed.observed_at + interval '1 minute'
    )
    DELETE FROM traffic_counter_hourly_usage usage
    USING affected
    WHERE usage.client_id = affected.client_id
      AND usage.source_kind = affected.source_kind
      AND usage.interface = affected.interface
      AND usage.bucket_start = affected.bucket_start;

    WITH changed AS MATERIALIZED (
        SELECT DISTINCT *
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_observed_at
        ) AS item(client_id, source_kind, interface, observed_at)
    ), changed_with_next AS MATERIALIZED (
        SELECT
            changed.*,
            LEAD(observed_at) OVER (
                PARTITION BY client_id, source_kind, interface
                ORDER BY observed_at
            ) AS next_changed_at
        FROM changed
    ), affected AS MATERIALIZED (
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            date_bin(
                interval '1 hour',
                changed.observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) AS bucket_start
        FROM changed_with_next changed
        UNION
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            date_bin(
                interval '1 hour',
                successor.observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) AS bucket_start
        FROM changed_with_next changed
        JOIN LATERAL (
            SELECT sample.observed_at
            FROM traffic_counter_samples sample
            WHERE sample.client_id = changed.client_id
              AND sample.source_kind = changed.source_kind
              AND sample.interface = changed.interface
              AND sample.observed_at > changed.observed_at
            ORDER BY sample.observed_at ASC
            LIMIT 1
        ) successor ON TRUE
        WHERE changed.next_changed_at IS NULL
           OR changed.next_changed_at > changed.observed_at + interval '1 minute'
    ), selected AS MATERIALIZED (
        SELECT
            affected.client_id,
            affected.source_kind,
            affected.interface,
            affected.bucket_start,
            sample.observed_at,
            sample.rx_bytes,
            sample.tx_bytes,
            sample.rx_counter_epoch,
            sample.tx_counter_epoch,
            sample.sample_source
        FROM affected
        JOIN LATERAL (
            (
                SELECT
                    sample.observed_at,
                    sample.rx_bytes,
                    sample.tx_bytes,
                    sample.rx_counter_epoch,
                    sample.tx_counter_epoch,
                    sample.sample_source
                FROM traffic_counter_samples sample
                WHERE sample.client_id = affected.client_id
                  AND sample.source_kind = affected.source_kind
                  AND sample.interface = affected.interface
                  AND sample.observed_at < affected.bucket_start
                ORDER BY sample.observed_at DESC
                LIMIT 1
            )
            UNION ALL
            SELECT
                sample.observed_at,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.sample_source
            FROM traffic_counter_samples sample
            WHERE sample.client_id = affected.client_id
              AND sample.source_kind = affected.source_kind
              AND sample.interface = affected.interface
              AND sample.observed_at >= affected.bucket_start
              AND sample.observed_at < affected.bucket_start + interval '1 hour'
        ) sample ON TRUE
    ), sequenced AS (
        SELECT
            selected.*,
            LAG(rx_bytes) OVER stream AS previous_rx_bytes,
            LAG(tx_bytes) OVER stream AS previous_tx_bytes,
            LAG(rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
            LAG(tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
            LAG(sample_source) OVER stream AS previous_sample_source
        FROM selected
        WINDOW stream AS (
            PARTITION BY client_id, source_kind, interface, bucket_start
            ORDER BY observed_at
        )
    )
    INSERT INTO traffic_counter_hourly_usage (
        client_id,
        source_kind,
        interface,
        bucket_start,
        rx_bytes,
        tx_bytes,
        rx_reset_count,
        tx_reset_count,
        sample_count,
        first_observed_at,
        latest_observed_at,
        updated_at
    )
    SELECT
        client_id,
        source_kind,
        interface,
        bucket_start,
        COALESCE(SUM(
            CASE
                WHEN rx_counter_epoch = previous_rx_counter_epoch
                 AND rx_bytes >= previous_rx_bytes
                THEN rx_bytes - previous_rx_bytes
                ELSE 0
            END
        ) FILTER (WHERE observed_at >= bucket_start), 0)::bigint,
        COALESCE(SUM(
            CASE
                WHEN tx_counter_epoch = previous_tx_counter_epoch
                 AND tx_bytes >= previous_tx_bytes
                THEN tx_bytes - previous_tx_bytes
                ELSE 0
            END
        ) FILTER (WHERE observed_at >= bucket_start), 0)::bigint,
        COUNT(*) FILTER (
            WHERE observed_at >= bucket_start
              AND previous_rx_counter_epoch IS NOT NULL
              AND rx_counter_epoch <> previous_rx_counter_epoch
              AND NOT (
                  previous_sample_source LIKE 'vnstat_import:%'
                  AND sample_source NOT LIKE 'vnstat_import:%'
              )
        )::integer,
        COUNT(*) FILTER (
            WHERE observed_at >= bucket_start
              AND previous_tx_counter_epoch IS NOT NULL
              AND tx_counter_epoch <> previous_tx_counter_epoch
              AND NOT (
                  previous_sample_source LIKE 'vnstat_import:%'
                  AND sample_source NOT LIKE 'vnstat_import:%'
              )
        )::integer,
        COUNT(*) FILTER (WHERE observed_at >= bucket_start)::integer,
        MIN(observed_at) FILTER (WHERE observed_at >= bucket_start),
        MAX(observed_at) FILTER (WHERE observed_at >= bucket_start),
        now()
    FROM sequenced
    GROUP BY client_id, source_kind, interface, bucket_start
    HAVING COUNT(*) FILTER (WHERE observed_at >= bucket_start) > 0;

    UPDATE traffic_counter_hourly_usage_streams streams
    SET
        materialized_revision = streams.source_revision,
        updated_at = now()
    FROM (
        SELECT DISTINCT client_id, source_kind, interface
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_observed_at
        ) AS item(client_id, source_kind, interface, observed_at)
    ) changed
    WHERE streams.client_id = changed.client_id
      AND streams.source_kind = changed.source_kind
      AND streams.interface = changed.interface;
END;
$$;
