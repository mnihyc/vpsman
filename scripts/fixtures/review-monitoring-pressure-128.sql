\set ON_ERROR_STOP on

\if :{?pressure_skip_traffic}
\else
\set pressure_skip_traffic false
\endif

-- Supplemental load fixture for the isolated real-data monitoring review.
-- The base fixture owns the eight review-* semantic cases. This fixture owns
-- only pressure-* rows and may be reapplied without changing those cases.

BEGIN ISOLATION LEVEL REPEATABLE READ;

CREATE TEMP TABLE pressure_fixture_options (
    skip_traffic BOOLEAN NOT NULL
) ON COMMIT DROP;

INSERT INTO pressure_fixture_options (skip_traffic)
VALUES (:'pressure_skip_traffic'::boolean);

-- Keep the ownership preflight valid until commit. Ordinary client writes take
-- ROW EXCLUSIVE, which conflicts with this lock, while this transaction can
-- still delete and recreate its own deterministic identities.
LOCK TABLE clients IN SHARE ROW EXCLUSIVE MODE;

CREATE TEMP TABLE pressure_fixture_client_ids (
    client_id TEXT PRIMARY KEY
) ON COMMIT DROP;

INSERT INTO pressure_fixture_client_ids (client_id)
SELECT 'pressure-' || lpad(client_number::text, 3, '0')
FROM generate_series(1, 120) generated(client_number);

DO $$
DECLARE
    base_client_ids TEXT[];
BEGIN
    IF EXISTS (
        SELECT 1
        FROM clients client
        WHERE client.id LIKE 'pressure-%'
          AND NOT EXISTS (
              SELECT 1
              FROM pressure_fixture_client_ids owned
              WHERE owned.client_id = client.id
          )
    ) THEN
        RAISE EXCEPTION
            'pressure fixture refuses to delete an unowned pressure-* client';
    END IF;

    SELECT array_agg(client.id ORDER BY client.id)
    INTO base_client_ids
    FROM clients client
    WHERE NOT EXISTS (
        SELECT 1
        FROM pressure_fixture_client_ids owned
        WHERE owned.client_id = client.id
    );

    IF base_client_ids IS DISTINCT FROM ARRAY[
        'review-empty-rates',
        'review-no-primary',
        'review-no-reset',
        'review-rx-yearly',
        'review-total-monthly',
        'review-traffic-exceeded',
        'review-tx-unlimited',
        'review-unconfigured'
    ]::TEXT[] THEN
        RAISE EXCEPTION
            'pressure fixture requires the exact isolated eight-client review seed; found %',
            base_client_ids;
    END IF;

    IF (SELECT count(*) FROM telemetry_samples
        WHERE client_id = 'review-total-monthly') <> 16
       OR (SELECT count(*) FROM telemetry_rollups
           WHERE client_id = 'review-total-monthly') <> 16
       OR (SELECT count(*) FROM telemetry_resource_latest
           WHERE client_id = 'review-total-monthly') <> 1
       OR (SELECT count(*) FROM telemetry_network_rates
           WHERE client_id = 'review-total-monthly') <> 16 THEN
        RAISE EXCEPTION
            'pressure fixture requires refreshed review-total-monthly telemetry';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM telemetry_samples
        WHERE client_id = 'review-total-monthly'
          AND (
              disk_total_bytes IS NULL
              OR disk_available_bytes IS NULL
              OR payload ->> 'disk_semantics'
                    IS DISTINCT FROM 'persistent_block_filesystems_v1'
              OR payload ->> 'disk_collection_available'
                    IS DISTINCT FROM 'true'
          )
    ) OR EXISTS (
        SELECT 1
        FROM telemetry_rollups
        WHERE client_id = 'review-total-monthly'
          AND disk_sample_count <> sample_count
    ) OR EXISTS (
        SELECT 1
        FROM telemetry_resource_latest
        WHERE client_id = 'review-total-monthly'
          AND disk_sample_count <> sample_count
    ) THEN
        RAISE EXCEPTION
            'pressure fixture requires authoritative 0014 physical-disk telemetry';
    END IF;
