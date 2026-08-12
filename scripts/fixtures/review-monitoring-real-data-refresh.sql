\set ON_ERROR_STOP on

BEGIN;

UPDATE clients
SET
    status = 'online',
    last_seen_at = now(),
    stale_since = NULL,
    stale_reason = NULL,
    stale_build_number = NULL,
    system_reported_at = now()
WHERE id LIKE 'review-%';

DELETE FROM telemetry_ping_series WHERE client_id LIKE 'review-%';
DELETE FROM telemetry_network_rates WHERE client_id LIKE 'review-%';
DELETE FROM telemetry_resource_latest WHERE client_id LIKE 'review-%';
DELETE FROM telemetry_rollups WHERE client_id LIKE 'review-%';
DELETE FROM telemetry_samples WHERE client_id LIKE 'review-%';
DELETE FROM traffic_counter_samples WHERE client_id LIKE 'review-%';

WITH review_cases (
    client_id,
    cpu_offset,
    cpu_cores,
    uptime_base,
    network_enabled,
    ping_mode
) AS (
    VALUES
        ('review-total-monthly', 0.12::double precision, 4, 720000::bigint, true, 'healthy'),
        ('review-traffic-exceeded', 0.28::double precision, 4, 950400::bigint, true, 'healthy'),
        ('review-rx-yearly', 0.18::double precision, 8, 1728000::bigint, true, 'degraded'),
        ('review-tx-unlimited', 0.08::double precision, 2, 345600::bigint, true, 'none'),
        ('review-no-reset', 0.22::double precision, 16, 2592000::bigint, true, 'none'),
        ('review-empty-rates', 0.05::double precision, 2, 86400::bigint, false, 'none'),
        ('review-unconfigured', 0.10::double precision, 4, 432000::bigint, false, 'none'),
        ('review-no-primary', 0.16::double precision, 6, 604800::bigint, true, 'none')
), points AS (
    SELECT
        review_cases.*,
        sample_index,
        date_trunc('minute', now()) - sample_index * interval '1 minute' AS observed_at,
        least(
            0.92::double precision,
            cpu_offset + ((15 - sample_index) % 5) * 0.025::double precision
        ) AS cpu_ratio
    FROM review_cases
    CROSS JOIN generate_series(0, 15) AS generated(sample_index)
)
INSERT INTO telemetry_samples (
    id,
    client_id,
    observed_at,
    cpu_utilization_ratio,
    cpu_cores,
    cpu_load_1,
    cpu_load_5,
    cpu_load_15,
    memory_total_bytes,
    memory_available_bytes,
    swap_total_bytes,
    swap_available_bytes,
    disk_total_bytes,
    disk_available_bytes,
    network_rx_bytes,
    network_tx_bytes,
    tcp_sockets,
    udp_sockets,
    payload
)
SELECT
    md5(points.client_id || ':' || points.sample_index::text)::uuid,
    points.client_id,
    points.observed_at,
    points.cpu_ratio,
    points.cpu_cores,
    points.cpu_ratio * points.cpu_cores::double precision,
    points.cpu_ratio * points.cpu_cores::double precision * 0.88,
    points.cpu_ratio * points.cpu_cores::double precision * 0.74,
    8589934592::bigint,
    (
        8589934592::numeric
        * (0.78::numeric - points.cpu_ratio::numeric * 0.20::numeric)
    )::bigint,
    4294967296::bigint,
    3221225472::bigint,
    100000000000::bigint,
    64000000000::bigint - (15 - points.sample_index) * 10000000::bigint,
    CASE WHEN points.network_enabled THEN
        10000000000::bigint + (15 - points.sample_index) * 150000000::bigint
    ELSE 0 END,
    CASE WHEN points.network_enabled THEN
        5000000000::bigint + (15 - points.sample_index) * 60000000::bigint
    ELSE 0 END,
    120,
    28,
    jsonb_build_object(
        'observed_unix', extract(epoch FROM points.observed_at)::bigint,
        'hostname', points.client_id,
        'uptime_secs', points.uptime_base + (15 - points.sample_index) * 60,
        'cpu', jsonb_build_object(
            'cores', points.cpu_cores,
            'utilization_ratio', points.cpu_ratio,
            'load', jsonb_build_object(
                'one', points.cpu_ratio * points.cpu_cores::double precision,
                'five', points.cpu_ratio * points.cpu_cores::double precision * 0.88,
                'fifteen', points.cpu_ratio * points.cpu_cores::double precision * 0.74
            )
        ),
        'memory', jsonb_build_object(
            'total_bytes', 8589934592::bigint,
            'available_bytes', (
                8589934592::numeric
                * (0.78::numeric - points.cpu_ratio::numeric * 0.20::numeric)
            )::bigint,
            'swap_total_bytes', 4294967296::bigint,
            'swap_available_bytes', 3221225472::bigint
        ),
        'disks', jsonb_build_array(
            jsonb_build_object(
                'mountpoint', '/',
                'total_bytes', 100000000000::bigint,
                'available_bytes',
                    64000000000::bigint - (15 - points.sample_index) * 10000000::bigint
            )
        ),
        'networks', CASE
            WHEN points.network_enabled THEN jsonb_build_array(
                jsonb_build_object(
                    'interface', 'eth0',
                    'rx_bytes',
                        10000000000::bigint
                        + (15 - points.sample_index) * 150000000::bigint,
                    'tx_bytes',
                        5000000000::bigint
                        + (15 - points.sample_index) * 60000000::bigint
                )
            )
            ELSE '[]'::jsonb
        END,
        'connections', jsonb_build_object('tcp', 120, 'udp', 28),
        'ping_results', CASE points.ping_mode
            WHEN 'healthy' THEN jsonb_build_array(
                jsonb_build_object(
                    'target_id', '20000000-0000-4000-8000-000000000001',
                    'generation', 1,
                    'checked_unix', extract(epoch FROM points.observed_at)::bigint,
                    'status', 'ok',
                    'latency_avg_ms', 17.5 + ((15 - points.sample_index) % 4),
                    'loss_ratio', 0.0,
                    'reason', NULL
                )
            )
            WHEN 'degraded' THEN jsonb_build_array(
                jsonb_build_object(
                    'target_id', '20000000-0000-4000-8000-000000000002',
                    'generation', 1,
                    'checked_unix', extract(epoch FROM points.observed_at)::bigint,
                    'status', 'degraded',
                    'latency_avg_ms', 68.0 + ((15 - points.sample_index) % 6),
                    'loss_ratio', 0.2,
                    'reason', 'Intermittent packet loss'
                )
            )
            ELSE '[]'::jsonb
        END
    )
