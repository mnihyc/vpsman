CREATE TABLE telemetry_samples (
    id UUID PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    observed_at TIMESTAMPTZ NOT NULL,
    cpu_utilization_ratio DOUBLE PRECISION,
    cpu_cores INTEGER NOT NULL,
    cpu_load_1 DOUBLE PRECISION NOT NULL,
    cpu_load_5 DOUBLE PRECISION NOT NULL,
    cpu_load_15 DOUBLE PRECISION NOT NULL,
    memory_total_bytes BIGINT NOT NULL,
    memory_available_bytes BIGINT NOT NULL,
    swap_total_bytes BIGINT,
    swap_available_bytes BIGINT,
    disk_total_bytes BIGINT NOT NULL,
    disk_available_bytes BIGINT NOT NULL,
    network_rx_bytes BIGINT NOT NULL,
    network_tx_bytes BIGINT NOT NULL,
    tcp_sockets BIGINT NOT NULL,
    udp_sockets BIGINT NOT NULL,
    payload JSONB NOT NULL,
    CHECK (cpu_utilization_ratio IS NULL OR cpu_utilization_ratio BETWEEN 0 AND 1),
    CHECK (cpu_cores >= 0),
    CHECK (cpu_load_1 >= 0),
    CHECK (cpu_load_5 >= 0),
    CHECK (cpu_load_15 >= 0),
    CHECK (memory_total_bytes >= 0),
    CHECK (memory_available_bytes >= 0),
    CHECK (
        (swap_total_bytes IS NULL AND swap_available_bytes IS NULL)
        OR (
            swap_total_bytes >= 0
            AND swap_available_bytes BETWEEN 0 AND swap_total_bytes
        )
    ),
    CHECK (disk_total_bytes >= 0),
    CHECK (disk_available_bytes >= 0),
    CHECK (network_rx_bytes >= 0),
    CHECK (network_tx_bytes >= 0),
    CHECK (tcp_sockets >= 0),
    CHECK (udp_sockets >= 0),
    CHECK (jsonb_typeof(payload) = 'object')
);

CREATE INDEX telemetry_samples_client_latest_idx
    ON telemetry_samples (client_id, observed_at DESC);

CREATE INDEX telemetry_samples_retention_idx
    ON telemetry_samples (observed_at);

