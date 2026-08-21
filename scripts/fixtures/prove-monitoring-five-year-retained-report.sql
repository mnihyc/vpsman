\set ON_ERROR_STOP on
SET TIME ZONE 'UTC';

-- Machine-readable invariant report for prove-monitoring-five-year-retained.sql.
-- Keep this read-only so it can be captured before and after the real worker's
-- bounded maintenance passes.

DO $$
DECLARE
    pressure_clients BIGINT;
    resource_streams BIGINT;
    resource_min_rows BIGINT;
    resource_max_rows BIGINT;
    resource_min_minutes BIGINT;
    resource_max_minutes BIGINT;
    observation_min_checks BIGINT;
    observation_max_checks BIGINT;
    observation_min_exact_rows BIGINT;
    observation_max_exact_rows BIGINT;
    observation_min_rollup_rows BIGINT;
    observation_max_rollup_rows BIGINT;
    gap_or_overlap_rows BIGINT;
BEGIN
    SELECT count(*) INTO pressure_clients
    FROM clients WHERE id LIKE 'pressure-%';
    IF pressure_clients <> 120 THEN
        RAISE EXCEPTION 'retained-history report requires 120 pressure clients';
    END IF;

    SELECT
        count(*), min(rows), max(rows), min(minutes), max(minutes)
    INTO
        resource_streams, resource_min_rows, resource_max_rows,
        resource_min_minutes, resource_max_minutes
    FROM (
        SELECT
            client_id,
            count(*) AS rows,
            sum(sample_count) AS minutes,
            count(DISTINCT bucket_secs) AS tiers
        FROM telemetry_rollups
        WHERE client_id LIKE 'pressure-%'
        GROUP BY client_id
    ) stream
    WHERE stream.tiers = 7;
    IF resource_streams <> 120
       OR resource_min_rows <> resource_max_rows
       OR resource_min_minutes <> resource_max_minutes
       OR resource_min_minutes NOT BETWEEN 2628000 AND 2629439 THEN
        RAISE EXCEPTION
            'resource stream shape mismatch: streams %, rows %..%, minutes %..%',
            resource_streams, resource_min_rows, resource_max_rows,
            resource_min_minutes, resource_max_minutes;
    END IF;

    SELECT count(*) INTO gap_or_overlap_rows
    FROM (
        SELECT
            client_id,
            bucket_start,
            lag(bucket_start + make_interval(secs => bucket_secs)) OVER (
                PARTITION BY client_id ORDER BY bucket_start, bucket_secs
            ) AS previous_end
        FROM telemetry_rollups
        WHERE client_id LIKE 'pressure-%'
    ) ordered
    WHERE previous_end IS NOT NULL AND previous_end <> bucket_start;
    IF gap_or_overlap_rows <> 0 THEN
        RAISE EXCEPTION
            'resource retained tiers contain % gaps or overlaps', gap_or_overlap_rows;
    END IF;

    SELECT count(*) INTO gap_or_overlap_rows
    FROM (
        SELECT bucket_start, previous_end
        FROM (
            SELECT
                client_id,
                bucket_start,
                lag(bucket_start + make_interval(secs => bucket_secs)) OVER (
                    PARTITION BY client_id, interface
                    ORDER BY bucket_start, bucket_secs
                ) AS previous_end
            FROM telemetry_network_rates
            WHERE client_id LIKE 'pressure-%'
        ) network_ordered
        UNION ALL
        SELECT bucket_start, previous_end
        FROM (
            SELECT
                rollup.bucket_start,
                lag(rollup.bucket_start
                    + make_interval(secs => rollup.bucket_secs)) OVER (
                        PARTITION BY rollup.series_id
                        ORDER BY rollup.bucket_start, rollup.bucket_secs
                    ) AS previous_end
            FROM telemetry_ping_rollups rollup
            JOIN telemetry_ping_series series ON series.id = rollup.series_id
            WHERE series.client_id LIKE 'pressure-%'
        ) ping_ordered
        UNION ALL
        SELECT bucket_start, previous_end
        FROM (
            SELECT
                bucket_start,
                lag(bucket_start + make_interval(secs => bucket_secs)) OVER (
                    PARTITION BY metric ORDER BY bucket_start, bucket_secs
                ) AS previous_end
            FROM system_metric_rollups
            WHERE metric LIKE 'pressure.%'
        ) system_ordered
    ) ordered
    WHERE previous_end IS NOT NULL AND previous_end <> bucket_start;
    IF gap_or_overlap_rows <> 0 THEN
        RAISE EXCEPTION
            'network, Ping, or system retained tiers contain % gaps or overlaps',
            gap_or_overlap_rows;
    END IF;

    IF (SELECT count(*) FROM telemetry_samples
        WHERE client_id LIKE 'pressure-%') <> 120 * 10080
       OR EXISTS (
            SELECT 1 FROM telemetry_samples
            WHERE client_id LIKE 'pressure-%'
            GROUP BY client_id
            HAVING count(*) <> 10080
                OR max(observed_at) - min(observed_at) <> interval '6 days 23:59'
       )
       OR (SELECT count(*) FROM telemetry_counter_facts
           WHERE client_id LIKE 'pressure-%') <> 120 * 10080
       OR (SELECT count(*) FROM telemetry_resource_latest
           WHERE client_id LIKE 'pressure-%') <> 120 THEN
        RAISE EXCEPTION 'raw resource/current cardinality mismatch';
    END IF;

    IF (SELECT count(*) FROM telemetry_network_rates
        WHERE client_id LIKE 'pressure-%') <> 120 * resource_min_rows
       OR EXISTS (
            SELECT 1
            FROM telemetry_network_rates rate
            WHERE rate.client_id LIKE 'pressure-%'
            GROUP BY rate.client_id, rate.interface
            HAVING count(*) <> resource_min_rows
                OR sum(rate.sample_count) <> resource_min_minutes
                OR count(DISTINCT rate.bucket_secs) <> 7
       ) THEN
        RAISE EXCEPTION 'network-rate retained shape differs from resources';
    END IF;

    IF (SELECT count(*) FROM telemetry_ping_series
        WHERE client_id LIKE 'pressure-%') <> 120
       OR EXISTS (
            SELECT 1
            FROM telemetry_ping_series series
            LEFT JOIN telemetry_ping_facts fact ON fact.series_id = series.id
            WHERE series.client_id LIKE 'pressure-%'
            GROUP BY series.id
            HAVING count(fact.series_id) <> 10080
       )
       OR EXISTS (
            SELECT 1
            FROM telemetry_ping_series series
            LEFT JOIN telemetry_ping_rollups rollup ON rollup.series_id = series.id
            WHERE series.client_id LIKE 'pressure-%'
            GROUP BY series.id
            HAVING count(rollup.series_id) <> resource_min_rows
                OR sum(rollup.sample_count) <> resource_min_minutes
                OR count(DISTINCT rollup.bucket_secs) <> 7
       )
       OR (SELECT count(*) FROM telemetry_ping_current current
           JOIN telemetry_ping_series series ON series.id = current.series_id
           WHERE series.client_id LIKE 'pressure-%') <> 120 THEN
        RAISE EXCEPTION 'Ping retained shape differs from resources';
    END IF;

    IF (SELECT count(*) FROM network_observation_series series
        JOIN tunnel_plans plan ON plan.id = series.plan_id
        WHERE plan.name LIKE 'pressure-history-plan-%') <> 120
       OR (SELECT count(*) FROM network_observation_latest latest
           JOIN network_observation_series series ON series.id = latest.series_id
           JOIN tunnel_plans plan ON plan.id = series.plan_id
           WHERE plan.name LIKE 'pressure-history-plan-%') <> 120
       OR EXISTS (
            SELECT 1
            FROM network_observation_series series
            JOIN tunnel_plans plan ON plan.id = series.plan_id
            LEFT JOIN network_observations observation
              ON observation.automatic_series_id = series.id
            WHERE plan.name LIKE 'pressure-history-plan-%'
            GROUP BY series.id
            HAVING count(observation.*) = 0
                OR min(observation.source) <> 'automatic'
                OR max(observation.source) <> 'automatic'
       )
       OR EXISTS (
            SELECT 1
            FROM network_observation_series series
            JOIN tunnel_plans plan ON plan.id = series.plan_id
            LEFT JOIN network_observation_rollups rollup ON rollup.series_id = series.id
            WHERE plan.name LIKE 'pressure-history-plan-%'
            GROUP BY series.id
            HAVING count(DISTINCT rollup.bucket_secs) <> 6
       ) THEN
        RAISE EXCEPTION 'network-observation retained shape is incomplete';
    END IF;

    SELECT
        min(exact_rows), max(exact_rows),
        min(rollup_rows), max(rollup_rows)
    INTO
        observation_min_exact_rows, observation_max_exact_rows,
        observation_min_rollup_rows, observation_max_rollup_rows
    FROM (
        SELECT
            series.id,
            (SELECT count(*) FROM network_observations observation
                WHERE observation.automatic_series_id = series.id) AS exact_rows,
            (SELECT count(*) FROM network_observation_rollups rollup
                WHERE rollup.series_id = series.id) AS rollup_rows
        FROM network_observation_series series
        JOIN tunnel_plans plan ON plan.id = series.plan_id
        WHERE plan.name LIKE 'pressure-history-plan-%'
    ) stream;
    IF observation_min_exact_rows <> 552
       OR observation_max_exact_rows <> 552
       OR observation_min_rollup_rows <> observation_max_rollup_rows
       OR observation_min_rollup_rows NOT BETWEEN 7192 AND 7203 THEN
        RAISE EXCEPTION
            'network-observation row shape mismatch: exact %..%, rollups %..%',
            observation_min_exact_rows, observation_max_exact_rows,
            observation_min_rollup_rows, observation_max_rollup_rows;
    END IF;

    SELECT count(*) INTO gap_or_overlap_rows
    FROM (
        SELECT
            series_id,
            evidence_start,
            lag(evidence_end) OVER (
                PARTITION BY series_id ORDER BY evidence_start, evidence_end
            ) AS previous_end
        FROM (
            SELECT
                observation.automatic_series_id AS series_id,
                observation.observed_at AS evidence_start,
                observation.observed_at + interval '5 minutes' AS evidence_end
            FROM network_observations observation
            JOIN tunnel_plans plan ON plan.id = observation.plan_id
            WHERE plan.name LIKE 'pressure-history-plan-%'
            UNION ALL
            SELECT
                rollup.series_id,
                rollup.bucket_start,
                rollup.bucket_start + make_interval(secs => rollup.bucket_secs)
            FROM network_observation_rollups rollup
            JOIN network_observation_series series ON series.id = rollup.series_id
            JOIN tunnel_plans plan ON plan.id = series.plan_id
            WHERE plan.name LIKE 'pressure-history-plan-%'
        ) evidence
    ) ordered
    WHERE previous_end IS NOT NULL AND previous_end <> evidence_start;
    IF gap_or_overlap_rows <> 0 THEN
        RAISE EXCEPTION
            'network-observation evidence contains % gaps or overlaps',
            gap_or_overlap_rows;
    END IF;

    SELECT min(represented_checks), max(represented_checks)
    INTO observation_min_checks, observation_max_checks
    FROM (
        SELECT
            series.id,
            (SELECT count(*) FROM network_observations observation
                WHERE observation.automatic_series_id = series.id)
            +
            (SELECT COALESCE(sum(rollup.sample_count), 0)
                FROM network_observation_rollups rollup
                WHERE rollup.series_id = series.id) AS represented_checks
        FROM network_observation_series series
        JOIN tunnel_plans plan ON plan.id = series.plan_id
        WHERE plan.name LIKE 'pressure-history-plan-%'
    ) streams;
    IF observation_min_checks <> observation_max_checks
       OR observation_min_checks NOT BETWEEN 525600 AND 525887 THEN
        RAISE EXCEPTION
            'network-observation represented checks mismatch: %..%',
            observation_min_checks, observation_max_checks;
    END IF;

    IF (SELECT count(DISTINCT metric) FROM system_metric_rollups
        WHERE metric LIKE 'pressure.%') <> 50
       OR EXISTS (
            SELECT 1
            FROM system_metric_rollups
            WHERE metric LIKE 'pressure.%'
            GROUP BY metric
            HAVING count(*) <> resource_min_rows
                OR sum(sample_count) <> resource_min_minutes
                OR count(DISTINCT bucket_secs) <> 7
       ) THEN
        RAISE EXCEPTION 'system-metric retained shape differs from resources';
    END IF;