END;
$$;

-- Cascades remove only data owned by a previous application of this fixture.
-- The hourly DELETE trigger observes that these client identities are already
-- absent and therefore does not recreate their derived coverage rows.
DELETE FROM clients client
USING pressure_fixture_client_ids owned
WHERE client.id = owned.client_id;

WITH pressure_clients AS (
    SELECT
        substring(owned.client_id FROM '[0-9]+$')::integer AS client_number,
        owned.client_id,
        'Pressure agent '
            || substring(owned.client_id FROM '[0-9]+$') AS display_name
    FROM pressure_fixture_client_ids owned
)
INSERT INTO clients (
    id,
    display_name,
    public_key,
    status,
    agent_version,
    process_incarnation_id,
    os_release,
    arch,
    cpu_model,
    kernel_release,
    virtualization,
    system_reported_at,
    capabilities,
    registration_ip,
    last_ip,
    last_seen_at,
    created_at
)
SELECT
    pressure.client_id,
    pressure.display_name,
    decode(
        md5('pressure-key-a:' || pressure.client_number::text)
        || md5('pressure-key-b:' || pressure.client_number::text),
        'hex'
    ),
    'online',
    'pressure-agent-fixture',
    md5('pressure-process:' || pressure.client_number::text)::uuid,
    'Debian GNU/Linux 13 (trixie)',
    'x86_64',
    'AMD EPYC pressure fixture',
    '6.12.38-amd64',
    'KVM',
    now(),
    jsonb_build_object(
        'privilege_mode', 'root',
        'can_attempt_privileged_ops', true,
        'can_apply_process_limits', true,
        'can_manage_runtime_tunnels', true,
        'max_job_timeout_secs', 3600
    ),
    ('198.51.100.' || pressure.client_number::text)::inet,
    ('198.51.100.' || pressure.client_number::text)::inet,
    now(),
    now() - interval '90 days'
FROM pressure_clients pressure;

WITH pressure_clients AS (
    SELECT
        client.id AS client_id,
        substring(client.id FROM '[0-9]+$')::integer AS client_number
    FROM clients client
    JOIN pressure_fixture_client_ids owned ON owned.client_id = client.id
)
INSERT INTO vps_rule_values (
    client_id,
    key,
    value_raw,
    value_json,
    source_kind,
    updated_at
)
SELECT
    pressure.client_id,
    rule.key,
    rule.value_raw,
    rule.value_json,
    'pressure_fixture',
    now()
FROM pressure_clients pressure
CROSS JOIN LATERAL (
    VALUES
        (
            'product.name',
            'Pressure plan ' || lpad(pressure.client_number::text, 3, '0'),
            jsonb_build_object(
                'name',
                'Pressure plan ' || lpad(pressure.client_number::text, 3, '0'),
                'display',
                'Pressure plan ' || lpad(pressure.client_number::text, 3, '0')
            )
        ),
        (
            'traffic.reset_day',
            '1',
            '{"day":1}'::jsonb
        ),
        (
            'traffic.selectors',
            'eth0',
            '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb
        ),
        (
            'traffic.quota.total',
            '1 TB',
            '{"bytes":1000000000000,"display":"1 TB"}'::jsonb
        ),
        (
            'network.port_speed',
            '1 Gbps',
            '{"bps":1000000000,"display":"1 Gbps"}'::jsonb
        ),
        (
            'billing.price',
            '5.00 $/m',
            '{"disabled":false,"price":"5.00","currency":"USD","currency_display":"$","period":"month","period_code":"m","display":"5.00 $/m"}'::jsonb
        )
) rule(key, value_raw, value_json);