CREATE TABLE telemetry_counter_facts (
    sample_id UUID NOT NULL REFERENCES telemetry_samples(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    source_kind TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    interface TEXT NOT NULL,
    rx_bytes BIGINT NOT NULL,
    tx_bytes BIGINT NOT NULL,
    PRIMARY KEY (sample_id, source_kind, ordinal),
    CHECK (source_kind IN ('host', 'tunnel')),
    CHECK (ordinal >= 0),
    CHECK (length(interface) BETWEEN 1 AND 64),
    CHECK (rx_bytes >= 0),
    CHECK (tx_bytes >= 0)
);

CREATE INDEX telemetry_counter_facts_client_source_time_idx
    ON telemetry_counter_facts (
        client_id,
        source_kind,
        observed_at,
        sample_id,
        ordinal
    ) INCLUDE (interface, rx_bytes, tx_bytes);

CREATE INDEX telemetry_counter_facts_client_source_interface_time_idx
    ON telemetry_counter_facts (
        client_id,
        source_kind,
        interface,
        observed_at,
        sample_id,
        ordinal
    ) INCLUDE (rx_bytes, tx_bytes);

CREATE TABLE telemetry_rollups (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    bucket_start TIMESTAMPTZ NOT NULL,
    bucket_secs INTEGER NOT NULL,
    sample_count INTEGER NOT NULL,
    cpu_usage_sample_count INTEGER NOT NULL DEFAULT 0,
    cpu_usage_sum DOUBLE PRECISION NOT NULL DEFAULT 0,
    cpu_usage_avg DOUBLE PRECISION,
    cpu_usage_max DOUBLE PRECISION,
    cpu_cores_max INTEGER NOT NULL DEFAULT 0,
    cpu_load_1_avg DOUBLE PRECISION NOT NULL,
    cpu_load_1_sum DOUBLE PRECISION NOT NULL DEFAULT 0,
    cpu_load_1_max DOUBLE PRECISION NOT NULL,
    cpu_load_5_avg DOUBLE PRECISION NOT NULL DEFAULT 0,
    cpu_load_5_sum DOUBLE PRECISION NOT NULL DEFAULT 0,
    cpu_load_5_max DOUBLE PRECISION NOT NULL DEFAULT 0,
    cpu_load_15_avg DOUBLE PRECISION NOT NULL DEFAULT 0,
    cpu_load_15_sum DOUBLE PRECISION NOT NULL DEFAULT 0,
    cpu_load_15_max DOUBLE PRECISION NOT NULL DEFAULT 0,
    memory_total_bytes_max BIGINT NOT NULL,
    memory_available_bytes_avg BIGINT NOT NULL,
    memory_available_bytes_sum NUMERIC(39, 0) NOT NULL DEFAULT 0,
    memory_available_bytes_min BIGINT NOT NULL,
    memory_used_ratio_avg DOUBLE PRECISION NOT NULL,
    memory_used_ratio_sum DOUBLE PRECISION NOT NULL DEFAULT 0,
    memory_used_ratio_max DOUBLE PRECISION NOT NULL,
    swap_sample_count INTEGER NOT NULL DEFAULT 0,
    swap_total_bytes_max BIGINT,
    swap_available_bytes_avg BIGINT,
    swap_available_bytes_sum NUMERIC(39, 0) NOT NULL DEFAULT 0,
    swap_available_bytes_min BIGINT,
    swap_used_ratio_avg DOUBLE PRECISION,
    swap_used_ratio_sum DOUBLE PRECISION NOT NULL DEFAULT 0,
    swap_used_ratio_max DOUBLE PRECISION,
    disk_total_bytes_max BIGINT NOT NULL DEFAULT 0,
    disk_available_bytes_avg BIGINT NOT NULL DEFAULT 0,
    disk_available_bytes_sum NUMERIC(39, 0) NOT NULL DEFAULT 0,
    disk_available_bytes_min BIGINT NOT NULL DEFAULT 0,
    disk_used_ratio_avg DOUBLE PRECISION NOT NULL DEFAULT 0,
    disk_used_ratio_sum DOUBLE PRECISION NOT NULL DEFAULT 0,
    disk_used_ratio_max DOUBLE PRECISION NOT NULL DEFAULT 0,
    network_rx_bytes_max BIGINT NOT NULL DEFAULT 0,
    network_tx_bytes_max BIGINT NOT NULL DEFAULT 0,
    connections_sample_count INTEGER NOT NULL DEFAULT 0,
    tcp_sockets_latest BIGINT,
    udp_sockets_latest BIGINT,
    connections_observed_at TIMESTAMPTZ,
    latest_observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, bucket_secs, bucket_start),
    CHECK (bucket_secs >= 60 AND bucket_secs % 60 = 0),
    CHECK (bucket_start = date_trunc('minute', bucket_start)),
    CHECK (sample_count > 0),
    CHECK (cpu_usage_avg IS NULL OR cpu_usage_avg BETWEEN 0 AND 1),
    CHECK (cpu_usage_max IS NULL OR cpu_usage_max BETWEEN 0 AND 1),
    CHECK (cpu_usage_sample_count BETWEEN 0 AND sample_count),
    CHECK (cpu_cores_max >= 0),
    CHECK (memory_used_ratio_avg BETWEEN 0 AND 1),
    CHECK (memory_used_ratio_max BETWEEN 0 AND 1),
    CHECK (swap_sample_count BETWEEN 0 AND sample_count),
    CHECK ((
        (swap_sample_count = 0 AND (
            (swap_total_bytes_max IS NULL
                AND swap_available_bytes_avg IS NULL
                AND swap_available_bytes_min IS NULL
            )
            OR (swap_total_bytes_max = 0
                AND swap_available_bytes_avg = 0
                AND swap_available_bytes_min = 0
            )
        )
            AND swap_used_ratio_avg IS NULL
            AND swap_used_ratio_max IS NULL
        )
        OR (swap_sample_count > 0
            AND swap_total_bytes_max > 0
            AND swap_available_bytes_avg IS NOT NULL
            AND swap_available_bytes_min IS NOT NULL
            AND swap_used_ratio_avg IS NOT NULL
            AND swap_used_ratio_max IS NOT NULL
        )
    ) IS TRUE),
    CHECK (
        swap_total_bytes_max IS NULL OR (
            swap_total_bytes_max >= 0
            AND swap_available_bytes_avg >= 0
            AND swap_available_bytes_min >= 0
            AND swap_available_bytes_min <= swap_available_bytes_avg
            AND swap_available_bytes_avg <= swap_total_bytes_max
        )
    ),
    CHECK (swap_used_ratio_avg IS NULL OR swap_used_ratio_avg BETWEEN 0 AND 1),
    CHECK (swap_used_ratio_max IS NULL OR swap_used_ratio_max BETWEEN 0 AND 1),
    CHECK (disk_used_ratio_avg BETWEEN 0 AND 1),
    CHECK (disk_used_ratio_max BETWEEN 0 AND 1),
    CHECK (connections_sample_count BETWEEN 0 AND sample_count),
    CHECK ((connections_sample_count = 0) = (connections_observed_at IS NULL)),
    CHECK (
        connections_observed_at IS NULL OR (
            connections_observed_at >= bucket_start
            AND connections_observed_at < bucket_start + make_interval(secs => bucket_secs)
        )
    ),
    CHECK (
        latest_observed_at >= bucket_start
        AND latest_observed_at < bucket_start + make_interval(secs => bucket_secs)
    ),
    CHECK ((connections_sample_count = 0) = (tcp_sockets_latest IS NULL)),
    CHECK ((tcp_sockets_latest IS NULL) = (udp_sockets_latest IS NULL)),
    CHECK (tcp_sockets_latest IS NULL OR tcp_sockets_latest >= 0),
    CHECK (udp_sockets_latest IS NULL OR udp_sockets_latest >= 0)
);

CREATE INDEX telemetry_rollups_latest_idx
    ON telemetry_rollups (bucket_secs, bucket_start DESC, client_id);

CREATE INDEX telemetry_rollups_client_latest_idx
    ON telemetry_rollups (client_id, bucket_start DESC, bucket_secs);

CREATE INDEX telemetry_rollups_retention_idx
    ON telemetry_rollups (bucket_start);

-- Current fleet cards must remain exact even after their source minute is
-- compacted into historical tiers. This is one bounded row per client.
CREATE TABLE telemetry_resource_latest (
    LIKE telemetry_rollups INCLUDING DEFAULTS INCLUDING CONSTRAINTS
);

ALTER TABLE telemetry_resource_latest
    ADD PRIMARY KEY (client_id),
    ADD FOREIGN KEY (client_id) REFERENCES clients(id) ON DELETE CASCADE;

CREATE INDEX telemetry_resource_latest_observed_idx
    ON telemetry_resource_latest (latest_observed_at DESC, client_id);