END;
$$;

WITH resource_streams AS (
    SELECT
        client_id,
        count(*) AS rows,
        sum(sample_count) AS represented_minutes,
        min(bucket_start) AS oldest,
        max(bucket_start + make_interval(secs => bucket_secs)) AS newest
    FROM telemetry_rollups
    WHERE client_id LIKE 'pressure-%'
    GROUP BY client_id
), resource_tiers AS (
    SELECT
        bucket_secs,
        count(*) AS total_rows,
        min(per_stream_rows) AS min_rows_per_stream,
        max(per_stream_rows) AS max_rows_per_stream
    FROM (
        SELECT client_id, bucket_secs, count(*) AS per_stream_rows
        FROM telemetry_rollups
        WHERE client_id LIKE 'pressure-%'
        GROUP BY client_id, bucket_secs
    ) per_stream
    GROUP BY bucket_secs
), ping_streams AS (
    SELECT
        series.id,
        count(rollup.series_id) AS rows,
        sum(rollup.sample_count) AS represented_minutes
    FROM telemetry_ping_series series
    LEFT JOIN telemetry_ping_rollups rollup ON rollup.series_id = series.id
    WHERE series.client_id LIKE 'pressure-%'
    GROUP BY series.id
), observation_streams AS (
    SELECT
        series.id,
        exact.exact_rows,
        retained.rollup_rows,
        exact.exact_rows + retained.represented_checks AS represented_checks
    FROM network_observation_series series
    JOIN tunnel_plans plan ON plan.id = series.plan_id
    CROSS JOIN LATERAL (
        SELECT count(*) AS exact_rows
        FROM network_observations observation
        WHERE observation.automatic_series_id = series.id
    ) exact
    CROSS JOIN LATERAL (
        SELECT
            count(*) AS rollup_rows,
            COALESCE(sum(rollup.sample_count), 0) AS represented_checks
        FROM network_observation_rollups rollup
        WHERE rollup.series_id = series.id
    ) retained
    WHERE plan.name LIKE 'pressure-history-plan-%'
), table_sizes AS (
    SELECT jsonb_object_agg(table_name, total_bytes ORDER BY table_name) AS bytes
    FROM (
        SELECT
            table_name,
            pg_total_relation_size(table_name::regclass) AS total_bytes
        FROM unnest(ARRAY[
            'telemetry_samples',
            'telemetry_counter_facts',
            'telemetry_rollups',
            'telemetry_resource_latest',
            'telemetry_network_rates',
            'telemetry_ping_series',
            'telemetry_ping_facts',
            'telemetry_ping_rollups',
            'telemetry_ping_current',
            'network_observation_series',
            'network_observations',
            'network_observation_latest',
            'network_observation_rollups',
            'system_metric_rollups',
            'traffic_counter_samples',
            'traffic_counter_rollups',
            'traffic_counter_hourly_usage',
            'traffic_counter_hourly_usage_streams'
        ]) listed(table_name)
    ) sized
), eligible_resource AS (
    SELECT count(*) AS rows
    FROM telemetry_rollups rollup
    JOIN (VALUES
        (60, 300, 2),
        (300, 1800, 8),
        (1800, 3600, 31),
        (3600, 10800, 91),
        (10800, 21600, 181),
        (21600, 86400, 366)
    ) tier(source_secs, destination_secs, retain_days)
      ON tier.source_secs = rollup.bucket_secs
    WHERE rollup.client_id LIKE 'pressure-%'
      AND rollup.bucket_start < to_timestamp(floor(extract(epoch FROM (
            now() - make_interval(days => tier.retain_days)
          )) / tier.destination_secs) * tier.destination_secs)
), eligible_rates AS (
    SELECT count(*) AS rows
    FROM telemetry_network_rates rate
    JOIN (VALUES
        (60, 300, 2),
        (300, 1800, 8),
        (1800, 3600, 31),
        (3600, 10800, 91),
        (10800, 21600, 181),
        (21600, 86400, 366)
    ) tier(source_secs, destination_secs, retain_days)
      ON tier.source_secs = rate.bucket_secs
    WHERE rate.client_id LIKE 'pressure-%'
      AND rate.bucket_start < to_timestamp(floor(extract(epoch FROM (
            now() - make_interval(days => tier.retain_days)
          )) / tier.destination_secs) * tier.destination_secs)
), eligible_ping AS (
    SELECT count(*) AS rows
    FROM telemetry_ping_rollups rollup
    JOIN telemetry_ping_series series ON series.id = rollup.series_id
    JOIN (VALUES
        (60, 300, 2),
        (300, 1800, 8),
        (1800, 3600, 31),
        (3600, 10800, 91),
        (10800, 21600, 181),
        (21600, 86400, 366)
    ) tier(source_secs, destination_secs, retain_days)
      ON tier.source_secs = rollup.bucket_secs
    WHERE series.client_id LIKE 'pressure-%'
      AND rollup.bucket_start < to_timestamp(floor(extract(epoch FROM (
            now() - make_interval(days => tier.retain_days)
          )) / tier.destination_secs) * tier.destination_secs)
), eligible_system AS (
    SELECT count(*) AS rows
    FROM system_metric_rollups rollup
    JOIN (VALUES
        (60, 300, 2),
        (300, 1800, 8),
        (1800, 3600, 31),
        (3600, 10800, 91),
        (10800, 21600, 181),
        (21600, 86400, 366)
    ) tier(source_secs, destination_secs, retain_days)
      ON tier.source_secs = rollup.bucket_secs
    WHERE rollup.metric LIKE 'pressure.%'
      AND rollup.bucket_start < to_timestamp(floor(extract(epoch FROM (
            now() - make_interval(days => tier.retain_days)
          )) / tier.destination_secs) * tier.destination_secs)
), eligible_raw_resource AS (
    SELECT count(*) AS rows
    FROM telemetry_samples sample
    WHERE sample.client_id LIKE 'pressure-%'
      AND sample.observed_at < (
        date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
      ) - interval '7 days'
), eligible_raw_ping AS (
    SELECT count(*) AS rows
    FROM telemetry_ping_facts fact
    JOIN telemetry_ping_series series ON series.id = fact.series_id
    WHERE series.client_id LIKE 'pressure-%'
      AND fact.observed_at < (
        date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
      ) - interval '7 days'
), eligible_observations AS (
    SELECT
        (SELECT count(*)
         FROM network_observations observation
         JOIN tunnel_plans plan ON plan.id = observation.plan_id
         WHERE plan.name LIKE 'pressure-history-plan-%'
           AND observation.observed_at < to_timestamp(floor(extract(epoch FROM (
                now() - interval '2 days'
              )) / 300) * 300))
        +
        (SELECT count(*)
         FROM network_observation_rollups rollup
         JOIN network_observation_series series ON series.id = rollup.series_id
         JOIN tunnel_plans plan ON plan.id = series.plan_id
         JOIN (VALUES
            (300, 1800, 8),
            (1800, 3600, 31),
            (3600, 10800, 91),
            (10800, 21600, 181),
            (21600, 86400, 366)
         ) tier(source_secs, destination_secs, retain_days)
           ON tier.source_secs = rollup.bucket_secs
         WHERE plan.name LIKE 'pressure-history-plan-%'
           AND rollup.bucket_start < to_timestamp(floor(extract(epoch FROM (
                now() - make_interval(days => tier.retain_days)
              )) / tier.destination_secs) * tier.destination_secs)) AS rows
)
SELECT jsonb_build_object(
    'schema', 'vpsman-five-year-retained-report/v1',
    'captured_at', clock_timestamp(),
    'raw', jsonb_build_object(
        'resource_rows', (SELECT count(*) FROM telemetry_samples
            WHERE client_id LIKE 'pressure-%'),
        'counter_fact_rows', (SELECT count(*) FROM telemetry_counter_facts
            WHERE client_id LIKE 'pressure-%'),
        'ping_fact_rows', (SELECT count(*) FROM telemetry_ping_facts fact
            JOIN telemetry_ping_series series ON series.id = fact.series_id
            WHERE series.client_id LIKE 'pressure-%')
    ),
    'resource', jsonb_build_object(
        'streams', (SELECT count(*) FROM resource_streams),
        'total_rows', (SELECT sum(rows) FROM resource_streams),
        'min_rows_per_stream', (SELECT min(rows) FROM resource_streams),
        'max_rows_per_stream', (SELECT max(rows) FROM resource_streams),
        'represented_minutes_per_stream',
            (SELECT min(represented_minutes) FROM resource_streams),
        'oldest', (SELECT min(oldest) FROM resource_streams),
        'newest', (SELECT max(newest) FROM resource_streams),
        'tiers', (SELECT jsonb_object_agg(bucket_secs::text,
            jsonb_build_object(
                'total_rows', total_rows,
                'min_rows_per_stream', min_rows_per_stream,
                'max_rows_per_stream', max_rows_per_stream
            ) ORDER BY bucket_secs) FROM resource_tiers)
    ),
    'network_rates', jsonb_build_object(
        'streams', (SELECT count(DISTINCT (client_id, interface))
            FROM telemetry_network_rates WHERE client_id LIKE 'pressure-%'),
        'total_rows', (SELECT count(*) FROM telemetry_network_rates
            WHERE client_id LIKE 'pressure-%'),
        'represented_minutes', (SELECT sum(sample_count)
            FROM telemetry_network_rates WHERE client_id LIKE 'pressure-%')
    ),
    'ping', jsonb_build_object(
        'streams', (SELECT count(*) FROM ping_streams),
        'total_rollup_rows', (SELECT sum(rows) FROM ping_streams),
        'represented_minutes', (SELECT sum(represented_minutes) FROM ping_streams),
        'current_rows', (SELECT count(*) FROM telemetry_ping_current current
            JOIN telemetry_ping_series series ON series.id = current.series_id
            WHERE series.client_id LIKE 'pressure-%')
    ),
    'network_observations', jsonb_build_object(
        'streams', (SELECT count(*) FROM observation_streams),
        'exact_rows', (SELECT sum(exact_rows) FROM observation_streams),
        'rollup_rows', (SELECT sum(rollup_rows) FROM observation_streams),
        'min_exact_rows_per_stream', (SELECT min(exact_rows) FROM observation_streams),
        'max_exact_rows_per_stream', (SELECT max(exact_rows) FROM observation_streams),
        'min_rollup_rows_per_stream', (SELECT min(rollup_rows) FROM observation_streams),
        'max_rollup_rows_per_stream', (SELECT max(rollup_rows) FROM observation_streams),
        'min_represented_checks_per_stream',
            (SELECT min(represented_checks) FROM observation_streams),
        'max_represented_checks_per_stream',
            (SELECT max(represented_checks) FROM observation_streams),
        'latest_rows', (SELECT count(*) FROM network_observation_latest latest
            JOIN network_observation_series series ON series.id = latest.series_id
            JOIN tunnel_plans plan ON plan.id = series.plan_id
            WHERE plan.name LIKE 'pressure-history-plan-%')
    ),
    'system_metrics', jsonb_build_object(
        'series', (SELECT count(DISTINCT metric) FROM system_metric_rollups
            WHERE metric LIKE 'pressure.%'),
        'rows', (SELECT count(*) FROM system_metric_rollups
            WHERE metric LIKE 'pressure.%'),
        'represented_minutes', (SELECT sum(sample_count) FROM system_metric_rollups
            WHERE metric LIKE 'pressure.%')
    ),
    'traffic', jsonb_build_object(
        'raw_rows', (SELECT count(*) FROM traffic_counter_samples
            WHERE client_id LIKE 'pressure-%'),
        'rollup_rows', (SELECT count(*) FROM traffic_counter_rollups
            WHERE client_id LIKE 'pressure-%'),
        'hourly_rows', (SELECT count(*) FROM traffic_counter_hourly_usage
            WHERE client_id LIKE 'pressure-%'),
        'hourly_rx_bytes', (SELECT COALESCE(sum(rx_bytes), 0)
            FROM traffic_counter_hourly_usage WHERE client_id LIKE 'pressure-%'),
        'hourly_tx_bytes', (SELECT COALESCE(sum(tx_bytes), 0)
            FROM traffic_counter_hourly_usage WHERE client_id LIKE 'pressure-%')
    ),
    'maintenance_eligible_source_rows', jsonb_build_object(
        'resource', (SELECT rows FROM eligible_resource),
        'network_rates', (SELECT rows FROM eligible_rates),
        'ping', (SELECT rows FROM eligible_ping),
        'system_metrics', (SELECT rows FROM eligible_system),
        'raw_resource', (SELECT rows FROM eligible_raw_resource),
        'raw_ping', (SELECT rows FROM eligible_raw_ping),
        'network_observations', (SELECT rows FROM eligible_observations)
    ),
    'table_total_bytes', (SELECT bytes FROM table_sizes),
    'database_bytes', pg_database_size(current_database())
);
