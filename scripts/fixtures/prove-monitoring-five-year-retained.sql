\set ON_ERROR_STOP on
SET TIME ZONE 'UTC';

-- Opt-in, storage-backed pressure data for prove-vnstat-browser-pressure.sh.
--
-- This fixture deliberately does not own clients or traffic-counter history.
-- review-monitoring-pressure-128.sql creates the 120 pressure-* clients and the
-- ignored Rust import proof owns their five-year vnStat replacement. This file
-- replaces only the other monitoring-history families with a production-tiered
-- retained shape. Reapplying it is safe after a partial fixture attempt.

SELECT pg_advisory_lock(1448104781, 5);

CREATE TEMP TABLE pressure_retained_config ON COMMIT PRESERVE ROWS AS
WITH frozen AS (
    SELECT date_trunc('minute', clock_timestamp()) AS history_end
)
SELECT
    history_end,
    date_trunc('day', history_end - interval '1825 days') AS history_start,
    history_end + interval '2 hours' AS maintenance_safe_until
FROM frozen;

CREATE TEMP TABLE pressure_retained_ranges (
    ordinal INTEGER PRIMARY KEY,
    bucket_secs INTEGER UNIQUE NOT NULL,
    range_start TIMESTAMPTZ NOT NULL,
    range_end TIMESTAMPTZ NOT NULL,
    expected_rows BIGINT NOT NULL
) ON COMMIT PRESERVE ROWS;

WITH boundaries AS (
    SELECT
        config.history_start,
        config.history_end,
        to_timestamp(floor(extract(epoch FROM
            config.maintenance_safe_until - interval '2 days') / 300) * 300) AS b_60,
        to_timestamp(floor(extract(epoch FROM
            config.maintenance_safe_until - interval '8 days') / 1800) * 1800) AS b_300,
        to_timestamp(floor(extract(epoch FROM
            config.maintenance_safe_until - interval '31 days') / 3600) * 3600) AS b_1800,
        to_timestamp(floor(extract(epoch FROM
            config.maintenance_safe_until - interval '91 days') / 10800) * 10800) AS b_3600,
        to_timestamp(floor(extract(epoch FROM
            config.maintenance_safe_until - interval '181 days') / 21600) * 21600) AS b_10800,
        to_timestamp(floor(extract(epoch FROM
            config.maintenance_safe_until - interval '366 days') / 86400) * 86400) AS b_21600
    FROM pressure_retained_config config
), ranges(ordinal, bucket_secs, range_start, range_end) AS (
    SELECT 1, 60, b_60, history_end FROM boundaries
    UNION ALL SELECT 2, 300, b_300, b_60 FROM boundaries
    UNION ALL SELECT 3, 1800, b_1800, b_300 FROM boundaries
    UNION ALL SELECT 4, 3600, b_3600, b_1800 FROM boundaries
    UNION ALL SELECT 5, 10800, b_10800, b_3600 FROM boundaries
    UNION ALL SELECT 6, 21600, b_21600, b_10800 FROM boundaries
    UNION ALL SELECT 7, 86400, history_start, b_21600 FROM boundaries
)
INSERT INTO pressure_retained_ranges (
    ordinal, bucket_secs, range_start, range_end, expected_rows
)
SELECT
    ordinal,
    bucket_secs,
    range_start,
    range_end,
    (extract(epoch FROM range_end - range_start)::bigint / bucket_secs)::bigint
FROM ranges;

DO $$
BEGIN
    IF (SELECT count(*) FROM clients WHERE id LIKE 'pressure-%') <> 120
       OR (SELECT count(*) FROM clients) <> 128 THEN
        RAISE EXCEPTION
            'retained-history fixture requires the isolated 120+8 pressure scope';
    END IF;
    IF (SELECT count(*) FROM pressure_retained_ranges) <> 7
       OR EXISTS (
            SELECT 1
            FROM pressure_retained_ranges range
            WHERE range.range_start >= range.range_end
               OR extract(epoch FROM range.range_start)::bigint % range.bucket_secs <> 0
               OR extract(epoch FROM range.range_end - range.range_start)::bigint
                    % range.bucket_secs <> 0
               OR range.expected_rows <= 0
       ) THEN
        RAISE EXCEPTION 'retained-history tier boundaries are invalid';
    END IF;
    IF EXISTS (
        SELECT 1
        FROM (
            SELECT
                range.*,
                lag(range.range_end) OVER (ORDER BY range.ordinal DESC) AS older_end
            FROM pressure_retained_ranges range
        ) ordered
        WHERE ordered.older_end IS NOT NULL
          AND ordered.older_end <> ordered.range_start
    ) THEN
        RAISE EXCEPTION 'retained-history tier boundaries overlap or have a gap';
    END IF;
    IF (
        SELECT extract(epoch FROM history_end - history_start)::bigint / 60
        FROM pressure_retained_config
    ) NOT BETWEEN 2628000 AND 2629439 THEN
        RAISE EXCEPTION 'retained-history horizon is not five full years';
    END IF;