CREATE TABLE telemetry_network_rates (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    interface TEXT NOT NULL,
    bucket_start TIMESTAMPTZ NOT NULL,
    bucket_secs INTEGER NOT NULL,
    sample_count INTEGER NOT NULL,
    rx_bytes_sum NUMERIC(39, 0) NOT NULL DEFAULT 0,
    tx_bytes_sum NUMERIC(39, 0) NOT NULL DEFAULT 0,
    rx_bytes_avg BIGINT NOT NULL,
    tx_bytes_avg BIGINT NOT NULL,
    rx_bytes_last BIGINT NOT NULL,
    tx_bytes_last BIGINT NOT NULL,
    rx_counter_epoch BIGINT NOT NULL DEFAULT 0,
    tx_counter_epoch BIGINT NOT NULL DEFAULT 0,
    latest_observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, interface, bucket_secs, bucket_start),
    CHECK (bucket_secs >= 60 AND bucket_secs % 60 = 0),
    CHECK (bucket_start = date_trunc('minute', bucket_start)),
    CHECK (sample_count > 0),
    CHECK (rx_bytes_avg >= 0 AND tx_bytes_avg >= 0),
    CHECK (rx_bytes_last >= 0 AND tx_bytes_last >= 0),
    CHECK (rx_counter_epoch >= 0 AND tx_counter_epoch >= 0),
    CHECK (
        latest_observed_at >= bucket_start
        AND latest_observed_at < bucket_start + make_interval(secs => bucket_secs)
    )
);

CREATE INDEX telemetry_network_rates_latest_idx
    ON telemetry_network_rates (bucket_secs, bucket_start DESC, client_id, interface);

CREATE INDEX telemetry_network_rates_client_latest_idx
    ON telemetry_network_rates (client_id, interface, bucket_start DESC, bucket_secs);

CREATE INDEX telemetry_network_rates_retention_idx
    ON telemetry_network_rates (bucket_start);

CREATE TABLE ping_targets (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    host TEXT NOT NULL,
    probe_kind TEXT NOT NULL,
    port INTEGER,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    selector_expression TEXT NOT NULL DEFAULT '*',
    generation BIGINT NOT NULL DEFAULT 1,
    created_by UUID REFERENCES operators(id),
    updated_by UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (length(trim(name)) BETWEEN 1 AND 128),
    CHECK (length(trim(host)) BETWEEN 1 AND 253),
    CHECK (probe_kind IN ('icmp', 'tcp')),
    CHECK (
        (probe_kind = 'icmp' AND port IS NULL)
        OR (probe_kind = 'tcp' AND port BETWEEN 1 AND 65535)
    ),
    CHECK (length(trim(selector_expression)) BETWEEN 1 AND 4096),
    CHECK (generation > 0)
);

CREATE UNIQUE INDEX ping_targets_name_unique_idx
    ON ping_targets (lower(name));

CREATE INDEX ping_targets_updated_idx
    ON ping_targets (updated_at DESC, name);