FROM points;

INSERT INTO telemetry_counter_facts (
    sample_id, client_id, observed_at, source_kind, ordinal,
    interface, rx_bytes, tx_bytes
)
SELECT
    sample.id,
    sample.client_id,
    sample.observed_at,
    'host',
    0,
    network ->> 'interface',
    (network ->> 'rx_bytes')::bigint,
    (network ->> 'tx_bytes')::bigint
FROM telemetry_samples sample
CROSS JOIN LATERAL jsonb_array_elements(sample.payload -> 'networks') network
WHERE sample.client_id LIKE 'review-%';

INSERT INTO telemetry_ping_series (client_id, target_id, generation)
SELECT DISTINCT
    sample.client_id,
    (result.value ->> 'target_id')::uuid,
    (result.value ->> 'generation')::bigint
FROM telemetry_samples sample
CROSS JOIN LATERAL jsonb_array_elements(sample.payload -> 'ping_results')
    WITH ORDINALITY result(value, ordinal)
WHERE sample.client_id LIKE 'review-%'
ON CONFLICT (client_id, target_id, generation) DO NOTHING;

INSERT INTO telemetry_ping_facts (
    series_id, observed_at, evidence_id, source_checked_unix, checked_unix,
    status, latency_avg_ms, loss_ratio, reason
)
SELECT DISTINCT ON (series.id, (result.value ->> 'checked_unix')::bigint)
    series.id,
    sample.observed_at,
    sample.id,
    (result.value ->> 'checked_unix')::bigint,
    (result.value ->> 'checked_unix')::bigint,
    result.value ->> 'status',
    NULLIF(result.value ->> 'latency_avg_ms', '')::double precision,
    (result.value ->> 'loss_ratio')::double precision,
    result.value ->> 'reason'