END;
$$;

-- Remove only identities owned by this supplemental fixture. Traffic history,
-- import jobs, outputs, hourly ledgers, and the pressure client rows survive.
BEGIN;
DELETE FROM network_observations observation
USING tunnel_plans plan
WHERE observation.plan_id = plan.id
  AND plan.name LIKE 'pressure-history-plan-%';
DELETE FROM tunnel_plans WHERE name LIKE 'pressure-history-plan-%';
DELETE FROM telemetry_ping_series WHERE client_id LIKE 'pressure-%';
DELETE FROM ping_targets WHERE id = '90000000-0000-4000-8000-000000000001';
DELETE FROM telemetry_resource_latest WHERE client_id LIKE 'pressure-%';
DELETE FROM telemetry_network_rates WHERE client_id LIKE 'pressure-%';
DELETE FROM telemetry_rollups WHERE client_id LIKE 'pressure-%';
DELETE FROM telemetry_samples WHERE client_id LIKE 'pressure-%';
DELETE FROM system_metric_rollups WHERE metric LIKE 'pressure.%';
COMMIT;

-- Seven exact days of raw resource samples and one host counter fact per sample.
BEGIN;
WITH pressure_clients AS (
    SELECT
        client.id AS client_id,
        substring(client.id FROM '[0-9]+$')::bigint AS client_number
    FROM clients client
    WHERE client.id LIKE 'pressure-%'
), raw_points AS (
    SELECT
        pressure.client_id,
        pressure.client_number,
        point,
        config.history_end - interval '7 days'
            + point * interval '1 minute' AS observed_at
    FROM pressure_clients pressure
    CROSS JOIN pressure_retained_config config
    CROSS JOIN generate_series(0, 10079) generated(point)
)
INSERT INTO telemetry_samples (
    id, client_id, observed_at, cpu_utilization_ratio, cpu_cores,
    cpu_load_1, cpu_load_5, cpu_load_15,
    memory_total_bytes, memory_available_bytes,
    swap_total_bytes, swap_available_bytes,
    disk_total_bytes, disk_available_bytes,
    network_rx_bytes, network_tx_bytes, tcp_sockets, udp_sockets, payload
)
SELECT
    md5('pressure-history-raw:' || point.client_id || ':' || point.point)::uuid,
    point.client_id,
    point.observed_at,
    0.20::double precision + (point.point % 17)::double precision / 100.0,
    4,
    0.80::double precision + (point.point % 11)::double precision / 100.0,
    0.70::double precision + (point.point % 7)::double precision / 100.0,
    0.60::double precision + (point.point % 5)::double precision / 100.0,
    8589934592::bigint,
    6442450944::bigint - (point.point % 128) * 1048576::bigint,
    2147483648::bigint,
    1610612736::bigint,
    100000000000::bigint,
    64000000000::bigint - (point.point % 256) * 1000000::bigint,
    point.client_number * 100000000000::bigint
        + point.point * 150000000::bigint,
    point.client_number * 50000000000::bigint
        + point.point * 60000000::bigint,
    120 + point.point % 19,
    28 + point.point % 7,
    jsonb_build_object(
        'observed_unix', extract(epoch FROM point.observed_at)::bigint,
        'hostname', point.client_id,
        'uptime_secs', 31536000 + point.point * 60,
        'cpu', jsonb_build_object(
            'cores', 4,
            'utilization_ratio',
                0.20::double precision + (point.point % 17)::double precision / 100.0,
            'load', jsonb_build_object('one', 0.8, 'five', 0.7, 'fifteen', 0.6)
        ),
        'memory', jsonb_build_object(
            'total_bytes', 8589934592::bigint,
            'available_bytes',
                6442450944::bigint - (point.point % 128) * 1048576::bigint,
            'swap_total_bytes', 2147483648::bigint,
            'swap_available_bytes', 1610612736::bigint
        ),
        'disks', jsonb_build_array(jsonb_build_object(
            'mountpoint', '/',
            'total_bytes', 100000000000::bigint,
            'available_bytes',
                64000000000::bigint - (point.point % 256) * 1000000::bigint
        )),
        'disk_collection_available', true,
        'disk_semantics', 'persistent_block_filesystems_v1',
        'networks', jsonb_build_array(jsonb_build_object(
            'interface', 'eth0',
            'rx_bytes', point.client_number * 100000000000::bigint
                + point.point * 150000000::bigint,
            'tx_bytes', point.client_number * 50000000000::bigint
                + point.point * 60000000::bigint
        )),
        'connections', jsonb_build_object(
            'tcp', 120 + point.point % 19,
            'udp', 28 + point.point % 7
        ),
        'ping_results', jsonb_build_array(jsonb_build_object(
            'target_id', '90000000-0000-4000-8000-000000000001',
            'generation', 1,
            'checked_unix', extract(epoch FROM point.observed_at)::bigint,
            'status', 'ok',
            'latency_avg_ms', 12.0 + (point.point % 9)::double precision / 10.0,
            'loss_ratio', 0.0
        ))
    )