CREATE TABLE ping_target_assignments (
    target_id UUID NOT NULL REFERENCES ping_targets(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    is_primary BOOLEAN NOT NULL DEFAULT FALSE,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (target_id, client_id)
);

CREATE INDEX ping_target_assignments_client_idx
    ON ping_target_assignments (client_id, target_id);

CREATE UNIQUE INDEX ping_target_assignments_one_primary_per_client_idx
    ON ping_target_assignments (client_id)
    WHERE is_primary;

CREATE TABLE telemetry_ping_series (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    target_id UUID NOT NULL REFERENCES ping_targets(id) ON DELETE CASCADE,
    generation BIGINT NOT NULL,
    UNIQUE (client_id, target_id, generation),
    CHECK (generation > 0)
);

CREATE TABLE telemetry_ping_facts (
    series_id BIGINT NOT NULL REFERENCES telemetry_ping_series(id) ON DELETE CASCADE,
    observed_at TIMESTAMPTZ NOT NULL,
    evidence_id UUID NOT NULL,
    source_checked_unix BIGINT NOT NULL,
    checked_unix BIGINT NOT NULL,
    status TEXT NOT NULL,
    latency_avg_ms DOUBLE PRECISION,
    loss_ratio DOUBLE PRECISION NOT NULL,
    reason TEXT,
    PRIMARY KEY (series_id, source_checked_unix),
    CHECK (source_checked_unix > 0),
    CHECK (checked_unix > 0),
    CHECK (checked_unix <= extract(epoch FROM observed_at)::bigint + 300),
    CHECK (extract(epoch FROM observed_at)::bigint - checked_unix <= 3900),
    CHECK (status IN ('ok', 'degraded', 'down', 'error')),
    CHECK (latency_avg_ms IS NULL OR latency_avg_ms BETWEEN 0 AND 3600000),
    CHECK (loss_ratio BETWEEN 0 AND 1),
    CHECK (reason IS NULL OR length(reason) <= 4096),
    CHECK (
        (status = 'ok' AND latency_avg_ms IS NOT NULL AND loss_ratio = 0)
        OR (
            status = 'degraded'
            AND latency_avg_ms IS NOT NULL
            AND loss_ratio > 0
            AND loss_ratio < 1
        )
        OR (
            status IN ('down', 'error')
            AND latency_avg_ms IS NULL
            AND loss_ratio = 1
        )
    )
);

CREATE INDEX telemetry_ping_facts_retention_idx
    ON telemetry_ping_facts (observed_at);

-- Logical identity uses the agent's source clock, while history ranges use the
-- API-rebased clock. Preserve the prior series/range access path explicitly.
CREATE INDEX telemetry_ping_facts_series_checked_idx
    ON telemetry_ping_facts (series_id, checked_unix);

CREATE TABLE telemetry_ping_rollups (
    series_id BIGINT NOT NULL REFERENCES telemetry_ping_series(id) ON DELETE CASCADE,
    bucket_start TIMESTAMPTZ NOT NULL,
    bucket_secs INTEGER NOT NULL,
    sample_count INTEGER NOT NULL,
    success_count INTEGER NOT NULL,
    latency_sum_ms DOUBLE PRECISION NOT NULL DEFAULT 0,
    latency_avg_ms DOUBLE PRECISION,
    latency_min_ms DOUBLE PRECISION,
    latency_max_ms DOUBLE PRECISION,
    loss_ratio_avg DOUBLE PRECISION NOT NULL,
    loss_ratio_sum DOUBLE PRECISION NOT NULL DEFAULT 0,
    loss_ratio_max DOUBLE PRECISION NOT NULL,
    latest_status TEXT NOT NULL,
    latest_reason TEXT,
    latest_checked_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (series_id, bucket_secs, bucket_start),
    CHECK (bucket_secs >= 60 AND bucket_secs % 60 = 0),
    CHECK (bucket_start = date_trunc('minute', bucket_start)),
    CHECK (sample_count > 0),
    CHECK (success_count BETWEEN 0 AND sample_count),
    CHECK (
        latest_checked_at >= bucket_start
        AND latest_checked_at < bucket_start + make_interval(secs => bucket_secs)
    ),
    CHECK (latency_avg_ms IS NULL OR latency_avg_ms >= 0),
    CHECK (latency_min_ms IS NULL OR latency_min_ms >= 0),
    CHECK (latency_max_ms IS NULL OR latency_max_ms >= 0),
    CHECK (loss_ratio_avg BETWEEN 0 AND 1),
    CHECK (loss_ratio_max BETWEEN 0 AND 1),
    CHECK (latest_status IN ('ok', 'degraded', 'down', 'error')),
    CHECK (latest_reason IS NULL OR length(latest_reason) <= 512)
);

CREATE INDEX telemetry_ping_rollups_lookup_idx
    ON telemetry_ping_rollups (series_id, bucket_start DESC);

CREATE INDEX telemetry_ping_rollups_current_latest_idx
    ON telemetry_ping_rollups (
        series_id,
        latest_checked_at DESC,
        bucket_start DESC
    );

CREATE INDEX telemetry_ping_rollups_current_range_idx
    ON telemetry_ping_rollups (
        series_id,
        bucket_start DESC
    ) INCLUDE (bucket_secs, sample_count, loss_ratio_avg);

CREATE INDEX telemetry_ping_rollups_retention_idx
    ON telemetry_ping_rollups (bucket_start);

-- Exact current state is intentionally separate from compacted chart history.
CREATE TABLE telemetry_ping_current (
    series_id BIGINT PRIMARY KEY REFERENCES telemetry_ping_series(id) ON DELETE CASCADE,
    latest_status TEXT NOT NULL,
    latency_avg_ms DOUBLE PRECISION,
    rolling_loss_ratio DOUBLE PRECISION NOT NULL,
    latest_reason TEXT,
    latest_checked_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (latest_status IN ('ok', 'degraded', 'down', 'error')),
    CHECK (latency_avg_ms IS NULL OR latency_avg_ms BETWEEN 0 AND 3600000),
    CHECK (rolling_loss_ratio BETWEEN 0 AND 1),
    CHECK (latest_reason IS NULL OR length(latest_reason) <= 512)
);

CREATE INDEX telemetry_ping_current_latest_idx
    ON telemetry_ping_current (latest_checked_at DESC, series_id);

CREATE TABLE monitoring_share_links (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    token_secret TEXT NOT NULL UNIQUE,
    selector_expression TEXT NOT NULL,
    show_identity_context BOOLEAN NOT NULL DEFAULT FALSE,
    show_billing BOOLEAN NOT NULL DEFAULT FALSE,
    show_system_information BOOLEAN NOT NULL DEFAULT FALSE,
    show_resources BOOLEAN NOT NULL DEFAULT TRUE,
    show_network BOOLEAN NOT NULL DEFAULT TRUE,
    show_traffic BOOLEAN NOT NULL DEFAULT TRUE,
    show_ping BOOLEAN NOT NULL DEFAULT TRUE,
    allow_detail_history BOOLEAN NOT NULL DEFAULT TRUE,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    revoked_by UUID REFERENCES operators(id),
    created_by UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (length(trim(name)) BETWEEN 1 AND 128),
    CHECK (token_secret ~ '^[0-9a-f]{64}$'),
    -- A frozen grid selection can contain up to 1,000 explicit v-* IDs.
    CHECK (length(trim(selector_expression)) BETWEEN 1 AND 65535),
    CHECK (expires_at > created_at)
);

CREATE INDEX monitoring_share_links_status_idx
    ON monitoring_share_links (revoked_at, expires_at, created_at DESC);

CREATE TABLE monitoring_share_targets (
    share_id UUID NOT NULL REFERENCES monitoring_share_links(id) ON DELETE CASCADE,
    -- The client row is the immutable tombstone identity for a frozen share.
    -- Logical deletion hides all live data but must not rewrite historical scope.
    client_id TEXT NOT NULL REFERENCES clients(id),
    -- Random per-share identity exposed to visitors. It is deliberately persisted
    -- rather than derived from the predictable internal v-N client ID.
    public_client_key TEXT NOT NULL,
    PRIMARY KEY (share_id, client_id),
    UNIQUE (share_id, public_client_key),
    CHECK (public_client_key ~ '^[0-9a-f]{64}$')
);

CREATE INDEX monitoring_share_targets_client_idx
    ON monitoring_share_targets (client_id, share_id);

CREATE TABLE monitoring_share_visitors (
    share_id UUID NOT NULL REFERENCES monitoring_share_links(id) ON DELETE CASCADE,
    visitor_id UUID NOT NULL,
    source_ip INET,
    user_agent TEXT,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (share_id, visitor_id),
    CHECK (user_agent IS NULL OR length(user_agent) <= 512)
);

CREATE INDEX monitoring_share_visitors_last_seen_idx
    ON monitoring_share_visitors (share_id, last_seen_at DESC);

CREATE TABLE telemetry_ingest_watermarks (
    client_id TEXT PRIMARY KEY REFERENCES clients(id) ON DELETE CASCADE,
    process_incarnation_id UUID NOT NULL,
    telemetry_seq BIGINT NOT NULL CHECK (telemetry_seq > 0),
    reported_observed_unix BIGINT NOT NULL CHECK (reported_observed_unix >= 0),
    accepted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    gateway_session_id UUID NOT NULL
);

CREATE TABLE telemetry_tunnels (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    observed_at TIMESTAMPTZ NOT NULL,
    interface TEXT NOT NULL,
    kind TEXT NOT NULL,
    ownership_mode TEXT NOT NULL,
    mutation_policy TEXT NOT NULL,
    source TEXT NOT NULL,
    operstate TEXT,
    mtu BIGINT,
    link_type BIGINT,
    address TEXT,
    rx_bytes BIGINT NOT NULL DEFAULT 0,
    tx_bytes BIGINT NOT NULL DEFAULT 0,
    traffic_source TEXT,
    traffic_status TEXT,
    traffic_reason TEXT,
    traffic_checked_unix BIGINT,
    telemetry_plan_id TEXT,
    telemetry_plan_name TEXT,
    telemetry_plan_runtime_manager TEXT,
    telemetry_endpoint_side TEXT,
    telemetry_peer_client_id TEXT,
    adapter_health JSONB,
    latency_monitoring_enabled BOOLEAN,
    latency_status TEXT,
    latency_reason TEXT,
    latency_primary_family TEXT,
    latency_target TEXT,
    latency_checked_unix BIGINT,
    latency_avg_ms DOUBLE PRECISION,
    packet_loss_ratio DOUBLE PRECISION,
    latency_healthy_windows INTEGER,
    latency_missed_windows INTEGER,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, interface)
);