FROM telemetry_samples sample
CROSS JOIN LATERAL jsonb_array_elements(sample.payload -> 'ping_results')
    WITH ORDINALITY result(value, ordinal)
JOIN telemetry_ping_series series
  ON series.client_id = sample.client_id
 AND series.target_id = (result.value ->> 'target_id')::uuid
 AND series.generation = (result.value ->> 'generation')::bigint
WHERE sample.client_id LIKE 'review-%'
ORDER BY series.id, (result.value ->> 'checked_unix')::bigint, result.ordinal DESC;

WITH latest AS (
    SELECT DISTINCT ON (fact.series_id)
        fact.series_id,
        fact.checked_unix,
        fact.status,
        fact.latency_avg_ms,
        fact.loss_ratio,
        fact.reason
    FROM telemetry_ping_facts fact
    JOIN telemetry_ping_series series ON series.id = fact.series_id
    WHERE series.client_id LIKE 'review-%'
    ORDER BY
        fact.series_id,
        fact.checked_unix DESC,
        fact.observed_at DESC,
        fact.evidence_id DESC,
        fact.source_checked_unix DESC
), rolling AS (
    SELECT
        latest.series_id,
        avg(fact.loss_ratio)::double precision AS loss_ratio
    FROM latest
    JOIN telemetry_ping_facts fact
      ON fact.series_id = latest.series_id
     AND fact.checked_unix >= latest.checked_unix - 899
     AND fact.checked_unix <= latest.checked_unix
    GROUP BY latest.series_id
)
INSERT INTO telemetry_ping_current (
    series_id,
    latest_status,
    latency_avg_ms,
    rolling_loss_ratio,
    latest_reason,
    latest_checked_at,
    updated_at
)
SELECT
    latest.series_id,
    latest.status,
    latest.latency_avg_ms,
    COALESCE(rolling.loss_ratio, latest.loss_ratio),
    left(latest.reason, 512),
    to_timestamp(latest.checked_unix::double precision),
    now()
FROM latest
JOIN rolling ON rolling.series_id = latest.series_id;