FROM raw_points point;

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
    'eth0',
    sample.network_rx_bytes,
    sample.network_tx_bytes
FROM telemetry_samples sample
WHERE sample.client_id LIKE 'pressure-%';
COMMIT;

-- Every resource and network-rate stream covers one continuous retained
-- interval. The sample_count weights conserve the represented source minutes
-- even though old buckets use coarser storage.
BEGIN;
WITH pressure_clients AS (
    SELECT
        client.id AS client_id,
        substring(client.id FROM '[0-9]+$')::bigint AS client_number
    FROM clients client
    WHERE client.id LIKE 'pressure-%'
), buckets AS (
    SELECT
        range.bucket_secs,
        generated.bucket_start,
        range.bucket_secs / 60 AS represented_minutes
    FROM pressure_retained_ranges range
    CROSS JOIN LATERAL generate_series(
        range.range_start,
        range.range_end - make_interval(secs => range.bucket_secs),
        make_interval(secs => range.bucket_secs)
    ) generated(bucket_start)
)
INSERT INTO telemetry_rollups (
    client_id, bucket_start, bucket_secs, sample_count,
    cpu_usage_sample_count, cpu_usage_sum, cpu_usage_avg, cpu_usage_max,
    cpu_cores_max, cpu_load_1_avg, cpu_load_1_sum, cpu_load_1_max,
    cpu_load_5_avg, cpu_load_5_sum, cpu_load_5_max,
    cpu_load_15_avg, cpu_load_15_sum, cpu_load_15_max,
    memory_total_bytes_max, memory_available_bytes_avg,
    memory_available_bytes_sum, memory_available_bytes_min,
    memory_used_ratio_avg, memory_used_ratio_sum, memory_used_ratio_max,
    swap_sample_count, swap_total_bytes_max, swap_available_bytes_avg,
    swap_available_bytes_sum, swap_available_bytes_min,
    swap_used_ratio_avg, swap_used_ratio_sum, swap_used_ratio_max,
    disk_total_bytes_max, disk_available_bytes_avg,
    disk_available_bytes_sum, disk_available_bytes_min,
    disk_used_ratio_avg, disk_used_ratio_sum, disk_used_ratio_max,
    network_rx_bytes_max, network_tx_bytes_max,
    connections_sample_count, tcp_sockets_latest, udp_sockets_latest,
    connections_observed_at, latest_observed_at, updated_at, disk_sample_count
)
SELECT
    pressure.client_id,
    bucket.bucket_start,
    bucket.bucket_secs,
    bucket.represented_minutes,
    bucket.represented_minutes,
    0.25::double precision * bucket.represented_minutes,
    0.25,
    0.42,
    4,
    1.0,
    1.0::double precision * bucket.represented_minutes,
    1.4,
    0.8,
    0.8::double precision * bucket.represented_minutes,
    1.2,
    0.6,
    0.6::double precision * bucket.represented_minutes,
    0.9,
    8589934592::bigint,
    6442450944::bigint,
    6442450944::numeric * bucket.represented_minutes,
    6308233216::bigint,
    0.25,
    0.25::double precision * bucket.represented_minutes,
    0.27,
    bucket.represented_minutes,
    2147483648::bigint,
    1610612736::bigint,
    1610612736::numeric * bucket.represented_minutes,
    1577058304::bigint,
    0.25,
    0.25::double precision * bucket.represented_minutes,
    0.27,
    100000000000::bigint,
    64000000000::bigint,
    64000000000::numeric * bucket.represented_minutes,
    63500000000::bigint,
    0.36,
    0.36::double precision * bucket.represented_minutes,
    0.365,
    pressure.client_number * 100000000000::bigint
        + extract(epoch FROM bucket.bucket_start)::bigint * 1000,
    pressure.client_number * 50000000000::bigint
        + extract(epoch FROM bucket.bucket_start)::bigint * 500,
    bucket.represented_minutes,
    128,
    32,
    bucket.bucket_start + make_interval(secs => bucket.bucket_secs - 60),
    bucket.bucket_start + make_interval(secs => bucket.bucket_secs - 60),
    now(),
    bucket.represented_minutes
FROM pressure_clients pressure
CROSS JOIN buckets bucket;

INSERT INTO telemetry_resource_latest
SELECT DISTINCT ON (rollup.client_id) rollup.*
FROM telemetry_rollups rollup
WHERE rollup.client_id LIKE 'pressure-%'
ORDER BY rollup.client_id, rollup.latest_observed_at DESC, rollup.bucket_start DESC;