CREATE INDEX telemetry_tunnels_latest_idx
    ON telemetry_tunnels (observed_at DESC, client_id, interface);

CREATE TABLE vps_rule_values (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value_raw TEXT NOT NULL,
    value_json JSONB NOT NULL,
    source_kind TEXT NOT NULL DEFAULT 'operator',
    source_id UUID,
    updated_by UUID REFERENCES operators(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, key),
    CHECK (key IN (
        'traffic.reset_day',
        'traffic.quota.total',
        'traffic.quota.rx',
        'traffic.quota.tx',
        'traffic.selectors',
        'billing.price',
        'billing.cycle',
        'network.port_speed',
        'network.rate.interfaces',
        'product.name'
    )),
    CHECK (length(value_raw) BETWEEN 1 AND 4096),
    CHECK (jsonb_typeof(value_json) = 'object')
);

CREATE INDEX vps_rule_values_key_idx
    ON vps_rule_values (key, client_id);

CREATE TABLE traffic_counter_samples (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL,
    interface TEXT NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    rx_bytes BIGINT NOT NULL,
    tx_bytes BIGINT NOT NULL,
    rx_counter_epoch BIGINT NOT NULL DEFAULT 0,
    tx_counter_epoch BIGINT NOT NULL DEFAULT 0,
    sample_source TEXT NOT NULL,
    inbound_promoted BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (client_id, source_kind, interface, observed_at),
    CHECK (source_kind IN ('host', 'tunnel')),
    CHECK (length(interface) BETWEEN 1 AND 128),
    CHECK (observed_at = date_trunc('minute', observed_at)),
    CHECK (rx_bytes >= 0),
    CHECK (tx_bytes >= 0),
    CHECK (rx_counter_epoch >= 0),
    CHECK (tx_counter_epoch >= 0)
);

CREATE INDEX traffic_counter_samples_lookup_idx
    ON traffic_counter_samples (client_id, source_kind, interface, observed_at DESC);

CREATE INDEX traffic_counter_samples_observed_idx
    ON traffic_counter_samples (observed_at DESC);

-- Long-term traffic history stores counter transitions rather than counter
-- endpoints. A transition belongs to its later sample, so the sums remain
-- exact across promotion boundaries and counter resets never become traffic.
-- Imported and live contributions are separate to make vnStat re-imports
-- replaceable without touching traffic observed by the running agent.
CREATE TABLE traffic_counter_rollups (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL,
    interface TEXT NOT NULL,
    origin_kind TEXT NOT NULL,
    bucket_secs INTEGER NOT NULL,
    bucket_start TIMESTAMPTZ NOT NULL,
    rx_bytes BIGINT NOT NULL DEFAULT 0,
    tx_bytes BIGINT NOT NULL DEFAULT 0,
    rx_valid_count INTEGER NOT NULL DEFAULT 0,
    tx_valid_count INTEGER NOT NULL DEFAULT 0,
    any_valid_count INTEGER NOT NULL DEFAULT 0,
    rx_reset_count INTEGER NOT NULL DEFAULT 0,
    tx_reset_count INTEGER NOT NULL DEFAULT 0,
    any_reset_count INTEGER NOT NULL DEFAULT 0,
    first_observed_at TIMESTAMPTZ NOT NULL,
    latest_observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (
        client_id,
        source_kind,
        interface,
        origin_kind,
        bucket_secs,
        bucket_start
    ),
    CHECK (source_kind IN ('host', 'tunnel')),
    CHECK (length(interface) BETWEEN 1 AND 128),
    CHECK (origin_kind IN ('live', 'vnstat_import')),
    CHECK (bucket_secs IN (3600, 10800, 21600, 86400)),
    CHECK (
        extract(epoch FROM bucket_start)::bigint % bucket_secs::bigint = 0
    ),
    CHECK (rx_bytes >= 0 AND tx_bytes >= 0),
    CHECK (
        rx_valid_count >= 0
        AND tx_valid_count >= 0
        AND any_valid_count >= 0
        AND rx_reset_count >= 0
        AND tx_reset_count >= 0
        AND any_reset_count >= 0
    ),
    CHECK (first_observed_at <= latest_observed_at),
    CHECK (first_observed_at >= bucket_start),
    CHECK (
        latest_observed_at
            < bucket_start + make_interval(secs => bucket_secs)
    )
);

CREATE INDEX traffic_counter_rollups_range_idx
    ON traffic_counter_rollups (
        client_id,
        source_kind,
        interface,
        bucket_start,
        bucket_secs
    ) INCLUDE (
        origin_kind,
        rx_bytes,
        tx_bytes,
        rx_valid_count,
        tx_valid_count,
        any_valid_count,
        rx_reset_count,
        tx_reset_count,
        any_reset_count,
        first_observed_at,
        latest_observed_at
    );