WITH review_cases (
    client_id,
    cpu_offset,
    cpu_cores
) AS (
    VALUES
        ('review-total-monthly', 0.12::double precision, 4),
        ('review-traffic-exceeded', 0.28::double precision, 4),
        ('review-rx-yearly', 0.18::double precision, 8),
        ('review-tx-unlimited', 0.08::double precision, 2),
        ('review-no-reset', 0.22::double precision, 16),
        ('review-empty-rates', 0.05::double precision, 2),
        ('review-unconfigured', 0.10::double precision, 4),
        ('review-no-primary', 0.16::double precision, 6)
), points AS (
    SELECT
        review_cases.*,
        sample_index,
        date_trunc('minute', now()) - sample_index * interval '1 minute' AS bucket_start,
        least(
            0.92::double precision,
            cpu_offset + ((15 - sample_index) % 5) * 0.025::double precision
        ) AS cpu_ratio
    FROM review_cases
    CROSS JOIN generate_series(0, 15) AS generated(sample_index)
)
INSERT INTO telemetry_rollups (
    client_id,
    bucket_start,
    bucket_secs,
    sample_count,
    cpu_usage_sample_count,
    cpu_usage_sum,
    cpu_usage_avg,
    cpu_usage_max,
    cpu_cores_max,
    cpu_load_1_avg,
    cpu_load_1_sum,
    cpu_load_1_max,
    cpu_load_5_avg,
    cpu_load_5_sum,
    cpu_load_5_max,
    cpu_load_15_avg,
    cpu_load_15_sum,
    cpu_load_15_max,
    memory_total_bytes_max,
    memory_available_bytes_avg,
    memory_available_bytes_sum,
    memory_available_bytes_min,
    memory_used_ratio_avg,
    memory_used_ratio_sum,
    memory_used_ratio_max,
    swap_sample_count,
    swap_total_bytes_max,
    swap_available_bytes_avg,
    swap_available_bytes_sum,
    swap_available_bytes_min,
    swap_used_ratio_avg,
    swap_used_ratio_sum,
    swap_used_ratio_max,
    disk_total_bytes_max,
    disk_available_bytes_avg,
    disk_available_bytes_sum,
    disk_available_bytes_min,
    disk_used_ratio_avg,
    disk_used_ratio_sum,
    disk_used_ratio_max,
    network_rx_bytes_max,
    network_tx_bytes_max,
    connections_sample_count,
    tcp_sockets_latest,
    udp_sockets_latest,
    connections_observed_at,
    latest_observed_at,
    updated_at
)
SELECT
    points.client_id,
    points.bucket_start,
    60,
    1,
    1,
    points.cpu_ratio,
    points.cpu_ratio,
    points.cpu_ratio,
    points.cpu_cores,
    points.cpu_ratio * points.cpu_cores::double precision,
    points.cpu_ratio * points.cpu_cores::double precision,
    points.cpu_ratio * points.cpu_cores::double precision,
    points.cpu_ratio * points.cpu_cores::double precision * 0.88,
    points.cpu_ratio * points.cpu_cores::double precision * 0.88,
    points.cpu_ratio * points.cpu_cores::double precision * 0.88,
    points.cpu_ratio * points.cpu_cores::double precision * 0.74,
    points.cpu_ratio * points.cpu_cores::double precision * 0.74,
    points.cpu_ratio * points.cpu_cores::double precision * 0.74,
    8589934592::bigint,
    (
        8589934592::numeric
        * (0.78::numeric - points.cpu_ratio::numeric * 0.20::numeric)
    )::bigint,
    (
        8589934592::numeric
        * (0.78::numeric - points.cpu_ratio::numeric * 0.20::numeric)
    ),
    (
        8589934592::numeric
        * (0.78::numeric - points.cpu_ratio::numeric * 0.20::numeric)
    )::bigint,
    0.22::double precision + points.cpu_ratio * 0.20::double precision,
    0.22::double precision + points.cpu_ratio * 0.20::double precision,
    0.22::double precision + points.cpu_ratio * 0.20::double precision,
    1,
    4294967296::bigint,
    3221225472::bigint,
    3221225472::numeric,
    3221225472::bigint,
    0.25::double precision,
    0.25::double precision,
    0.25::double precision,
    100000000000::bigint,
    64000000000::bigint - (15 - points.sample_index) * 10000000::bigint,
    64000000000::numeric - (15 - points.sample_index) * 10000000::numeric,
    64000000000::bigint - (15 - points.sample_index) * 10000000::bigint,
    0.36::double precision + (15 - points.sample_index) * 0.0001::double precision,
    0.36::double precision + (15 - points.sample_index) * 0.0001::double precision,
    0.36::double precision + (15 - points.sample_index) * 0.0001::double precision,
    10000000000::bigint + (15 - points.sample_index) * 150000000::bigint,
    5000000000::bigint + (15 - points.sample_index) * 60000000::bigint,
    1,
    120,
    28,
    points.bucket_start,
    points.bucket_start,
    now()
FROM points;