WITH pressure_clients AS (
    SELECT
        client.id AS client_id,
        substring(client.id FROM '[0-9]+$')::bigint AS client_number
    FROM clients client
    WHERE client.id LIKE 'pressure-%'
), buckets AS (
    SELECT
        range.bucket_secs,
        generated.bucket_start,
        range.bucket_secs / 60 AS represented_minutes
    FROM pressure_retained_ranges range
    CROSS JOIN LATERAL generate_series(
        range.range_start,
        range.range_end - make_interval(secs => range.bucket_secs),
        make_interval(secs => range.bucket_secs)
    ) generated(bucket_start)
)
INSERT INTO telemetry_network_rates (
    client_id, interface, bucket_start, bucket_secs, sample_count,
    rx_bytes_sum, tx_bytes_sum, rx_bytes_avg, tx_bytes_avg,
    rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch,
    latest_observed_at, updated_at
)
SELECT
    pressure.client_id,
    'eth0',
    bucket.bucket_start,
    bucket.bucket_secs,
    bucket.represented_minutes,
    2500000::numeric * bucket.represented_minutes,
    1000000::numeric * bucket.represented_minutes,
    2500000,
    1000000,
    pressure.client_number * 100000000000::bigint
        + extract(epoch FROM bucket.bucket_start)::bigint * 2500000,
    pressure.client_number * 50000000000::bigint
        + extract(epoch FROM bucket.bucket_start)::bigint * 1000000,
    0,
    0,
    bucket.bucket_start + make_interval(secs => bucket.bucket_secs - 60),
    now()
FROM pressure_clients pressure
CROSS JOIN buckets bucket;
COMMIT;

-- One enabled primary Ping target, one series per pressure client, seven days
-- of exact checks, an exact current row, and the same seven retained tiers.
BEGIN;
INSERT INTO ping_targets (
    id, name, host, probe_kind, enabled, selector_expression, generation
)
VALUES (
    '90000000-0000-4000-8000-000000000001',
    'Pressure retained-history Ping',
    '1.1.1.1',
    'icmp',
    true,
    'id:pressure-*',
    1
);

INSERT INTO ping_target_assignments (target_id, client_id, is_primary)
SELECT
    '90000000-0000-4000-8000-000000000001',
    client.id,
    true
FROM clients client
WHERE client.id LIKE 'pressure-%';

INSERT INTO telemetry_ping_series (client_id, target_id, generation)
SELECT
    client.id,
    '90000000-0000-4000-8000-000000000001',
    1
FROM clients client
WHERE client.id LIKE 'pressure-%';

WITH raw_points AS (
    SELECT
        series.id AS series_id,
        point,
        config.history_end - interval '7 days'
            + point * interval '1 minute' AS observed_at
    FROM telemetry_ping_series series
    JOIN clients client ON client.id = series.client_id
    CROSS JOIN pressure_retained_config config
    CROSS JOIN generate_series(0, 10079) generated(point)
    WHERE client.id LIKE 'pressure-%'
)
INSERT INTO telemetry_ping_facts (
    series_id, observed_at, evidence_id, source_checked_unix, checked_unix,
    status, latency_avg_ms, loss_ratio, reason
)
SELECT
    point.series_id,
    point.observed_at,
    md5('pressure-history-ping:' || point.series_id || ':' || point.point)::uuid,
    extract(epoch FROM point.observed_at)::bigint,
    extract(epoch FROM point.observed_at)::bigint,
    'ok',
    12.0::double precision + (point.point % 9)::double precision / 10.0,
    0.0,
    NULL
FROM raw_points point;

WITH buckets AS (
    SELECT
        range.bucket_secs,
        generated.bucket_start,
        range.bucket_secs / 60 AS represented_minutes
    FROM pressure_retained_ranges range
    CROSS JOIN LATERAL generate_series(
        range.range_start,
        range.range_end - make_interval(secs => range.bucket_secs),
        make_interval(secs => range.bucket_secs)
    ) generated(bucket_start)
)
INSERT INTO telemetry_ping_rollups (
    series_id, bucket_start, bucket_secs, sample_count, success_count,
    latency_sum_ms, latency_avg_ms, latency_min_ms, latency_max_ms,
    loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
    latest_status, latest_reason, latest_checked_at, updated_at
)
SELECT
    series.id,
    bucket.bucket_start,
    bucket.bucket_secs,
    bucket.represented_minutes,
    bucket.represented_minutes,
    12.5::double precision * bucket.represented_minutes,
    12.5,
    12.0,
    13.0,
    0.0,
    0.0,
    0.0,
    'ok',
    NULL,
    bucket.bucket_start + make_interval(secs => bucket.bucket_secs - 60),
    now()
FROM telemetry_ping_series series
JOIN clients client ON client.id = series.client_id
CROSS JOIN buckets bucket
WHERE client.id LIKE 'pressure-%';