CREATE INDEX traffic_counter_rollups_retention_idx
    ON traffic_counter_rollups (bucket_secs, bucket_start);

CREATE TABLE traffic_cycle_usage (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    selector_hash TEXT NOT NULL,
    selectors_json JSONB NOT NULL,
    cycle_start TIMESTAMPTZ NOT NULL,
    cycle_end TIMESTAMPTZ NOT NULL,
    rx_bytes BIGINT NOT NULL DEFAULT 0,
    tx_bytes BIGINT NOT NULL DEFAULT 0,
    total_bytes BIGINT NOT NULL DEFAULT 0,
    quota_rx_bytes BIGINT,
    quota_tx_bytes BIGINT,
    quota_total_bytes BIGINT,
    cycle_percent DOUBLE PRECISION,
    state TEXT NOT NULL,
    incomplete_reasons TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    last_sample_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, selector_hash, cycle_start),
    CHECK (jsonb_typeof(selectors_json) = 'array'),
    CHECK (state IN ('ok', 'incomplete', 'unknown', 'stale'))
);

CREATE INDEX traffic_cycle_usage_client_idx
    ON traffic_cycle_usage (client_id, cycle_start DESC);

CREATE TABLE policy_groups (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    selector_expression TEXT NOT NULL,
    notes TEXT,
    created_by UUID REFERENCES operators(id),
    updated_by UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (length(trim(name)) BETWEEN 1 AND 128),
    CHECK (length(trim(selector_expression)) BETWEEN 1 AND 4096),
    CHECK (notes IS NULL OR length(notes) <= 1024)
);

CREATE INDEX policy_groups_enabled_idx
    ON policy_groups (enabled, updated_at DESC, name);

CREATE TABLE policy_rules (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES policy_groups(id) ON DELETE CASCADE,
    rule_version INTEGER NOT NULL DEFAULT 1,
    sort_order INTEGER NOT NULL DEFAULT 0,
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    traffic_selector TEXT,
    condition_expression TEXT NOT NULL,
    window_secs BIGINT NOT NULL DEFAULT 0,
    severity TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (length(trim(name)) BETWEEN 1 AND 128),
    CHECK (severity IN ('info', 'warning', 'critical')),
    CHECK (window_secs IN (0, 60, 300, 900)),
    CHECK (length(trim(condition_expression)) BETWEEN 1 AND 4096)
);

CREATE INDEX policy_rules_group_idx
    ON policy_rules (group_id, sort_order ASC, created_at ASC);

INSERT INTO policy_groups (
    id, name, enabled, selector_expression, notes
) VALUES
    (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1',
        'Predefined CPU load warning',
        FALSE,
        'status:online',
        'Disabled predefined policy. Enable or edit after confirming fleet-specific CPU expectations.'
    ),
    (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2',
        'Predefined memory pressure',
        FALSE,
        'status:online',
        'Disabled predefined policy. Enable or edit after confirming memory pressure thresholds.'
    ),
    (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3',
        'Predefined traffic quota warning',
        FALSE,
        'status:online',
        'Disabled predefined policy. Enable or edit after configuring VPS traffic quota rules.'
    )
ON CONFLICT (name) DO NOTHING;

INSERT INTO policy_rules (
    id,
    group_id,
    sort_order,
    name,
    enabled,
    traffic_selector,
    condition_expression,
    window_secs,
    severity
)
SELECT
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1',
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1',
    0,
    'CPU load above 1.5',
    TRUE,
    NULL,
    'cpu.load_1 >= 1.5',
    300,
    'warning'