INSERT INTO telemetry_resource_latest
SELECT DISTINCT ON (rollup.client_id) rollup.*
FROM telemetry_rollups rollup
WHERE rollup.client_id LIKE 'review-%'
ORDER BY
    rollup.client_id,
    rollup.latest_observed_at DESC,
    rollup.bucket_start DESC;

WITH rate_cases (client_id, rx_delta, tx_delta) AS (
    VALUES
        ('review-total-monthly', 150000000::bigint, 60000000::bigint),
        ('review-traffic-exceeded', 110000000::bigint, 85000000::bigint),
        ('review-rx-yearly', 90000000::bigint, 45000000::bigint),
        ('review-tx-unlimited', 55000000::bigint, 140000000::bigint),
        ('review-no-reset', 210000000::bigint, 100000000::bigint),
        ('review-no-primary', 70000000::bigint, 35000000::bigint)
), points AS (
    SELECT
        rate_cases.*,
        sample_index,
        date_trunc('minute', now()) - sample_index * interval '1 minute' AS bucket_start,
        15 - sample_index AS elapsed_minutes
    FROM rate_cases
    CROSS JOIN generate_series(0, 15) AS generated(sample_index)
)
INSERT INTO telemetry_network_rates (
    client_id,
    interface,
    bucket_start,
    bucket_secs,
    sample_count,
    rx_bytes_sum,
    tx_bytes_sum,
    rx_bytes_avg,
    tx_bytes_avg,
    rx_bytes_last,
    tx_bytes_last,
    rx_counter_epoch,
    tx_counter_epoch,
    latest_observed_at,
    updated_at
)
SELECT
    points.client_id,
    'eth0',
    points.bucket_start,
    60,
    1,
    10000000000::numeric + points.elapsed_minutes * points.rx_delta,
    5000000000::numeric + points.elapsed_minutes * points.tx_delta,
    10000000000::bigint + points.elapsed_minutes * points.rx_delta,
    5000000000::bigint + points.elapsed_minutes * points.tx_delta,
    10000000000::bigint + points.elapsed_minutes * points.rx_delta,
    5000000000::bigint + points.elapsed_minutes * points.tx_delta,
    0,
    0,
    points.bucket_start,
    now()
FROM points;

INSERT INTO traffic_counter_samples (
    client_id,
    source_kind,
    interface,
    observed_at,
    rx_bytes,
    tx_bytes,
    rx_counter_epoch,
    tx_counter_epoch,
    sample_source
)
VALUES
    (
        'review-total-monthly', 'host', 'eth0',
        date_trunc('minute', now()) - interval '20 minutes',
        10000000000, 5000000000, 0, 0, 'agent'
    ),
    (
        'review-total-monthly', 'host', 'eth0',
        date_trunc('minute', now()),
        30000000000, 15000000000, 0, 0, 'agent'
    ),
    (
        'review-traffic-exceeded', 'host', 'eth0',
        date_trunc('minute', now()) - interval '20 minutes',
        1000000000, 1000000000, 0, 0, 'agent'
    ),
    (
        'review-traffic-exceeded', 'host', 'eth0',
        date_trunc('minute', now()),
        8000000000, 6000000000, 0, 0, 'agent'
    ),
    (
        'review-rx-yearly', 'host', 'eth0',
        date_trunc('minute', now()) - interval '20 minutes',
        5000000000, 2000000000, 0, 0, 'agent'
    ),
    (
        'review-rx-yearly', 'host', 'eth0',
        date_trunc('minute', now()),
        27000000000, 11000000000, 0, 0, 'agent'
    ),
    (
        'review-tx-unlimited', 'host', 'eth0',
        date_trunc('minute', now()) - interval '20 minutes',
        7000000000, 3000000000, 0, 0, 'agent'
    ),
    (
        'review-tx-unlimited', 'host', 'eth0',
        date_trunc('minute', now()),
        15000000000, 28000000000, 0, 0, 'agent'
    ),
    (
        'review-no-reset', 'host', 'eth0',
        '2020-01-01 00:00:00+00',
        1000000000, 2000000000, 0, 0, 'vnstat_import:review-2020'
    ),
    (
        'review-no-reset', 'host', 'eth0',
        '2022-01-01 00:00:00+00',
        21000000000, 12000000000, 0, 0, 'vnstat_import:review-2022'
    ),
    (
        'review-no-reset', 'host', 'eth0',
        date_trunc('minute', now()) - interval '20 minutes',
        5000000000, 3000000000, 1, 1, 'agent'
    ),
    (
        'review-no-reset', 'host', 'eth0',
        date_trunc('minute', now()),
        65000000000, 33000000000, 1, 1, 'agent'
    ),
    (
        'review-empty-rates', 'tunnel', 'wg0',
        date_trunc('minute', now()) - interval '20 minutes',
        1000000000, 1000000000, 0, 0, 'agent'
    ),
    (
        'review-empty-rates', 'tunnel', 'wg0',
        date_trunc('minute', now()),
        3000000000, 5000000000, 0, 0, 'agent'
    ),
    (
        'review-no-primary', 'host', 'eth0',
        date_trunc('minute', now()) - interval '20 minutes',
        4000000000, 2000000000, 0, 0, 'agent'
    ),
    (
        'review-no-primary', 'host', 'eth0',
        date_trunc('minute', now()),
        16000000000, 8000000000, 0, 0, 'agent'
    );

