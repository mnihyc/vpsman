-- Read-only, representation-independent semantic fingerprints for the opt-in
-- five-year pressure fixture. Physical tier and row-count fields are omitted;
-- every history family is canonicalized by logical stream and UTC day before
-- hashing in deterministic key order.
WITH raw_resource_streams AS (
    SELECT
        sample.client_id,
        count(*) AS rows,
        md5(string_agg(
            md5(jsonb_build_array(
                sample.observed_at, sample.cpu_utilization_ratio,
                sample.cpu_cores, sample.cpu_load_1, sample.cpu_load_5,
                sample.cpu_load_15, sample.memory_total_bytes,
                sample.memory_available_bytes, sample.swap_total_bytes,
                sample.swap_available_bytes, sample.disk_total_bytes,
                sample.disk_available_bytes, sample.network_rx_bytes,
                sample.network_tx_bytes, sample.tcp_sockets,
                sample.udp_sockets, sample.payload
            )::text),
            '' ORDER BY sample.observed_at, sample.id
        )) AS stream_hash
    FROM telemetry_samples sample
    WHERE sample.client_id LIKE 'pressure-%'
    GROUP BY sample.client_id
), raw_resource_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(client_id, rows, stream_hash)::text),
            '' ORDER BY client_id
        ), '')) AS hash
    FROM raw_resource_streams
), raw_counter_streams AS (
    SELECT
        fact.client_id,
        fact.source_kind,
        fact.interface,
        count(*) AS rows,
        md5(string_agg(
            md5(jsonb_build_array(
                fact.observed_at, fact.ordinal, fact.rx_bytes, fact.tx_bytes
            )::text),
            '' ORDER BY fact.observed_at, fact.sample_id, fact.ordinal
        )) AS stream_hash
    FROM telemetry_counter_facts fact
    WHERE fact.client_id LIKE 'pressure-%'
    GROUP BY fact.client_id, fact.source_kind, fact.interface
), raw_counter_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(
                client_id, source_kind, interface, rows, stream_hash
            )::text),
            '' ORDER BY client_id, source_kind, interface
        ), '')) AS hash
    FROM raw_counter_streams
), resource_days AS (
    SELECT
        rollup.client_id,
        date_bin(
            interval '1 day', rollup.bucket_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) AS semantic_day,
        md5(jsonb_build_array(
            sum(rollup.sample_count),
            sum(rollup.cpu_usage_sample_count),
            round(sum(rollup.cpu_usage_sum)::numeric, 9),
            max(rollup.cpu_usage_max), max(rollup.cpu_cores_max),
            round(sum(rollup.cpu_load_1_sum)::numeric, 9),
            max(rollup.cpu_load_1_max),
            round(sum(rollup.cpu_load_5_sum)::numeric, 9),
            max(rollup.cpu_load_5_max),
            round(sum(rollup.cpu_load_15_sum)::numeric, 9),
            max(rollup.cpu_load_15_max),
            max(rollup.memory_total_bytes_max),
            sum(rollup.memory_available_bytes_sum),
            min(rollup.memory_available_bytes_min),
            round(sum(rollup.memory_used_ratio_sum)::numeric, 9),
            max(rollup.memory_used_ratio_max),
            sum(rollup.swap_sample_count), max(rollup.swap_total_bytes_max),
            sum(rollup.swap_available_bytes_sum),
            min(rollup.swap_available_bytes_min),
            round(sum(rollup.swap_used_ratio_sum)::numeric, 9),
            max(rollup.swap_used_ratio_max),
            sum(rollup.disk_sample_count), max(rollup.disk_total_bytes_max),
            sum(rollup.disk_available_bytes_sum),
            min(rollup.disk_available_bytes_min),
            round(sum(rollup.disk_used_ratio_sum)::numeric, 9),
            max(rollup.disk_used_ratio_max),
            max(rollup.network_rx_bytes_max), max(rollup.network_tx_bytes_max),
            sum(rollup.connections_sample_count),
            (array_agg(rollup.tcp_sockets_latest ORDER BY
                rollup.connections_observed_at DESC NULLS LAST))[1],
            (array_agg(rollup.udp_sockets_latest ORDER BY
                rollup.connections_observed_at DESC NULLS LAST))[1],
            max(rollup.connections_observed_at), max(rollup.latest_observed_at)
        )::text) AS day_hash
    FROM telemetry_rollups rollup
    WHERE rollup.client_id LIKE 'pressure-%'
    GROUP BY rollup.client_id, semantic_day
), resource_streams AS (
    SELECT
        client_id,
        md5(string_agg(
            md5(jsonb_build_array(semantic_day, day_hash)::text),
            '' ORDER BY semantic_day
        )) AS stream_hash
    FROM resource_days
    GROUP BY client_id
), resource_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(client_id, stream_hash)::text),
            '' ORDER BY client_id
        ), '')) AS hash
    FROM resource_streams
), resource_latest_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(
                latest.client_id, latest.sample_count,
                latest.cpu_usage_sample_count,
                round(latest.cpu_usage_sum::numeric, 9), latest.cpu_usage_max,
                latest.cpu_cores_max,
                round(latest.cpu_load_1_sum::numeric, 9), latest.cpu_load_1_max,
                round(latest.cpu_load_5_sum::numeric, 9), latest.cpu_load_5_max,
                round(latest.cpu_load_15_sum::numeric, 9), latest.cpu_load_15_max,
                latest.memory_total_bytes_max,
                latest.memory_available_bytes_sum,
                latest.memory_available_bytes_min,
                round(latest.memory_used_ratio_sum::numeric, 9),
                latest.memory_used_ratio_max, latest.swap_sample_count,
                latest.swap_total_bytes_max, latest.swap_available_bytes_sum,
                latest.swap_available_bytes_min,
                round(latest.swap_used_ratio_sum::numeric, 9),
                latest.swap_used_ratio_max, latest.disk_sample_count,
                latest.disk_total_bytes_max, latest.disk_available_bytes_sum,
                latest.disk_available_bytes_min,
                round(latest.disk_used_ratio_sum::numeric, 9),
                latest.disk_used_ratio_max, latest.network_rx_bytes_max,
                latest.network_tx_bytes_max, latest.connections_sample_count,
                latest.tcp_sockets_latest, latest.udp_sockets_latest,
                latest.connections_observed_at, latest.latest_observed_at
            )::text),
            '' ORDER BY latest.client_id
        ), '')) AS hash
    FROM telemetry_resource_latest latest
    WHERE latest.client_id LIKE 'pressure-%'
), network_rate_days AS (
    SELECT
        rate.client_id,
        rate.interface,
        date_bin(
            interval '1 day', rate.bucket_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) AS semantic_day,
        md5(jsonb_build_array(
            sum(rate.sample_count), sum(rate.rx_bytes_sum), sum(rate.tx_bytes_sum),
            (array_agg(rate.rx_bytes_last ORDER BY
                rate.latest_observed_at DESC))[1],
            (array_agg(rate.tx_bytes_last ORDER BY
                rate.latest_observed_at DESC))[1],
            (array_agg(rate.rx_counter_epoch ORDER BY
                rate.latest_observed_at DESC))[1],
            (array_agg(rate.tx_counter_epoch ORDER BY
                rate.latest_observed_at DESC))[1],
            max(rate.latest_observed_at)
        )::text) AS day_hash
    FROM telemetry_network_rates rate
    WHERE rate.client_id LIKE 'pressure-%'
    GROUP BY rate.client_id, rate.interface, semantic_day
), network_rate_streams AS (
    SELECT
        client_id,
        interface,
        md5(string_agg(
            md5(jsonb_build_array(semantic_day, day_hash)::text),
            '' ORDER BY semantic_day
        )) AS stream_hash
    FROM network_rate_days
    GROUP BY client_id, interface
), network_rate_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(client_id, interface, stream_hash)::text),
            '' ORDER BY client_id, interface
        ), '')) AS hash
    FROM network_rate_streams
), raw_ping_streams AS (
    SELECT
        series.client_id,
        series.target_id,
        series.generation,
        count(*) AS rows,
        md5(string_agg(
            md5(jsonb_build_array(
                fact.observed_at, fact.source_checked_unix, fact.checked_unix,
                fact.status, fact.latency_avg_ms, fact.loss_ratio, fact.reason
            )::text),
            '' ORDER BY fact.source_checked_unix
        )) AS stream_hash
    FROM telemetry_ping_facts fact
    JOIN telemetry_ping_series series ON series.id = fact.series_id
    WHERE series.client_id LIKE 'pressure-%'
    GROUP BY series.client_id, series.target_id, series.generation
), raw_ping_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(
                client_id, target_id, generation, rows, stream_hash
            )::text),
            '' ORDER BY client_id, target_id, generation
        ), '')) AS hash
    FROM raw_ping_streams
), ping_days AS (
    SELECT
        series.client_id,
        series.target_id,
        series.generation,
        date_bin(
            interval '1 day', rollup.bucket_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) AS semantic_day,
        md5(jsonb_build_array(
            sum(rollup.sample_count), sum(rollup.success_count),
            round(sum(rollup.latency_sum_ms)::numeric, 9),
            min(rollup.latency_min_ms), max(rollup.latency_max_ms),
            round(sum(rollup.loss_ratio_sum)::numeric, 9),
            max(rollup.loss_ratio_max),
            (array_agg(rollup.latest_status ORDER BY
                rollup.latest_checked_at DESC))[1],
            (array_agg(rollup.latest_reason ORDER BY
                rollup.latest_checked_at DESC))[1],
            max(rollup.latest_checked_at)
        )::text) AS day_hash
    FROM telemetry_ping_rollups rollup
    JOIN telemetry_ping_series series ON series.id = rollup.series_id
    WHERE series.client_id LIKE 'pressure-%'
    GROUP BY
        series.client_id, series.target_id, series.generation, semantic_day
), ping_streams AS (
    SELECT
        client_id,
        target_id,
        generation,
        md5(string_agg(
            md5(jsonb_build_array(semantic_day, day_hash)::text),
            '' ORDER BY semantic_day
        )) AS stream_hash
    FROM ping_days
    GROUP BY client_id, target_id, generation
), ping_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(
                client_id, target_id, generation, stream_hash
            )::text),
            '' ORDER BY client_id, target_id, generation
        ), '')) AS hash
    FROM ping_streams
), ping_current_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(
                series.client_id, series.target_id, series.generation,
                current.latest_status, current.latency_avg_ms,
                current.rolling_loss_ratio, current.latest_reason,
                current.latest_checked_at
            )::text),
            '' ORDER BY series.client_id, series.target_id, series.generation
        ), '')) AS hash
    FROM telemetry_ping_current current
    JOIN telemetry_ping_series series ON series.id = current.series_id
    WHERE series.client_id LIKE 'pressure-%'
), observation_components AS (
    SELECT
        observation.automatic_series_id AS series_id,
        date_bin(
            interval '1 day', observation.observed_at,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) AS semantic_day,
        CASE WHEN observation.healthy IS TRUE THEN 1
             WHEN observation.healthy IS FALSE THEN 0 ELSE -1 END::smallint
            AS health_state,
        COALESCE(observation.reason, '') AS reason_key,
        1::bigint AS sample_count,
        COALESCE(observation.transmitted, 0)::numeric AS transmitted_total,
        (observation.transmitted IS NOT NULL)::integer::bigint
            AS transmitted_sample_count,
        COALESCE(observation.received, 0)::numeric AS received_total,
        (observation.received IS NOT NULL)::integer::bigint
            AS received_sample_count,
        COALESCE(observation.latency_avg_ms, 0.0) AS latency_sum_ms,
        (observation.latency_avg_ms IS NOT NULL)::integer::bigint
            AS latency_sample_count,
        observation.latency_min_ms,
        observation.latency_max_ms,
        COALESCE(observation.latency_mdev_ms, 0.0) AS latency_mdev_sum_ms,
        (observation.latency_mdev_ms IS NOT NULL)::integer::bigint
            AS latency_mdev_sample_count,
        COALESCE(observation.packet_loss_ratio, 0.0) AS packet_loss_sum_ratio,
        (observation.packet_loss_ratio IS NOT NULL)::integer::bigint
            AS packet_loss_sample_count,
        observation.packet_loss_ratio AS packet_loss_min_ratio,
        observation.packet_loss_ratio AS packet_loss_max_ratio,
        COALESCE(observation.throughput_mbps, 0.0) AS throughput_sum_mbps,
        (observation.throughput_mbps IS NOT NULL)::integer::bigint
            AS throughput_sample_count,
        observation.throughput_mbps AS throughput_max_mbps,
        COALESCE(observation.bytes, 0)::numeric AS bytes_total,
        observation.id AS latest_observation_id,
        observation.stale_after_secs AS latest_stale_after_secs,
        observation.healthy AS latest_healthy,
        observation.transmitted AS latest_transmitted,
        observation.received AS latest_received,
        observation.latency_min_ms AS latest_latency_min_ms,
        observation.latency_avg_ms AS latest_latency_avg_ms,
        observation.latency_max_ms AS latest_latency_max_ms,
        observation.latency_mdev_ms AS latest_latency_mdev_ms,
        observation.packet_loss_ratio AS latest_packet_loss_ratio,
        observation.reason AS latest_reason,
        observation.observed_at AS latest_observed_at,
        observation.received_at AS latest_received_at
    FROM network_observations observation
    JOIN tunnel_plans plan ON plan.id = observation.plan_id
    WHERE plan.name LIKE 'pressure-history-plan-%'
      AND observation.source = 'automatic'
    UNION ALL
    SELECT
        rollup.series_id,
        date_bin(
            interval '1 day', rollup.bucket_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ),
        rollup.health_state,
        rollup.reason_key,
        rollup.sample_count,
        rollup.transmitted_total,
        rollup.transmitted_sample_count,
        rollup.received_total,
        rollup.received_sample_count,
        rollup.latency_sum_ms,
        rollup.latency_sample_count,
        rollup.latency_min_ms,
        rollup.latency_max_ms,
        rollup.latency_mdev_sum_ms,
        rollup.latency_mdev_sample_count,
        rollup.packet_loss_sum_ratio,
        rollup.packet_loss_sample_count,
        rollup.packet_loss_min_ratio,
        rollup.packet_loss_max_ratio,
        0.0::double precision,
        0::bigint,
        NULL::double precision,
        0::numeric,
        rollup.latest_observation_id,
        rollup.latest_stale_after_secs,
        rollup.latest_healthy,
        rollup.latest_transmitted,
        rollup.latest_received,
        rollup.latest_latency_min_ms,
        rollup.latest_latency_avg_ms,
        rollup.latest_latency_max_ms,
        rollup.latest_latency_mdev_ms,
        rollup.latest_packet_loss_ratio,
        rollup.latest_reason,
        rollup.latest_observed_at,
        rollup.latest_received_at
    FROM network_observation_rollups rollup
    JOIN network_observation_series series ON series.id = rollup.series_id
    JOIN tunnel_plans plan ON plan.id = series.plan_id
    WHERE plan.name LIKE 'pressure-history-plan-%'
), observation_days AS (
    SELECT
        component.series_id,
        component.semantic_day,
        component.health_state,
        component.reason_key,
        md5(jsonb_build_array(
            sum(component.sample_count), sum(component.transmitted_total),
            sum(component.transmitted_sample_count),
            sum(component.received_total), sum(component.received_sample_count),
            round(sum(component.latency_sum_ms)::numeric, 9),
            sum(component.latency_sample_count), min(component.latency_min_ms),
            max(component.latency_max_ms),
            round(sum(component.latency_mdev_sum_ms)::numeric, 9),
            sum(component.latency_mdev_sample_count),
            round(sum(component.packet_loss_sum_ratio)::numeric, 9),
            sum(component.packet_loss_sample_count),
            min(component.packet_loss_min_ratio),
            max(component.packet_loss_max_ratio),
            round(sum(component.throughput_sum_mbps)::numeric, 9),
            sum(component.throughput_sample_count),
            max(component.throughput_max_mbps), sum(component.bytes_total),
            (array_agg(component.latest_observation_id ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1],
            (array_agg(component.latest_stale_after_secs ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1],
            (array_agg(component.latest_healthy ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1],
            (array_agg(component.latest_transmitted ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1],
            (array_agg(component.latest_received ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1],
            (array_agg(component.latest_latency_min_ms ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1],
            (array_agg(component.latest_latency_avg_ms ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1],
            (array_agg(component.latest_latency_max_ms ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1],
            (array_agg(component.latest_latency_mdev_ms ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1],
            (array_agg(component.latest_packet_loss_ratio ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1],
            (array_agg(component.latest_reason ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1],
            max(component.latest_observed_at),
            (array_agg(component.latest_received_at ORDER BY
                component.latest_observed_at DESC,
                component.latest_observation_id DESC))[1]
        )::text) AS component_hash
    FROM observation_components component
    GROUP BY
        component.series_id, component.semantic_day,
        component.health_state, component.reason_key
), observation_streams AS (
    SELECT
        series.id,
        series.client_id,
        series.peer_client_id,
        series.plan_id,
        md5(string_agg(
            md5(jsonb_build_array(
                day.semantic_day, day.health_state,
                day.reason_key, day.component_hash
            )::text),
            '' ORDER BY day.semantic_day, day.health_state, day.reason_key
        )) AS stream_hash
    FROM observation_days day
    JOIN network_observation_series series ON series.id = day.series_id
    GROUP BY
        series.id, series.client_id, series.peer_client_id, series.plan_id
), observation_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(
                client_id, peer_client_id, plan_id, stream_hash
            )::text),
            '' ORDER BY client_id, peer_client_id, plan_id
        ), '')) AS hash
    FROM observation_streams
), observation_latest_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(
                series.client_id, series.peer_client_id, series.plan_id,
                latest.observation_id, latest.stale_after_secs,
                latest.healthy, latest.transmitted, latest.received,
                latest.latency_min_ms, latest.latency_avg_ms,
                latest.latency_max_ms, latest.latency_mdev_ms,
                latest.packet_loss_ratio, latest.reason, latest.metadata,
                latest.observed_at, latest.received_at
            )::text),
            '' ORDER BY series.client_id, series.peer_client_id, series.plan_id
        ), '')) AS hash
    FROM network_observation_latest latest
    JOIN network_observation_series series ON series.id = latest.series_id
    JOIN tunnel_plans plan ON plan.id = series.plan_id
    WHERE plan.name LIKE 'pressure-history-plan-%'
), system_days AS (
    SELECT
        rollup.metric,
        date_bin(
            interval '1 day', rollup.bucket_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) AS semantic_day,
        md5(jsonb_build_array(
            sum(rollup.sample_count),
            round(sum(rollup.value_sum)::numeric, 9),
            max(rollup.max_value),
            (array_agg(rollup.latest_value ORDER BY
                rollup.latest_observed_at DESC))[1],
            max(rollup.latest_observed_at)
        )::text) AS day_hash
    FROM system_metric_rollups rollup
    WHERE rollup.metric LIKE 'pressure.%'
    GROUP BY rollup.metric, semantic_day
), system_streams AS (
    SELECT
        metric,
        md5(string_agg(
            md5(jsonb_build_array(semantic_day, day_hash)::text),
            '' ORDER BY semantic_day
        )) AS stream_hash
    FROM system_days
    GROUP BY metric
), system_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(metric, stream_hash)::text),
            '' ORDER BY metric
        ), '')) AS hash
    FROM system_streams
), traffic_ledger_days AS (
    SELECT
        usage.client_id,
        usage.source_kind,
        usage.interface,
        date_bin(
            interval '1 day', usage.bucket_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) AS semantic_day,
        md5(string_agg(
            md5(jsonb_build_array(
                usage.bucket_start, usage.rx_bytes, usage.tx_bytes,
                usage.rx_reset_count, usage.tx_reset_count,
                usage.sample_count, usage.first_observed_at,
                usage.latest_observed_at
            )::text),
            '' ORDER BY usage.bucket_start
        )) AS day_hash
    FROM traffic_counter_hourly_usage usage
    WHERE usage.client_id LIKE 'pressure-%'
    GROUP BY
        usage.client_id, usage.source_kind, usage.interface, semantic_day
), traffic_ledger_streams AS (
    SELECT
        client_id,
        source_kind,
        interface,
        md5(string_agg(
            md5(jsonb_build_array(semantic_day, day_hash)::text),
            '' ORDER BY semantic_day
        )) AS stream_hash
    FROM traffic_ledger_days
    GROUP BY client_id, source_kind, interface
), traffic_ledger_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(
                client_id, source_kind, interface, stream_hash
            )::text),
            '' ORDER BY client_id, source_kind, interface
        ), '')) AS hash
    FROM traffic_ledger_streams
), traffic_latest_keys AS (
    -- The pressure fixture requires a clean, exactly 120-stream hourly
    -- registry before this query runs.  Use that maintained key relation and
    -- the raw lookup index for one latest-row probe per stream; scanning and
    -- sorting the entire five-year raw tail here made this one hash InitPlan
    -- dominate the retained post-validation query.
    SELECT streams.client_id, streams.source_kind, streams.interface
    FROM traffic_counter_hourly_usage_streams streams
    WHERE streams.client_id LIKE 'pressure-%'
), traffic_latest_key_guard AS (
    SELECT count(*) AS streams
    FROM traffic_latest_keys
), traffic_latest_rows AS (
    SELECT
        key.client_id,
        key.source_kind,
        key.interface,
        latest.observed_at,
        latest.rx_bytes,
        latest.tx_bytes,
        latest.rx_counter_epoch,
        latest.tx_counter_epoch,
        latest.sample_source
    FROM traffic_latest_keys key
    CROSS JOIN traffic_latest_key_guard guard
    CROSS JOIN LATERAL (
        SELECT
            sample.observed_at,
            sample.rx_bytes,
            sample.tx_bytes,
            sample.rx_counter_epoch,
            sample.tx_counter_epoch,
            sample.sample_source
        FROM traffic_counter_samples sample
        WHERE sample.client_id = key.client_id
          AND sample.source_kind = key.source_kind
          AND sample.interface = key.interface
        ORDER BY sample.observed_at DESC
        LIMIT 1
    ) latest
    WHERE guard.streams = 120
), traffic_latest_hash AS (
    SELECT
        count(*) AS streams,
        md5(COALESCE(string_agg(
            md5(jsonb_build_array(
                client_id, source_kind, interface, observed_at,
                rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch,
                sample_source
            )::text),
            '' ORDER BY client_id, source_kind, interface
        ), '')) AS hash
    FROM traffic_latest_rows
)
SELECT jsonb_build_object(
    'schema', 'vpsman-five-year-semantic-hashes/v1',
    'raw_resource', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM raw_resource_hash),
    'raw_counter_facts', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM raw_counter_hash),
    'resource_rollups', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM resource_hash),
    'resource_latest', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM resource_latest_hash),
    'network_rate_rollups', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM network_rate_hash),
    'raw_ping_facts', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM raw_ping_hash),
    'ping_rollups', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM ping_hash),
    'ping_current', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM ping_current_hash),
    'network_observations', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM observation_hash),
    'network_observation_latest', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM observation_latest_hash),
    'system_metric_rollups', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM system_hash),
    'traffic_hourly_ledger', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM traffic_ledger_hash),
    'traffic_latest_counter_epochs', (SELECT jsonb_build_object(
        'streams', streams, 'hash', hash) FROM traffic_latest_hash)
) AS semantic_hashes;