WHERE EXISTS (
    SELECT 1 FROM policy_groups
    WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO policy_rules (
    id,
    group_id,
    sort_order,
    name,
    enabled,
    traffic_selector,
    condition_expression,
    window_secs,
    severity
)
SELECT
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb2',
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2',
    0,
    'Available memory below 15%',
    TRUE,
    NULL,
    'memory.available_ratio <= 0.15',
    300,
    'warning'
WHERE EXISTS (
    SELECT 1 FROM policy_groups
    WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO policy_rules (
    id,
    group_id,
    sort_order,
    name,
    enabled,
    traffic_selector,
    condition_expression,
    window_secs,
    severity
)
SELECT
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb3',
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3',
    0,
    'Traffic cycle above 80%',
    TRUE,
    NULL,
    'traffic.cycle_percent >= 80',
    300,
    'warning'
WHERE EXISTS (
    SELECT 1 FROM policy_groups
    WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3'
)
ON CONFLICT (id) DO NOTHING;

CREATE TABLE policy_rule_states (
    policy_rule_id UUID NOT NULL REFERENCES policy_rules(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    rule_version INTEGER NOT NULL,
    condition_true BOOLEAN NOT NULL,
    previous_condition_true BOOLEAN NOT NULL,
    window_satisfied BOOLEAN NOT NULL,
    first_true_at TIMESTAMPTZ,
    last_true_at TIMESTAMPTZ,
    last_false_at TIMESTAMPTZ,
    last_evaluated_at TIMESTAMPTZ NOT NULL,
    incomplete BOOLEAN NOT NULL,
    incomplete_reasons TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    last_actual_value DOUBLE PRECISION,
    last_threshold_value DOUBLE PRECISION,
    last_fired_at TIMESTAMPTZ,
    trigger_generation BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (policy_rule_id, client_id, rule_version)
);

CREATE INDEX policy_rule_states_client_idx
    ON policy_rule_states (client_id, updated_at DESC);

CREATE TABLE policy_alerts (
    id UUID PRIMARY KEY,
    policy_group_id UUID NOT NULL,
    policy_rule_id UUID NOT NULL,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    trigger_generation BIGINT NOT NULL,
    severity TEXT NOT NULL,
    category TEXT NOT NULL,
    title TEXT NOT NULL,
    detail TEXT NOT NULL,
    actual_value DOUBLE PRECISION,
    threshold_value DOUBLE PRECISION,
    payload JSONB NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (policy_rule_id, client_id, trigger_generation),
    CHECK (severity IN ('info', 'warning', 'critical')),
    CHECK (category IN ('traffic', 'resource')),
    CHECK (jsonb_typeof(payload) = 'object')
);

CREATE INDEX policy_alerts_recent_idx
    ON policy_alerts (observed_at DESC, client_id, severity);

CREATE INDEX policy_alerts_client_idx
    ON policy_alerts (client_id, observed_at DESC);

CREATE INDEX policy_alerts_fleet_priority_idx
    ON policy_alerts (
        (CASE severity
            WHEN 'critical' THEN 0
            WHEN 'warning' THEN 1
            WHEN 'info' THEN 2
            ELSE 3
        END),
        observed_at DESC,
        id DESC
    );

CREATE INDEX policy_alerts_client_fleet_priority_idx
    ON policy_alerts (
        client_id,
        (CASE severity
            WHEN 'critical' THEN 0
            WHEN 'warning' THEN 1
            WHEN 'info' THEN 2
            ELSE 3
        END),
        observed_at DESC,
        id DESC
    );

CREATE TABLE fleet_alert_states (
    alert_id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    muted_until_unix BIGINT,
    escalation_level INTEGER NOT NULL DEFAULT 0,
    reason TEXT,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (state IN ('open', 'acknowledged', 'muted', 'escalated')),
    CHECK (escalation_level >= 0),
    CHECK (
        (state = 'muted' AND muted_until_unix IS NOT NULL)
        OR state <> 'muted'
    )
);

CREATE INDEX fleet_alert_states_state_idx
    ON fleet_alert_states (state, updated_at DESC);

CREATE INDEX fleet_alert_states_updated_idx
    ON fleet_alert_states (updated_at DESC, alert_id ASC);

CREATE TABLE fleet_alert_notification_channels (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    scope_kind TEXT NOT NULL,
    scope_value TEXT,
    min_severity TEXT NOT NULL,
    categories JSONB NOT NULL DEFAULT '[]'::jsonb,
    operator_states JSONB NOT NULL DEFAULT '[]'::jsonb,
    delivery_kind TEXT NOT NULL,
    target TEXT NOT NULL,
    cooldown_secs BIGINT NOT NULL DEFAULT 3600,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    notes TEXT,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (scope_kind IN ('global', 'provider', 'tag', 'client')),
    CHECK (
        (scope_kind = 'global' AND scope_value IS NULL)
        OR (scope_kind <> 'global' AND scope_value IS NOT NULL)
    ),
    CHECK (min_severity IN ('info', 'warning', 'critical')),
    CHECK (jsonb_typeof(categories) = 'array'),
    CHECK (jsonb_typeof(operator_states) = 'array'),
    CHECK (cooldown_secs >= 0 AND cooldown_secs <= 2592000)
);

CREATE INDEX fleet_alert_notification_channels_match_idx
    ON fleet_alert_notification_channels (
        enabled,
        scope_kind,
        scope_value,
        min_severity,
        delivery_kind,
        updated_at DESC
    );

CREATE TABLE fleet_alert_notification_deliveries (
    id UUID PRIMARY KEY,
    channel_id UUID NOT NULL,
    channel_name TEXT NOT NULL,
    alert_id TEXT NOT NULL,
    alert_severity TEXT NOT NULL,
    alert_category TEXT NOT NULL,
    status TEXT NOT NULL,
    delivery_kind TEXT NOT NULL,
    target TEXT NOT NULL,
    dedupe_key TEXT NOT NULL,
    payload JSONB NOT NULL,
    error TEXT,
    cooldown_until_unix BIGINT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    delivery_lease_id UUID,
    delivery_lease_until TIMESTAMPTZ,
    next_attempt_at TIMESTAMPTZ,
    last_attempt_at TIMESTAMPTZ,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at TIMESTAMPTZ,
    CHECK (status IN ('queued', 'in_progress', 'failed', 'permanently_failed', 'canceled_disabled', 'delivered', 'matched_dry_run')),
    CHECK (alert_severity IN ('info', 'warning', 'critical')),
    CHECK (cooldown_until_unix >= 0)
);

CREATE INDEX fleet_alert_notification_deliveries_status_idx
    ON fleet_alert_notification_deliveries (status, created_at DESC);

CREATE INDEX fleet_alert_notification_deliveries_dedupe_idx
    ON fleet_alert_notification_deliveries (dedupe_key, cooldown_until_unix DESC);

CREATE INDEX fleet_alert_notification_deliveries_alert_idx
    ON fleet_alert_notification_deliveries (alert_id, created_at DESC);

CREATE INDEX fleet_alert_notification_deliveries_created_idx
    ON fleet_alert_notification_deliveries (created_at DESC, alert_id ASC);

CREATE INDEX fleet_alert_notification_deliveries_channel_created_idx
    ON fleet_alert_notification_deliveries (channel_id, created_at DESC, alert_id ASC);

CREATE INDEX fleet_alert_notification_deliveries_attempt_idx
    ON fleet_alert_notification_deliveries (
        status,
        next_attempt_at ASC,
        created_at ASC
    );

CREATE TABLE webhook_rules (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    expression TEXT NOT NULL,
    target TEXT NOT NULL,
    body_template TEXT NOT NULL DEFAULT '',
    signing_secret TEXT,
    cooldown_secs BIGINT NOT NULL DEFAULT 300,
    notes TEXT,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (length(trim(name)) BETWEEN 1 AND 128),
    CHECK (length(trim(expression)) BETWEEN 1 AND 4096),
    CHECK (length(trim(target)) BETWEEN 1 AND 512),
    CHECK (length(body_template) <= 4096),
    CHECK (signing_secret IS NULL OR length(signing_secret) <= 1024),
    CHECK (cooldown_secs >= 0 AND cooldown_secs <= 2592000),
    CHECK (notes IS NULL OR length(notes) <= 1024)
);

CREATE INDEX fleet_alert_notification_deliveries_lease_idx
    ON fleet_alert_notification_deliveries (
        status,
        delivery_lease_until,
        next_attempt_at ASC,
        created_at ASC
    );

CREATE INDEX webhook_rules_enabled_idx
    ON webhook_rules (enabled, updated_at DESC, name);

CREATE TABLE webhook_events (
    id UUID NOT NULL,
    kind TEXT NOT NULL,
    event_id TEXT NOT NULL,
    event_predicates TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    subject_client_ids TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    payload JSONB NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at TIMESTAMPTZ,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (occurred_at, id),
    CHECK (length(trim(kind)) BETWEEN 1 AND 128),
    CHECK (length(trim(event_id)) BETWEEN 1 AND 256),
    CHECK (jsonb_typeof(payload) = 'object')
) PARTITION BY RANGE (occurred_at);

CREATE TABLE webhook_events_default
    PARTITION OF webhook_events DEFAULT;

CREATE INDEX webhook_events_unprocessed_idx
    ON webhook_events (processed_at, occurred_at ASC);

CREATE INDEX webhook_events_processed_telemetry_retention_idx
    ON webhook_events (occurred_at ASC, id ASC)
    WHERE kind = 'telemetry.rollup' AND processed_at IS NOT NULL;

CREATE INDEX webhook_events_kind_idx
    ON webhook_events (kind, event_id, occurred_at DESC);

CREATE TABLE webhook_rule_deliveries (
    id UUID PRIMARY KEY,
    rule_id UUID NOT NULL,
    rule_name TEXT NOT NULL,
    event_kind TEXT NOT NULL,
    event_id TEXT NOT NULL,
    status TEXT NOT NULL,
    target TEXT NOT NULL,
    dedupe_key TEXT NOT NULL,
    payload JSONB NOT NULL,
    matched_vps JSONB NOT NULL,
    message TEXT NOT NULL,
    error TEXT,
    cooldown_until_unix BIGINT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    delivery_lease_id UUID,
    delivery_lease_until TIMESTAMPTZ,
    next_attempt_at TIMESTAMPTZ,
    last_attempt_at TIMESTAMPTZ,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at TIMESTAMPTZ,
    CHECK (status IN ('queued', 'in_progress', 'failed', 'permanently_failed', 'canceled_disabled', 'delivered', 'matched_dry_run')),
    CHECK (length(trim(event_kind)) BETWEEN 1 AND 128),
    CHECK (length(trim(event_id)) BETWEEN 1 AND 256),
    CHECK (length(trim(target)) BETWEEN 1 AND 512),
    CHECK (jsonb_typeof(payload) = 'object'),
    CHECK (jsonb_typeof(matched_vps) = 'array'),
    CHECK (cooldown_until_unix >= 0)
);

CREATE INDEX webhook_rule_deliveries_status_idx
    ON webhook_rule_deliveries (status, created_at DESC);

CREATE INDEX webhook_rule_deliveries_rule_idx
    ON webhook_rule_deliveries (rule_id, created_at DESC);

CREATE INDEX webhook_rule_deliveries_event_idx
    ON webhook_rule_deliveries (event_kind, event_id, created_at DESC);

CREATE INDEX webhook_rule_deliveries_created_idx
    ON webhook_rule_deliveries (created_at DESC, id DESC);

CREATE INDEX webhook_rule_deliveries_event_kind_created_idx
    ON webhook_rule_deliveries (event_kind, created_at DESC, id DESC);

CREATE UNIQUE INDEX webhook_rule_deliveries_rule_event_unique_idx
    ON webhook_rule_deliveries (rule_id, event_id);

CREATE INDEX webhook_rule_deliveries_dedupe_idx
    ON webhook_rule_deliveries (dedupe_key, cooldown_until_unix DESC);

CREATE INDEX webhook_rule_deliveries_attempt_idx
    ON webhook_rule_deliveries (status, next_attempt_at ASC, created_at ASC);

CREATE INDEX webhook_rule_deliveries_lease_idx
    ON webhook_rule_deliveries (
        status,
        delivery_lease_until,
        next_attempt_at ASC,
        created_at ASC
    );

CREATE TABLE webhook_rule_cursors (
    rule_id UUID NOT NULL REFERENCES webhook_rules(id) ON DELETE CASCADE,
    event_key TEXT NOT NULL,
    last_event_id TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (rule_id, event_key),
    CHECK (length(trim(event_key)) BETWEEN 1 AND 128),
    CHECK (length(trim(last_event_id)) BETWEEN 1 AND 256)
);

CREATE TABLE history_retention_policies (
    domain TEXT PRIMARY KEY,
    retention_days INTEGER NOT NULL,
    prune_limit INTEGER NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    metadata_only BOOLEAN NOT NULL DEFAULT FALSE,
    export_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    notes TEXT,
    updated_by UUID REFERENCES operators(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (domain IN (
        'audit_logs',
        'system_metric_rollups',
        'telemetry_samples',
        'telemetry_rollups',
        'telemetry_network_rates',
        'telemetry_ping_rollups',
        'traffic_counter_samples',
        'job_outputs',
        'backup_artifacts',
        'network_observations',
        'topology_history',
        'client_status_history',
        'gateway_sessions'
    )),
    CONSTRAINT history_retention_policies_traffic_counter_min_days_check
        CHECK (domain <> 'traffic_counter_samples' OR retention_days >= 32),
    CHECK (retention_days BETWEEN 1 AND 3650),
    CHECK (prune_limit BETWEEN 1 AND 100000),
    CHECK (notes IS NULL OR length(notes) <= 1000)
);

CREATE INDEX history_retention_policies_updated_idx
    ON history_retention_policies (updated_at DESC, domain);