WITH ping_cases (
    client_id,
    target_id,
    latency,
    success_count,
    loss_ratio,
    status,
    reason
) AS (
    VALUES
        (
            'review-total-monthly',
            '20000000-0000-4000-8000-000000000001'::uuid,
            18.5::double precision,
            10,
            0.0::double precision,
            'ok',
            NULL::text
        ),
        (
            'review-traffic-exceeded',
            '20000000-0000-4000-8000-000000000001'::uuid,
            22.0::double precision,
            10,
            0.0::double precision,
            'ok',
            NULL::text
        ),
        (
            'review-rx-yearly',
            '20000000-0000-4000-8000-000000000002'::uuid,
            72.0::double precision,
            8,
            0.2::double precision,
            'degraded',
            'Intermittent packet loss'
        )
), ensured_series AS (
    INSERT INTO telemetry_ping_series (client_id, target_id, generation)
    SELECT DISTINCT client_id, target_id, 1 FROM ping_cases
    ON CONFLICT (client_id, target_id, generation) DO UPDATE
        SET generation = EXCLUDED.generation
    RETURNING id, client_id, target_id, generation
), points AS (
    SELECT
        ping_cases.*,
        sample_index,
        date_trunc('minute', now()) - sample_index * interval '1 minute' AS bucket_start
    FROM ping_cases
    CROSS JOIN generate_series(0, 15) AS generated(sample_index)
)
INSERT INTO telemetry_ping_rollups (
    series_id,
    bucket_start,
    bucket_secs,
    sample_count,
    success_count,
    latency_sum_ms,
    latency_avg_ms,
    latency_min_ms,
    latency_max_ms,
    loss_ratio_avg,
    loss_ratio_sum,
    loss_ratio_max,
    latest_status,
    latest_reason,
    latest_checked_at,
    updated_at
)
SELECT
    series.id,
    points.bucket_start,
    60,
    10,
    points.success_count,
    (points.latency + ((15 - points.sample_index) % 4)) * points.success_count,
    points.latency + ((15 - points.sample_index) % 4),
    points.latency - 2,
    points.latency + 4,
    points.loss_ratio,
    points.loss_ratio * 10,
    points.loss_ratio,
    points.status,
    points.reason,
    points.bucket_start,
    now()
FROM points
JOIN ensured_series series
  ON series.client_id = points.client_id
 AND series.target_id = points.target_id
 AND series.generation = 1;

COMMIT;