INSERT INTO telemetry_ping_current (
    series_id, latest_status, latency_avg_ms, rolling_loss_ratio,
    latest_reason, latest_checked_at, updated_at
)
SELECT
    series.id,
    'ok',
    12.5,
    0.0,
    NULL,
    config.history_end - interval '1 minute',
    now()
FROM telemetry_ping_series series
JOIN clients client ON client.id = series.client_id
CROSS JOIN pressure_retained_config config
WHERE client.id LIKE 'pressure-%';
COMMIT;

-- Pair the 120 clients into 60 deterministic tunnel plans and retain both
-- endpoint series. Exact automatic observations use their native five-minute
-- cadence; older evidence uses every configured observation rollup tier.
BEGIN;
WITH plan_pairs AS (
    SELECT
        pair,
        'pressure-' || lpad((pair * 2 - 1)::text, 3, '0') AS left_client_id,
        'pressure-' || lpad((pair * 2)::text, 3, '0') AS right_client_id
    FROM generate_series(1, 60) generated(pair)
)
INSERT INTO tunnel_plans (
    id, name, kind, enabled, left_client_id, right_client_id, input, plan
)
SELECT
    md5('pressure-history-plan:' || pair.pair)::uuid,
    'pressure-history-plan-' || lpad(pair.pair::text, 3, '0'),
    'wireguard',
    true,
    pair.left_client_id,
    pair.right_client_id,
    '{}'::jsonb,
    '{}'::jsonb
FROM plan_pairs pair;

WITH endpoints AS (
    SELECT
        plan.id AS plan_id,
        plan.name AS plan_name,
        plan.left_client_id AS client_id,
        plan.right_client_id AS peer_client_id,
        'left'::text AS endpoint_side,
        '10.250.0.2'::text AS target
    FROM tunnel_plans plan
    WHERE plan.name LIKE 'pressure-history-plan-%'
    UNION ALL
    SELECT
        plan.id,
        plan.name,
        plan.right_client_id,
        plan.left_client_id,
        'right',
        '10.250.0.1'
    FROM tunnel_plans plan
    WHERE plan.name LIKE 'pressure-history-plan-%'
)
INSERT INTO network_observation_series (
    plan_id, topology_identity_hash, plan_name, interface_name,
    client_id, peer_client_id, endpoint_side, address_family, target,
    active, last_seen_at
)
SELECT
    endpoint.plan_id,
    md5(endpoint.plan_id::text || ':a') || md5(endpoint.plan_id::text || ':b'),
    endpoint.plan_name,
    'wg-pressure',
    endpoint.client_id,
    endpoint.peer_client_id,
    endpoint.endpoint_side,
    'ipv4',
    endpoint.target,
    true,
    config.history_end
FROM endpoints endpoint
CROSS JOIN pressure_retained_config config;

WITH exact_range AS (
    SELECT range.range_start, config.history_end
    FROM pressure_retained_ranges range
    CROSS JOIN pressure_retained_config config
    WHERE range.bucket_secs = 60
), exact_points AS (
    SELECT
        series.id AS series_id,
        series.client_id,
        series.peer_client_id,
        series.plan_id,
        series.topology_identity_hash,
        series.plan_name,
        series.interface_name,
        series.target,
        series.endpoint_side,
        point,
        generated.observed_at
    FROM network_observation_series series
    JOIN tunnel_plans plan ON plan.id = series.plan_id
    CROSS JOIN exact_range range
    CROSS JOIN LATERAL generate_series(
        range.range_start,
        range.history_end - interval '5 minutes',
        interval '5 minutes'
    ) WITH ORDINALITY generated(observed_at, point)
    WHERE plan.name LIKE 'pressure-history-plan-%'
)
INSERT INTO network_observations (
    id, client_id, kind, role, plan_id, topology_identity_hash,
    plan_name, interface_name, peer_client_id, target,
    endpoint_side, address_family, stale_after_secs,
    healthy, transmitted, received,
    latency_min_ms, latency_avg_ms, latency_max_ms, latency_mdev_ms,
    packet_loss_ratio, reason, source, automatic_series_id,
    metadata, observed_at, received_at
)
SELECT
    md5('pressure-history-observation:' || point.series_id || ':' || point.point)::uuid,
    point.client_id,
    'tunnel_reachability',
    'endpoint',
    point.plan_id,
    point.topology_identity_hash,
    point.plan_name,
    point.interface_name,
    point.peer_client_id,
    point.target,
    point.endpoint_side,
    'ipv4',
    360,
    true,
    5,
    5,
    8.0,
    8.5::double precision + (point.point % 5)::double precision / 10.0,
    9.0,
    0.2,
    0.0,
    NULL,
    'automatic',
    point.series_id,
    jsonb_build_object('fixture', 'five_year_retained_v1'),
    point.observed_at,
    point.observed_at
FROM exact_points point;

