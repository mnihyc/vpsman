CREATE TABLE telemetry_rollups (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    bucket_start TIMESTAMPTZ NOT NULL,
    bucket_secs INTEGER NOT NULL,
    sample_count INTEGER NOT NULL,
    cpu_load_1_avg DOUBLE PRECISION NOT NULL,
    cpu_load_1_max DOUBLE PRECISION NOT NULL,
    memory_total_bytes_max BIGINT NOT NULL,
    memory_available_bytes_avg BIGINT NOT NULL,
    memory_available_bytes_min BIGINT NOT NULL,
    disk_total_bytes_max BIGINT NOT NULL DEFAULT 0,
    disk_available_bytes_avg BIGINT NOT NULL DEFAULT 0,
    disk_available_bytes_min BIGINT NOT NULL DEFAULT 0,
    network_rx_bytes_max BIGINT NOT NULL DEFAULT 0,
    network_tx_bytes_max BIGINT NOT NULL DEFAULT 0,
    latest_observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, bucket_secs, bucket_start)
);

CREATE INDEX telemetry_rollups_latest_idx
    ON telemetry_rollups (bucket_secs, bucket_start DESC, client_id);

CREATE TABLE telemetry_network_rates (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    interface TEXT NOT NULL,
    bucket_start TIMESTAMPTZ NOT NULL,
    bucket_secs INTEGER NOT NULL,
    sample_count INTEGER NOT NULL,
    rx_bytes_avg BIGINT NOT NULL,
    tx_bytes_avg BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, interface, bucket_secs, bucket_start)
);

CREATE INDEX telemetry_network_rates_latest_idx
    ON telemetry_network_rates (bucket_secs, bucket_start DESC, client_id, interface);

CREATE TABLE telemetry_tunnels (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    observed_at TIMESTAMPTZ NOT NULL,
    interface TEXT NOT NULL,
    kind TEXT NOT NULL,
    ownership_mode TEXT NOT NULL,
    mutation_policy TEXT NOT NULL,
    promotion_required BOOLEAN NOT NULL,
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
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, interface)
);

CREATE INDEX telemetry_tunnels_latest_idx
    ON telemetry_tunnels (observed_at DESC, client_id, interface);

CREATE TABLE fleet_alert_policies (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    scope_kind TEXT NOT NULL,
    scope_value TEXT,
    memory_available_warning_ratio DOUBLE PRECISION,
    memory_available_critical_ratio DOUBLE PRECISION,
    disk_available_warning_ratio DOUBLE PRECISION,
    disk_available_critical_ratio DOUBLE PRECISION,
    cpu_load_warning DOUBLE PRECISION,
    cpu_load_critical DOUBLE PRECISION,
    priority INTEGER NOT NULL DEFAULT 0,
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
    CHECK (
        memory_available_warning_ratio IS NULL
        OR (memory_available_warning_ratio > 0 AND memory_available_warning_ratio < 1)
    ),
    CHECK (
        memory_available_critical_ratio IS NULL
        OR (memory_available_critical_ratio > 0 AND memory_available_critical_ratio < 1)
    ),
    CHECK (
        disk_available_warning_ratio IS NULL
        OR (disk_available_warning_ratio > 0 AND disk_available_warning_ratio < 1)
    ),
    CHECK (
        disk_available_critical_ratio IS NULL
        OR (disk_available_critical_ratio > 0 AND disk_available_critical_ratio < 1)
    ),
    CHECK (cpu_load_warning IS NULL OR cpu_load_warning > 0),
    CHECK (cpu_load_critical IS NULL OR cpu_load_critical > 0),
    CHECK (
        memory_available_warning_ratio IS NOT NULL
        OR memory_available_critical_ratio IS NOT NULL
        OR disk_available_warning_ratio IS NOT NULL
        OR disk_available_critical_ratio IS NOT NULL
        OR cpu_load_warning IS NOT NULL
        OR cpu_load_critical IS NOT NULL
    )
);

CREATE INDEX fleet_alert_policies_match_idx
    ON fleet_alert_policies (
        enabled,
        scope_kind,
        scope_value,
        priority DESC,
        updated_at DESC
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
    channel_id UUID NOT NULL REFERENCES fleet_alert_notification_channels(id) ON DELETE CASCADE,
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
    last_attempt_at TIMESTAMPTZ,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at TIMESTAMPTZ,
    CHECK (alert_severity IN ('info', 'warning', 'critical')),
    CHECK (cooldown_until_unix >= 0)
);

CREATE INDEX fleet_alert_notification_deliveries_status_idx
    ON fleet_alert_notification_deliveries (status, created_at DESC);

CREATE INDEX fleet_alert_notification_deliveries_dedupe_idx
    ON fleet_alert_notification_deliveries (dedupe_key, cooldown_until_unix DESC);

CREATE INDEX fleet_alert_notification_deliveries_alert_idx
    ON fleet_alert_notification_deliveries (alert_id, created_at DESC);

CREATE INDEX fleet_alert_notification_deliveries_attempt_idx
    ON fleet_alert_notification_deliveries (
        status,
        delivery_kind,
        attempt_count,
        created_at ASC
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
        'telemetry_rollups',
        'job_outputs',
        'backup_artifacts',
        'network_observations',
        'topology_history'
    )),
    CHECK (retention_days BETWEEN 1 AND 3650),
    CHECK (prune_limit BETWEEN 1 AND 100000),
    CHECK (notes IS NULL OR length(notes) <= 1000)
);

CREATE INDEX history_retention_policies_updated_idx
    ON history_retention_policies (updated_at DESC, domain);