-- Clone the current 0014 raw-resource shape. Identity-bearing fields are
-- replaced while all physical-disk, memory, CPU, and network facts remain
-- internally consistent with the base semantic fixture.
WITH pressure_clients AS (
    SELECT client.id AS client_id
    FROM clients client
    JOIN pressure_fixture_client_ids owned ON owned.client_id = client.id
), base_samples AS (
    SELECT sample.*
    FROM telemetry_samples sample
    WHERE sample.client_id = 'review-total-monthly'
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
    md5(pressure.client_id || ':' || base.id::text)::uuid,
    pressure.client_id,
    base.observed_at,
    base.cpu_utilization_ratio,
    base.cpu_cores,
    base.cpu_load_1,
    base.cpu_load_5,
    base.cpu_load_15,
    base.memory_total_bytes,
    base.memory_available_bytes,
    base.swap_total_bytes,
    base.swap_available_bytes,
    base.disk_total_bytes,
    base.disk_available_bytes,
    base.network_rx_bytes,
    base.network_tx_bytes,
    base.tcp_sockets,
    base.udp_sockets,
    jsonb_set(base.payload, '{hostname}', to_jsonb(pressure.client_id), true)
FROM pressure_clients pressure
CROSS JOIN base_samples base;

INSERT INTO telemetry_counter_facts (
    sample_id,
    client_id,
    observed_at,
    source_kind,
    ordinal,
    interface,
    rx_bytes,
    tx_bytes
)
SELECT
    sample.id,
    sample.client_id,
    sample.observed_at,
    'host',
    (network.ordinal - 1)::integer,
    network.value ->> 'interface',
    (network.value ->> 'rx_bytes')::bigint,
    (network.value ->> 'tx_bytes')::bigint
FROM telemetry_samples sample
JOIN pressure_fixture_client_ids owned ON owned.client_id = sample.client_id
CROSS JOIN LATERAL jsonb_array_elements(sample.payload -> 'networks')
    WITH ORDINALITY network(value, ordinal);

-- Retained resource history is intentionally copied column-for-column from
-- the validated base row set, including 0014 disk_sample_count.
WITH pressure_clients AS (
    SELECT client.id AS client_id
    FROM clients client
    JOIN pressure_fixture_client_ids owned ON owned.client_id = client.id
), base_rollups AS (
    SELECT rollup.*
    FROM telemetry_rollups rollup
    WHERE rollup.client_id = 'review-total-monthly'
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
    updated_at,
    disk_sample_count
)
SELECT
    pressure.client_id,
    base.bucket_start,
    base.bucket_secs,
    base.sample_count,
    base.cpu_usage_sample_count,
    base.cpu_usage_sum,
    base.cpu_usage_avg,
    base.cpu_usage_max,
    base.cpu_cores_max,
    base.cpu_load_1_avg,
    base.cpu_load_1_sum,
    base.cpu_load_1_max,
    base.cpu_load_5_avg,
    base.cpu_load_5_sum,
    base.cpu_load_5_max,
    base.cpu_load_15_avg,
    base.cpu_load_15_sum,
    base.cpu_load_15_max,
    base.memory_total_bytes_max,
    base.memory_available_bytes_avg,
    base.memory_available_bytes_sum,
    base.memory_available_bytes_min,
    base.memory_used_ratio_avg,
    base.memory_used_ratio_sum,
    base.memory_used_ratio_max,
    base.swap_sample_count,
    base.swap_total_bytes_max,
    base.swap_available_bytes_avg,
    base.swap_available_bytes_sum,
    base.swap_available_bytes_min,
    base.swap_used_ratio_avg,
    base.swap_used_ratio_sum,
    base.swap_used_ratio_max,
    base.disk_total_bytes_max,
    base.disk_available_bytes_avg,
    base.disk_available_bytes_sum,
    base.disk_available_bytes_min,
    base.disk_used_ratio_avg,
    base.disk_used_ratio_sum,
    base.disk_used_ratio_max,
    base.network_rx_bytes_max,
    base.network_tx_bytes_max,
    base.connections_sample_count,
    base.tcp_sockets_latest,
    base.udp_sockets_latest,
    base.connections_observed_at,
    base.latest_observed_at,
    now(),
    base.disk_sample_count
FROM pressure_clients pressure
CROSS JOIN base_rollups base;

INSERT INTO telemetry_resource_latest
SELECT DISTINCT ON (rollup.client_id) rollup.*
FROM telemetry_rollups rollup
JOIN pressure_fixture_client_ids owned ON owned.client_id = rollup.client_id
ORDER BY
    rollup.client_id,
    rollup.latest_observed_at DESC,
    rollup.bucket_start DESC;

WITH pressure_clients AS (
    SELECT client.id AS client_id
    FROM clients client
    JOIN pressure_fixture_client_ids owned ON owned.client_id = client.id
), base_rates AS (
    SELECT rate.*
    FROM telemetry_network_rates rate
    WHERE rate.client_id = 'review-total-monthly'
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
    pressure.client_id,
    base.interface,
    base.bucket_start,
    base.bucket_secs,
    base.sample_count,
    base.rx_bytes_sum,
    base.tx_bytes_sum,
    base.rx_bytes_avg,
    base.tx_bytes_avg,
    base.rx_bytes_last,
    base.tx_bytes_last,
    base.rx_counter_epoch,
    base.tx_counter_epoch,
    base.latest_observed_at,
    now()
FROM pressure_clients pressure
CROSS JOIN base_rates base;

-- One statement deliberately exceeds the hourly trigger's large-import
-- boundary. The trigger must rebuild each complete stream, advance its source
-- revision, and mark the materialized revision clean in this transaction.
\if :pressure_skip_traffic
\else
WITH pressure_clients AS (
    SELECT
        client.id AS client_id,
        substring(client.id FROM '[0-9]+$')::bigint AS client_number
    FROM clients client
    JOIN pressure_fixture_client_ids owned ON owned.client_id = client.id
), traffic_points AS (
    SELECT
        pressure.client_id,
        pressure.client_number,
        minute_number,
        date_trunc('minute', now()) - interval '48 hours'
            + minute_number * interval '1 minute' AS observed_at
    FROM pressure_clients pressure
    CROSS JOIN generate_series(0, 2880) generated(minute_number)
)
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
SELECT
    point.client_id,
    'host',
    'eth0',
    point.observed_at,
    point.client_number * 10000000000::bigint
        + point.minute_number * 1000000::bigint,
    point.client_number * 5000000000::bigint
        + point.minute_number * 500000::bigint,
    0,
    0,
    'pressure_fixture'
FROM traffic_points point;
\endif

ANALYZE
    clients,
    vps_rule_values,
    telemetry_samples,
    telemetry_counter_facts,
    telemetry_rollups,
    telemetry_resource_latest,
    telemetry_network_rates,
    traffic_counter_samples,
    traffic_counter_hourly_usage,
    traffic_counter_hourly_usage_streams;

DO $$
DECLARE
    pressure_client_count BIGINT;
    total_client_count BIGINT;
    pressure_rule_count BIGINT;
    pressure_sample_count BIGINT;
    pressure_fact_count BIGINT;
    pressure_rollup_count BIGINT;
    pressure_latest_count BIGINT;
    pressure_rate_count BIGINT;
    pressure_traffic_count BIGINT;
    pressure_stream_count BIGINT;
    pressure_hourly_count BIGINT;
    pressure_hourly_sample_count BIGINT;
    pressure_hourly_rx_bytes NUMERIC;
    pressure_hourly_tx_bytes NUMERIC;
    skip_traffic BOOLEAN;
BEGIN
    SELECT options.skip_traffic INTO skip_traffic
    FROM pressure_fixture_options options;
    SELECT count(*) INTO pressure_client_count
    FROM clients client
    JOIN pressure_fixture_client_ids owned ON owned.client_id = client.id;
    SELECT count(*) INTO total_client_count FROM clients;
    SELECT count(*) INTO pressure_rule_count
    FROM vps_rule_values rule
    JOIN pressure_fixture_client_ids owned ON owned.client_id = rule.client_id;
    SELECT count(*) INTO pressure_sample_count
    FROM telemetry_samples sample
    JOIN pressure_fixture_client_ids owned ON owned.client_id = sample.client_id;
    SELECT count(*) INTO pressure_fact_count
    FROM telemetry_counter_facts fact
    JOIN pressure_fixture_client_ids owned ON owned.client_id = fact.client_id;
    SELECT count(*) INTO pressure_rollup_count
    FROM telemetry_rollups rollup
    JOIN pressure_fixture_client_ids owned ON owned.client_id = rollup.client_id;
    SELECT count(*) INTO pressure_latest_count
    FROM telemetry_resource_latest latest
    JOIN pressure_fixture_client_ids owned ON owned.client_id = latest.client_id;
    SELECT count(*) INTO pressure_rate_count
    FROM telemetry_network_rates rate
    JOIN pressure_fixture_client_ids owned ON owned.client_id = rate.client_id;
    SELECT count(*) INTO pressure_traffic_count
    FROM traffic_counter_samples sample
    JOIN pressure_fixture_client_ids owned ON owned.client_id = sample.client_id;
    SELECT count(*) INTO pressure_stream_count
    FROM traffic_counter_hourly_usage_streams stream
    JOIN pressure_fixture_client_ids owned ON owned.client_id = stream.client_id;
    SELECT
        count(*),
        COALESCE(sum(sample_count), 0),
        COALESCE(sum(rx_bytes), 0),
        COALESCE(sum(tx_bytes), 0)
    INTO
        pressure_hourly_count,
        pressure_hourly_sample_count,
        pressure_hourly_rx_bytes,
        pressure_hourly_tx_bytes
    FROM traffic_counter_hourly_usage usage
    JOIN pressure_fixture_client_ids owned ON owned.client_id = usage.client_id;

    IF pressure_client_count <> 120 OR total_client_count <> 128 THEN
        RAISE EXCEPTION
            'pressure client cardinality mismatch: pressure %, total %',
            pressure_client_count,
            total_client_count;
    END IF;
    IF EXISTS (
        SELECT 1
        FROM clients client
        WHERE client.id LIKE 'pressure-%'
          AND NOT EXISTS (
              SELECT 1
              FROM pressure_fixture_client_ids owned
              WHERE owned.client_id = client.id
          )
    ) THEN
        RAISE EXCEPTION 'an unowned pressure-* client appeared during seeding';
    END IF;
    IF (SELECT count(*)
        FROM clients client
        JOIN pressure_fixture_client_ids owned ON owned.client_id = client.id
        WHERE client.status = 'online') <> 120 THEN
        RAISE EXCEPTION 'not every pressure client is online';
    END IF;
    IF pressure_rule_count <> 720 OR EXISTS (
        SELECT 1
        FROM vps_rule_values rule
        JOIN pressure_fixture_client_ids owned ON owned.client_id = rule.client_id
        GROUP BY rule.client_id
        HAVING count(*) <> 6
    ) THEN
        RAISE EXCEPTION
            'pressure rule cardinality mismatch: % rows',
            pressure_rule_count;
    END IF;
    IF pressure_sample_count <> 1920
       OR pressure_fact_count <> 1920
       OR pressure_rollup_count <> 1920
       OR pressure_latest_count <> 120
       OR pressure_rate_count <> 1920 THEN
        RAISE EXCEPTION
            'pressure telemetry cardinality mismatch: samples %, facts %, rollups %, latest %, rates %',
            pressure_sample_count,
            pressure_fact_count,
            pressure_rollup_count,
            pressure_latest_count,
            pressure_rate_count;
    END IF;
    IF EXISTS (
        SELECT 1
        FROM telemetry_samples sample
        JOIN pressure_fixture_client_ids owned ON owned.client_id = sample.client_id
        WHERE (
              sample.disk_total_bytes IS NULL
              OR sample.disk_available_bytes IS NULL
              OR sample.payload ->> 'disk_semantics'
                    IS DISTINCT FROM 'persistent_block_filesystems_v1'
              OR sample.payload ->> 'disk_collection_available'
                    IS DISTINCT FROM 'true'
              OR jsonb_array_length(sample.payload -> 'disks') <> 1
              OR sample.payload -> 'disks' -> 0 ->> 'mountpoint'
                    IS DISTINCT FROM '/'
          )
    ) OR EXISTS (
        SELECT 1
        FROM telemetry_rollups rollup
        JOIN pressure_fixture_client_ids owned ON owned.client_id = rollup.client_id
        WHERE rollup.disk_sample_count <> rollup.sample_count
    ) OR EXISTS (
        SELECT 1
        FROM telemetry_resource_latest latest
        JOIN pressure_fixture_client_ids owned ON owned.client_id = latest.client_id
        WHERE latest.disk_sample_count <> latest.sample_count
    ) THEN
        RAISE EXCEPTION
            'pressure telemetry lost authoritative physical-disk semantics';
    END IF;
    IF skip_traffic THEN
        IF pressure_traffic_count <> 0
           OR pressure_stream_count <> 0
           OR pressure_hourly_count <> 0
           OR pressure_hourly_sample_count <> 0
           OR pressure_hourly_rx_bytes <> 0
           OR pressure_hourly_tx_bytes <> 0 THEN
            RAISE EXCEPTION
                'pressure import-only seed unexpectedly contains traffic: raw %, streams %, hourly %',
                pressure_traffic_count,
                pressure_stream_count,
                pressure_hourly_count;
        END IF;
    ELSE
        IF pressure_traffic_count <> 345720 OR EXISTS (
            SELECT 1
            FROM traffic_counter_samples sample
            JOIN pressure_fixture_client_ids owned ON owned.client_id = sample.client_id
            GROUP BY sample.client_id, sample.source_kind, sample.interface
            HAVING count(*) <> 2881
                OR max(sample.observed_at) - min(sample.observed_at)
                    <> interval '48 hours'
                OR min(sample.rx_counter_epoch) <> 0
                OR max(sample.rx_counter_epoch) <> 0
                OR min(sample.tx_counter_epoch) <> 0
                OR max(sample.tx_counter_epoch) <> 0
        ) THEN
            RAISE EXCEPTION
                'pressure raw traffic coverage mismatch: % rows',
                pressure_traffic_count;
        END IF;
        IF pressure_stream_count <> 120 OR EXISTS (
            SELECT 1
            FROM traffic_counter_hourly_usage_streams stream
            JOIN pressure_fixture_client_ids owned ON owned.client_id = stream.client_id
            WHERE (
                  stream.source_revision <> stream.materialized_revision
                  OR stream.source_revision <= 0
              )
        ) THEN
            RAISE EXCEPTION
                'pressure hourly stream coverage is absent or dirty: % streams',
                pressure_stream_count;
        END IF;
        IF pressure_hourly_count <> 5880
           OR pressure_hourly_sample_count <> 345720
           OR pressure_hourly_rx_bytes <> 345600000000::numeric
           OR pressure_hourly_tx_bytes <> 172800000000::numeric
           OR EXISTS (
                SELECT 1
                FROM traffic_counter_hourly_usage usage
                JOIN pressure_fixture_client_ids owned
                  ON owned.client_id = usage.client_id
                GROUP BY usage.client_id, usage.source_kind, usage.interface
                HAVING count(*) <> 49
                    OR sum(usage.sample_count) <> 2881
                    OR sum(usage.rx_bytes) <> 2880000000
                    OR sum(usage.tx_bytes) <> 1440000000
                    OR sum(usage.rx_reset_count) <> 0
                    OR sum(usage.tx_reset_count) <> 0
           ) THEN
            RAISE EXCEPTION
                'pressure hourly ledger mismatch: rows %, samples %, rx %, tx %',
                pressure_hourly_count,
                pressure_hourly_sample_count,
                pressure_hourly_rx_bytes,
                pressure_hourly_tx_bytes;
        END IF;
    END IF;
END;
$$;

COMMIT;