INSERT INTO network_observation_latest (
    series_id, observation_id, stale_after_secs, healthy,
    transmitted, received,
    latency_min_ms, latency_avg_ms, latency_max_ms, latency_mdev_ms,
    packet_loss_ratio, reason, metadata, observed_at, received_at, updated_at
)
SELECT DISTINCT ON (observation.automatic_series_id)
    observation.automatic_series_id,
    observation.id,
    observation.stale_after_secs,
    observation.healthy,
    observation.transmitted,
    observation.received,
    observation.latency_min_ms,
    observation.latency_avg_ms,
    observation.latency_max_ms,
    observation.latency_mdev_ms,
    observation.packet_loss_ratio,
    observation.reason,
    observation.metadata,
    observation.observed_at,
    observation.received_at,
    now()
FROM network_observations observation
JOIN tunnel_plans plan ON plan.id = observation.plan_id
WHERE plan.name LIKE 'pressure-history-plan-%'
ORDER BY
    observation.automatic_series_id,
    observation.observed_at DESC,
    observation.id DESC;

WITH buckets AS (
    SELECT
        range.bucket_secs,
        generated.bucket_start,
        range.bucket_secs / 300 AS represented_checks
    FROM pressure_retained_ranges range
    CROSS JOIN LATERAL generate_series(
        range.range_start,
        range.range_end - make_interval(secs => range.bucket_secs),
        make_interval(secs => range.bucket_secs)
    ) generated(bucket_start)
    WHERE range.bucket_secs >= 300
)
INSERT INTO network_observation_rollups (
    series_id, bucket_secs, bucket_start, health_state, reason_key,
    sample_count,
    transmitted_total, transmitted_sample_count,
    received_total, received_sample_count,
    latency_sum_ms, latency_sample_count, latency_min_ms, latency_max_ms,
    latency_mdev_sum_ms, latency_mdev_sample_count,
    packet_loss_sum_ratio, packet_loss_sample_count,
    packet_loss_min_ratio, packet_loss_max_ratio,
    latest_observation_id, latest_stale_after_secs, latest_healthy,
    latest_transmitted, latest_received,
    latest_latency_min_ms, latest_latency_avg_ms,
    latest_latency_max_ms, latest_latency_mdev_ms,
    latest_packet_loss_ratio, latest_reason,
    latest_observed_at, latest_received_at, updated_at
)
SELECT
    series.id,
    bucket.bucket_secs,
    bucket.bucket_start,
    1,
    '',
    bucket.represented_checks,
    5::numeric * bucket.represented_checks,
    bucket.represented_checks,
    5::numeric * bucket.represented_checks,
    bucket.represented_checks,
    8.5::double precision * bucket.represented_checks,
    bucket.represented_checks,
    8.0,
    9.0,
    0.2::double precision * bucket.represented_checks,
    bucket.represented_checks,
    0.0,
    bucket.represented_checks,
    0.0,
    0.0,
    md5('pressure-history-latest:' || series.id || ':'
        || extract(epoch FROM bucket.bucket_start)::bigint)::uuid,
    360,
    true,
    5,
    5,
    8.0,
    8.5,
    9.0,
    0.2,
    0.0,
    NULL,
    bucket.bucket_start + make_interval(secs => bucket.bucket_secs - 300),
    bucket.bucket_start + make_interval(secs => bucket.bucket_secs - 300),
    now()
FROM network_observation_series series
JOIN tunnel_plans plan ON plan.id = series.plan_id
CROSS JOIN buckets bucket
WHERE plan.name LIKE 'pressure-history-plan-%';
COMMIT;

-- The live system sampler writes unprefixed production names. Prefixing this
-- fixed 50-metric set prevents a concurrent sampler from changing proof-owned
-- cardinalities while preserving the real metric-name and query distribution.
BEGIN;
WITH metrics(metric, metric_ordinal) AS (
    VALUES
        ('pressure.db_pool.max_connections', 1),
        ('pressure.db_pool.open_connections', 2),
        ('pressure.db_pool.idle_connections', 3),
        ('pressure.db_pool.in_use_connections', 4),
        ('pressure.dispatch.active_jobs', 5),
        ('pressure.dispatch.queued_jobs', 6),
        ('pressure.dispatch.running_jobs', 7),
        ('pressure.dispatch.queue_depth', 8),
        ('pressure.dispatch.total_dispatch_attempts', 9),
        ('pressure.dispatch.retried_targets', 10),
        ('pressure.targets.queued', 11),
        ('pressure.targets.dispatching', 12),
        ('pressure.targets.running', 13),
        ('pressure.targets.active', 14),
        ('pressure.targets.deadline_expired_active', 15),
        ('pressure.targets.control_timeout_last_24h', 16),
        ('pressure.targets.agent_timeout_last_24h', 17),
        ('pressure.targets.agent_lost_last_24h', 18),
        ('pressure.targets.canceled_last_24h', 19),
        ('pressure.cancellations.requested', 20),
        ('pressure.cancellations.sent', 21),
        ('pressure.cancellations.acked', 22),
        ('pressure.cancellations.awaiting_ack', 23),
        ('pressure.gateway_events.queued_events', 24),
        ('pressure.gateway_events.delivered_events', 25),
        ('pressure.gateway_events.retry_attempts', 26),
        ('pressure.gateway_events.active_queues', 27),
        ('pressure.gateway_events.current_queue_depth', 28),
        ('pressure.gateway_events.oldest_event_age_secs', 29),
        ('pressure.gateway_events.dropped_events', 30),
        ('pressure.gateway_events.telemetry_dropped_events', 31),
        ('pressure.gateway_events.expired_events', 32),
        ('pressure.gateway_events.critical_failures', 33),
        ('pressure.gateway_events.dropped_by_kind.telemetry', 34),
        ('pressure.gateway_events.dropped_by_kind.command_output', 35),
        ('pressure.gateway_events.dropped_by_kind.lifecycle', 36),
        ('pressure.gateway_events.dropped_by_kind.terminal_output', 37),
        ('pressure.gateway_events.dropped_by_kind.other', 38),
        ('pressure.gateway_events.dropped_by_reason.global_queue_full', 39),
        ('pressure.gateway_events.dropped_by_reason.target_queue_full', 40),
        ('pressure.gateway_events.dropped_by_reason.expired', 41),
        ('pressure.gateway_events.dropped_by_reason.coalesced', 42),
        ('pressure.gateway_events.critical_failures_by_reason.global_queue_full', 43),
        ('pressure.gateway_events.critical_failures_by_reason.target_queue_full', 44),
        ('pressure.gateway_events.critical_failures_by_reason.expired', 45),
        ('pressure.gateway_events.retained_output_truncated_events', 46),
        ('pressure.gateway_events.rejected_agent_connections', 47),
        ('pressure.gateway_events.telemetry_admission_limit', 48),
        ('pressure.gateway_events.telemetry_admission_active', 49),
        ('pressure.gateway_events.telemetry_admission_waiting', 50)
), buckets AS (
    SELECT
        range.bucket_secs,
        generated.bucket_start,
        range.bucket_secs / 60 AS represented_minutes
    FROM pressure_retained_ranges range
    CROSS JOIN LATERAL generate_series(
        range.range_start,
        range.range_end - make_interval(secs => range.bucket_secs),
        make_interval(secs => range.bucket_secs)
    ) generated(bucket_start)
)
INSERT INTO system_metric_rollups (
    metric, bucket_start, bucket_secs, sample_count, value_sum,
    avg_value, max_value, latest_value, latest_observed_at, updated_at
)
SELECT
    metric.metric,
    bucket.bucket_start,
    bucket.bucket_secs,
    bucket.represented_minutes,
    metric.metric_ordinal::double precision * bucket.represented_minutes,
    metric.metric_ordinal::double precision,
    metric.metric_ordinal::double precision + 1.0,
    metric.metric_ordinal::double precision,
    bucket.bucket_start + make_interval(secs => bucket.bucket_secs - 60),
    now()
FROM metrics metric
CROSS JOIN buckets bucket;
COMMIT;

ANALYZE
    telemetry_samples,
    telemetry_counter_facts,
    telemetry_rollups,
    telemetry_resource_latest,
    telemetry_network_rates,
    telemetry_ping_series,
    telemetry_ping_facts,
    telemetry_ping_rollups,
    telemetry_ping_current,
    network_observation_series,
    network_observations,
    network_observation_latest,
    network_observation_rollups,
    system_metric_rollups;

DO $$
DECLARE
    rows_per_stream BIGINT;
    represented_minutes BIGINT;
    observation_rollup_rows_per_stream BIGINT;
    observation_represented_checks BIGINT;
BEGIN
    SELECT sum(expected_rows), sum(expected_rows * bucket_secs / 60)
    INTO rows_per_stream, represented_minutes
    FROM pressure_retained_ranges;
    SELECT
        sum(expected_rows),
        sum(expected_rows * bucket_secs / 300)
    INTO observation_rollup_rows_per_stream, observation_represented_checks
    FROM pressure_retained_ranges
    WHERE bucket_secs >= 300;

    IF (SELECT count(*) FROM telemetry_samples WHERE client_id LIKE 'pressure-%')
            <> 120 * 10080
       OR (SELECT count(*) FROM telemetry_counter_facts
           WHERE client_id LIKE 'pressure-%') <> 120 * 10080
       OR (SELECT count(*) FROM telemetry_rollups
           WHERE client_id LIKE 'pressure-%') <> 120 * rows_per_stream
       OR (SELECT sum(sample_count) FROM telemetry_rollups
           WHERE client_id LIKE 'pressure-%') <> 120 * represented_minutes
       OR (SELECT count(*) FROM telemetry_resource_latest
           WHERE client_id LIKE 'pressure-%') <> 120
       OR (SELECT count(*) FROM telemetry_network_rates
           WHERE client_id LIKE 'pressure-%') <> 120 * rows_per_stream
       OR (SELECT sum(sample_count) FROM telemetry_network_rates
           WHERE client_id LIKE 'pressure-%') <> 120 * represented_minutes THEN
        RAISE EXCEPTION 'resource/network retained-history cardinality mismatch';
    END IF;

    IF (SELECT count(*) FROM telemetry_ping_series series
        JOIN clients client ON client.id = series.client_id
        WHERE client.id LIKE 'pressure-%') <> 120
       OR (SELECT count(*) FROM telemetry_ping_facts fact
           JOIN telemetry_ping_series series ON series.id = fact.series_id
           WHERE series.client_id LIKE 'pressure-%') <> 120 * 10080
       OR (SELECT count(*) FROM telemetry_ping_rollups rollup
           JOIN telemetry_ping_series series ON series.id = rollup.series_id
           WHERE series.client_id LIKE 'pressure-%') <> 120 * rows_per_stream
       OR (SELECT sum(rollup.sample_count) FROM telemetry_ping_rollups rollup
           JOIN telemetry_ping_series series ON series.id = rollup.series_id
           WHERE series.client_id LIKE 'pressure-%') <> 120 * represented_minutes
       OR (SELECT count(*) FROM telemetry_ping_current current
           JOIN telemetry_ping_series series ON series.id = current.series_id
           WHERE series.client_id LIKE 'pressure-%') <> 120 THEN
        RAISE EXCEPTION 'Ping retained-history cardinality mismatch';
    END IF;

    IF (SELECT count(*) FROM network_observation_series series
        JOIN tunnel_plans plan ON plan.id = series.plan_id
        WHERE plan.name LIKE 'pressure-history-plan-%') <> 120
       OR (SELECT count(*) FROM network_observation_latest latest
           JOIN network_observation_series series ON series.id = latest.series_id
           JOIN tunnel_plans plan ON plan.id = series.plan_id
           WHERE plan.name LIKE 'pressure-history-plan-%') <> 120
       OR (SELECT count(*) FROM network_observations observation
           JOIN tunnel_plans plan ON plan.id = observation.plan_id
           WHERE plan.name LIKE 'pressure-history-plan-%') <> 120 * 552
       OR (SELECT count(*) FROM network_observation_rollups rollup
           JOIN network_observation_series series ON series.id = rollup.series_id
           JOIN tunnel_plans plan ON plan.id = series.plan_id
           WHERE plan.name LIKE 'pressure-history-plan-%')
            <> 120 * observation_rollup_rows_per_stream
       OR (SELECT sum(rollup.sample_count)
           FROM network_observation_rollups rollup
           JOIN network_observation_series series ON series.id = rollup.series_id
           JOIN tunnel_plans plan ON plan.id = series.plan_id
           WHERE plan.name LIKE 'pressure-history-plan-%')
            <> 120 * observation_represented_checks
       OR (SELECT count(DISTINCT metric) FROM system_metric_rollups
           WHERE metric LIKE 'pressure.%') <> 50
       OR (SELECT count(*) FROM system_metric_rollups
           WHERE metric LIKE 'pressure.%') <> 50 * rows_per_stream
       OR (SELECT sum(sample_count) FROM system_metric_rollups
           WHERE metric LIKE 'pressure.%') <> 50 * represented_minutes THEN
        RAISE EXCEPTION 'observation/system retained-history cardinality mismatch';
    END IF;
END;
$$;

SELECT jsonb_build_object(
    'schema', 'vpsman-five-year-retained-fixture/v1',
    'history_start', config.history_start,
    'history_end', config.history_end,
    'maintenance_safe_until', config.maintenance_safe_until,
    'tier_rows_per_stream', (
        SELECT jsonb_object_agg(range.bucket_secs::text, range.expected_rows
            ORDER BY range.bucket_secs)
        FROM pressure_retained_ranges range
    ),
    'rollup_rows_per_stream', (
        SELECT sum(range.expected_rows) FROM pressure_retained_ranges range
    ),
    'represented_minutes_per_stream', (
        SELECT sum(range.expected_rows * range.bucket_secs / 60)
        FROM pressure_retained_ranges range
    ),
    'raw_resource_rows_per_client', 10080,
    'raw_ping_rows_per_client', 10080,
    'network_observation_exact_rows_per_stream', 552,
    'network_observation_rollup_rows_per_stream', (
        SELECT sum(range.expected_rows)
        FROM pressure_retained_ranges range
        WHERE range.bucket_secs >= 300
    ),
    'network_observation_represented_checks_per_stream', 552 + (
        SELECT sum(range.expected_rows * range.bucket_secs / 300)
        FROM pressure_retained_ranges range
        WHERE range.bucket_secs >= 300
    ),
    'system_metric_series', 50
)
FROM pressure_retained_config config;

SELECT pg_advisory_unlock(1448104781, 5);
