CREATE TABLE tunnel_plans (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision >= 1),
    left_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    right_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    input JSONB NOT NULL,
    plan JSONB NOT NULL,
    builtin_credentials JSONB,
    recommended_ospf_cost INTEGER,
    ospf_status TEXT NOT NULL DEFAULT 'disabled',
    left_ospf_status TEXT NOT NULL DEFAULT 'disabled',
    right_ospf_status TEXT NOT NULL DEFAULT 'disabled',
    desired_ospf_cost INTEGER,
    left_current_ospf_cost INTEGER,
    right_current_ospf_cost INTEGER,
    left_ospf_job_id UUID,
    right_ospf_job_id UUID,
    connection_assessment TEXT NOT NULL DEFAULT 'automatic',
    connection_assessment_note TEXT,
    connection_assessed_at TIMESTAMPTZ,
    connection_assessed_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    automatic_ospf_scanned_at TIMESTAMPTZ,
    pending_ospf_reconciled_at TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ,
    deleted_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    deleted_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT tunnel_plans_ospf_status_check
        CHECK (ospf_status IN ('disabled', 'unverified', 'pending', 'verified', 'partial', 'failed', 'stale')),
    CONSTRAINT tunnel_plans_left_ospf_status_check
        CHECK (left_ospf_status IN ('disabled', 'unverified', 'pending', 'verified', 'failed', 'stale')),
    CONSTRAINT tunnel_plans_right_ospf_status_check
        CHECK (right_ospf_status IN ('disabled', 'unverified', 'pending', 'verified', 'failed', 'stale')),
    CONSTRAINT tunnel_plans_desired_ospf_cost_check
        CHECK (desired_ospf_cost IS NULL OR desired_ospf_cost BETWEEN 1 AND 65535),
    CONSTRAINT tunnel_plans_left_ospf_cost_check
        CHECK (left_current_ospf_cost IS NULL OR left_current_ospf_cost BETWEEN 1 AND 65535),
    CONSTRAINT tunnel_plans_right_ospf_cost_check
        CHECK (right_current_ospf_cost IS NULL OR right_current_ospf_cost BETWEEN 1 AND 65535),
    CONSTRAINT tunnel_plans_connection_assessment_check
        CHECK (connection_assessment IN ('automatic', 'connected', 'disconnected')),
    CONSTRAINT tunnel_plans_connection_assessment_note_check
        CHECK (
            (connection_assessment = 'automatic'
                AND connection_assessment_note IS NULL
                AND connection_assessed_at IS NULL
                AND connection_assessed_by IS NULL)
            OR
            (connection_assessment IN ('connected', 'disconnected')
                AND connection_assessment_note IS NOT NULL
                AND length(btrim(connection_assessment_note)) BETWEEN 1 AND 500
                AND connection_assessed_at IS NOT NULL
                AND connection_assessed_by IS NOT NULL)
        )
);

CREATE UNIQUE INDEX tunnel_plans_active_name_idx
    ON tunnel_plans (name)
    WHERE deleted_at IS NULL;

CREATE INDEX tunnel_plans_clients_idx
    ON tunnel_plans (left_client_id, right_client_id);

CREATE INDEX tunnel_plans_ospf_status_idx
    ON tunnel_plans (ospf_status, updated_at DESC);

CREATE INDEX tunnel_plans_active_clients_idx
    ON tunnel_plans (left_client_id, right_client_id, updated_at DESC)
    WHERE deleted_at IS NULL;

CREATE INDEX tunnel_plans_automatic_controller_scan_idx
    ON tunnel_plans (automatic_ospf_scanned_at ASC NULLS FIRST, id)
    WHERE deleted_at IS NULL
      AND enabled = TRUE
      AND plan->'ospf'->>'mode' = 'automatic';

CREATE INDEX tunnel_plans_pending_controller_scan_idx
    ON tunnel_plans (pending_ospf_reconciled_at ASC NULLS FIRST, id)
    WHERE deleted_at IS NULL
      AND ospf_status = 'pending';

-- Automatic reachability probes repeat the topology identity on every sample.
-- Keep that stable identity once so retained history can use compact integer
-- keys without changing the exact-evidence contract used by recent readers.
CREATE TABLE network_observation_series (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    plan_id UUID NOT NULL REFERENCES tunnel_plans(id) ON DELETE CASCADE,
    topology_identity_hash TEXT NOT NULL,
    plan_name TEXT NOT NULL,
    interface_name TEXT NOT NULL,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    peer_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    endpoint_side TEXT NOT NULL,
    address_family TEXT NOT NULL,
    target TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT network_observation_series_endpoint_side_check
        CHECK (endpoint_side IN ('left', 'right')),
    CONSTRAINT network_observation_series_address_family_check
        CHECK (address_family IN ('ipv4', 'ipv6')),
    UNIQUE (
        plan_id,
        topology_identity_hash,
        client_id,
        peer_client_id,
        endpoint_side,
        address_family,
        interface_name,
        target
    )
);

CREATE INDEX network_observation_series_plan_identity_idx
    ON network_observation_series (plan_id, topology_identity_hash, endpoint_side);

CREATE INDEX network_observation_series_inactive_idx
    ON network_observation_series (last_seen_at, id)
    WHERE active = FALSE;

CREATE TABLE network_observations (
    id UUID PRIMARY KEY,
    job_id UUID REFERENCES jobs(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    seq INTEGER,
    kind TEXT NOT NULL,
    role TEXT,
    plan_id UUID NOT NULL REFERENCES tunnel_plans(id) ON DELETE RESTRICT,
    topology_identity_hash TEXT NOT NULL,
    plan_name TEXT NOT NULL,
    interface_name TEXT NOT NULL,
    peer_client_id TEXT NOT NULL,
    target TEXT,
    endpoint_side TEXT,
    address_family TEXT,
    stale_after_secs BIGINT,
    healthy BOOLEAN,
    transmitted INTEGER,
    received INTEGER,
    latency_min_ms DOUBLE PRECISION,
    latency_avg_ms DOUBLE PRECISION,
    latency_max_ms DOUBLE PRECISION,
    latency_mdev_ms DOUBLE PRECISION,
    packet_loss_ratio DOUBLE PRECISION,
    reason TEXT,
    throughput_mbps DOUBLE PRECISION,
    bytes BIGINT,
    source TEXT NOT NULL DEFAULT 'manual',
    automatic_series_id BIGINT REFERENCES network_observation_series(id) ON DELETE SET NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT network_observations_source_check CHECK (source IN ('automatic', 'manual')),
    CONSTRAINT network_observations_automatic_series_check CHECK (
        automatic_series_id IS NULL
        OR (source = 'automatic' AND kind = 'tunnel_reachability')
    ),
    CONSTRAINT network_observations_endpoint_side_check
        CHECK (endpoint_side IS NULL OR endpoint_side IN ('left', 'right')),
    CONSTRAINT network_observations_address_family_check
        CHECK (address_family IS NULL OR address_family IN ('ipv4', 'ipv6')),
    CONSTRAINT network_observations_stale_after_check
        CHECK (stale_after_secs IS NULL OR stale_after_secs >= 1),
    CONSTRAINT network_observations_packet_counts_check
        CHECK (
            transmitted IS NULL OR received IS NULL
            OR (transmitted >= 0 AND received >= 0 AND received <= transmitted)
        ),
    FOREIGN KEY (job_id, client_id) REFERENCES job_targets(job_id, client_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX network_observations_job_sequence_unique
    ON network_observations (job_id, client_id, seq)
    WHERE job_id IS NOT NULL AND seq IS NOT NULL;

CREATE INDEX network_observations_kind_observed_idx
    ON network_observations (kind, observed_at DESC, id DESC);

CREATE INDEX network_observations_plan_observed_idx
    ON network_observations (plan_name, observed_at DESC, id DESC);

CREATE INDEX network_observations_plan_identity_observed_idx
    ON network_observations (plan_id, topology_identity_hash, observed_at DESC, id DESC);

CREATE INDEX network_observations_client_observed_idx
    ON network_observations (client_id, observed_at DESC, id DESC);

CREATE INDEX network_observations_peer_client_observed_idx
    ON network_observations (peer_client_id, observed_at DESC, id DESC);

CREATE INDEX network_observations_plan_kind_observed_idx
    ON network_observations (plan_id, kind, endpoint_side, observed_at DESC, id DESC)
    WHERE kind IN ('tunnel_reachability', 'network_speed_test');

CREATE INDEX network_observations_range_filter_idx
    ON network_observations (observed_at DESC, plan_id, source, kind, client_id);

CREATE INDEX network_observations_plan_identity_kind_observed_idx
    ON network_observations (
        plan_id,
        topology_identity_hash,
        kind,
        observed_at DESC,
        id DESC
    );

CREATE INDEX network_observations_status_endpoint_observed_idx
    ON network_observations (
        plan_id,
        topology_identity_hash,
        client_id,
        observed_at DESC,
        id DESC
    )
    WHERE kind = 'network_status';

CREATE INDEX network_observations_automatic_series_observed_idx
    ON network_observations (automatic_series_id, observed_at, id)
    WHERE automatic_series_id IS NOT NULL;

-- The latest automatic observation is copied, not referenced, so compacting
-- exact rows can never remove the last exact endpoint state. Static topology
-- fields stay in network_observation_series and are not repeated here.
CREATE TABLE network_observation_latest (
    series_id BIGINT PRIMARY KEY REFERENCES network_observation_series(id) ON DELETE CASCADE,
    observation_id UUID NOT NULL UNIQUE,
    stale_after_secs BIGINT NOT NULL CHECK (stale_after_secs >= 1),
    healthy BOOLEAN NOT NULL,
    transmitted INTEGER NOT NULL CHECK (transmitted >= 0),
    received INTEGER NOT NULL CHECK (received >= 0 AND received <= transmitted),
    latency_min_ms DOUBLE PRECISION,
    latency_avg_ms DOUBLE PRECISION,
    latency_max_ms DOUBLE PRECISION,
    latency_mdev_ms DOUBLE PRECISION,
    packet_loss_ratio DOUBLE PRECISION NOT NULL,
    reason TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    observed_at TIMESTAMPTZ NOT NULL,
    received_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT network_observation_latest_packet_loss_check
        CHECK (packet_loss_ratio >= 0.0 AND packet_loss_ratio <= 1.0)
);

CREATE INDEX network_observation_latest_observed_idx
    ON network_observation_latest (observed_at DESC, observation_id DESC);

-- A row represents one health/reason component of a retained time bucket.
-- Splitting these dimensions preserves health and free-text reason filters
-- without retaining individual automatic observations.
CREATE TABLE network_observation_rollups (
    series_id BIGINT NOT NULL REFERENCES network_observation_series(id) ON DELETE CASCADE,
    bucket_secs INTEGER NOT NULL CHECK (bucket_secs IN (300, 1800, 3600, 10800, 21600, 86400)),
    bucket_start TIMESTAMPTZ NOT NULL,
    health_state SMALLINT NOT NULL CHECK (health_state IN (-1, 0, 1)),
    reason_key TEXT NOT NULL DEFAULT '',
    sample_count BIGINT NOT NULL CHECK (sample_count > 0),
    transmitted_total NUMERIC(38, 0) NOT NULL CHECK (transmitted_total >= 0),
    transmitted_sample_count BIGINT NOT NULL CHECK (transmitted_sample_count >= 0),
    received_total NUMERIC(38, 0) NOT NULL CHECK (received_total >= 0),
    received_sample_count BIGINT NOT NULL CHECK (received_sample_count >= 0),
    latency_sum_ms DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    latency_sample_count BIGINT NOT NULL CHECK (latency_sample_count >= 0),
    latency_min_ms DOUBLE PRECISION,
    latency_max_ms DOUBLE PRECISION,
    latency_mdev_sum_ms DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    latency_mdev_sample_count BIGINT NOT NULL CHECK (latency_mdev_sample_count >= 0),
    packet_loss_sum_ratio DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    packet_loss_sample_count BIGINT NOT NULL CHECK (packet_loss_sample_count >= 0),
    packet_loss_min_ratio DOUBLE PRECISION,
    packet_loss_max_ratio DOUBLE PRECISION,
    latest_observation_id UUID NOT NULL,
    latest_stale_after_secs BIGINT NOT NULL CHECK (latest_stale_after_secs >= 1),
    latest_healthy BOOLEAN NOT NULL,
    latest_transmitted INTEGER NOT NULL CHECK (latest_transmitted >= 0),
    latest_received INTEGER NOT NULL CHECK (
        latest_received >= 0 AND latest_received <= latest_transmitted
    ),
    latest_latency_min_ms DOUBLE PRECISION,
    latest_latency_avg_ms DOUBLE PRECISION,
    latest_latency_max_ms DOUBLE PRECISION,
    latest_latency_mdev_ms DOUBLE PRECISION,
    latest_packet_loss_ratio DOUBLE PRECISION NOT NULL,
    latest_reason TEXT,
    latest_observed_at TIMESTAMPTZ NOT NULL,
    latest_received_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (
        series_id,
        bucket_secs,
        bucket_start,
        health_state,
        reason_key
    ),
    CONSTRAINT network_observation_rollups_bucket_alignment_check CHECK (
        extract(epoch FROM bucket_start)::bigint % bucket_secs = 0
    ),
    CONSTRAINT network_observation_rollups_latency_count_check CHECK (
        (latency_sample_count = 0 AND latency_min_ms IS NULL AND latency_max_ms IS NULL)
        OR (latency_sample_count > 0 AND latency_min_ms IS NOT NULL AND latency_max_ms IS NOT NULL)
    ),
    CONSTRAINT network_observation_rollups_packet_loss_count_check CHECK (
        (packet_loss_sample_count = 0
            AND packet_loss_min_ratio IS NULL
            AND packet_loss_max_ratio IS NULL)
        OR (packet_loss_sample_count > 0
            AND packet_loss_min_ratio IS NOT NULL
            AND packet_loss_max_ratio IS NOT NULL)
    ),
    CONSTRAINT network_observation_rollups_latest_packet_loss_check CHECK (
        latest_packet_loss_ratio >= 0.0 AND latest_packet_loss_ratio <= 1.0
    )
);

CREATE INDEX network_observation_rollups_retention_idx
    ON network_observation_rollups (bucket_secs, bucket_start, series_id);

-- Retention starts from one oldest native bucket per compact series. This
-- order keeps discovery bounded without scanning every historical bucket.
CREATE INDEX network_observation_rollups_series_time_idx
    ON network_observation_rollups (
        series_id,
        bucket_start,
        bucket_secs,
        health_state,
        reason_key
    );

-- Exact-evidence readers include the permanent latest snapshot when its raw
-- row has already been compacted. A snapshot is never exposed twice.
CREATE VIEW network_observation_exact_evidence AS
SELECT
    observation.id,
    observation.job_id,
    observation.client_id,
    observation.seq,
    observation.kind,
    observation.source,
    observation.role,
    observation.plan_id,
    observation.topology_identity_hash,
    observation.plan_name,
    observation.interface_name,
    observation.peer_client_id,
    observation.target,
    observation.endpoint_side,
    observation.address_family,
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
    observation.throughput_mbps,
    observation.bytes,
    observation.metadata,
    observation.observed_at,
    observation.received_at
FROM network_observations observation
UNION ALL
SELECT
    latest.observation_id AS id,
    NULL::uuid AS job_id,
    series.client_id,
    NULL::integer AS seq,
    'tunnel_reachability'::text AS kind,
    'automatic'::text AS source,
    'endpoint'::text AS role,
    series.plan_id,
    series.topology_identity_hash,
    series.plan_name,
    series.interface_name,
    series.peer_client_id,
    series.target,
    series.endpoint_side,
    series.address_family,
    latest.stale_after_secs,
    latest.healthy,
    latest.transmitted,
    latest.received,
    latest.latency_min_ms,
    latest.latency_avg_ms,
    latest.latency_max_ms,
    latest.latency_mdev_ms,
    latest.packet_loss_ratio,
    latest.reason,
    NULL::double precision AS throughput_mbps,
    NULL::bigint AS bytes,
    latest.metadata,
    latest.observed_at,
    latest.received_at
FROM network_observation_latest latest
JOIN network_observation_series series ON series.id = latest.series_id
WHERE series.active = TRUE
  AND NOT EXISTS (
    SELECT 1
    FROM network_observations observation
    WHERE observation.id = latest.observation_id
);

CREATE TABLE port_forward_rules (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id) ON DELETE SET NULL,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    protocol TEXT NOT NULL,
    target_ip INET NOT NULL,
    target_hostname TEXT,
    mappings JSONB NOT NULL,
    masquerade BOOLEAN NOT NULL DEFAULT TRUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision >= 1),
    deleted_at TIMESTAMPTZ,
    deleted_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    deleted_reason TEXT,
    removal_confirmed_at TIMESTAMPTZ,
    forgotten_at TIMESTAMPTZ,
    forgotten_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    forget_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT port_forward_rules_name_check
        CHECK (length(btrim(name)) BETWEEN 1 AND 128),
    CONSTRAINT port_forward_rules_protocol_check
        CHECK (protocol IN ('tcp', 'udp', 'both')),
    CONSTRAINT port_forward_rules_target_hostname_check
        CHECK (
            target_hostname IS NULL
            OR (
                length(target_hostname) BETWEEN 1 AND 253
                AND target_hostname = lower(target_hostname)
                AND target_hostname = btrim(target_hostname)
                AND target_hostname ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)*$'
                AND target_hostname !~ '^((25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])\.){3}(25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])$'
            )
        ),
    CONSTRAINT port_forward_rules_mappings_array_check
        CHECK (jsonb_typeof(mappings) = 'array')
);

CREATE UNIQUE INDEX port_forward_rules_active_name_idx
    ON port_forward_rules (client_id, name)
    WHERE deleted_at IS NULL;

CREATE INDEX port_forward_rules_client_state_idx
    ON port_forward_rules (client_id, enabled, updated_at DESC)
    WHERE forgotten_at IS NULL;

CREATE INDEX port_forward_rules_removal_pending_idx
    ON port_forward_rules (client_id, deleted_at)
    WHERE deleted_at IS NOT NULL
      AND removal_confirmed_at IS NULL
      AND forgotten_at IS NULL;

CREATE TABLE port_forward_runtime_state (
    client_id TEXT PRIMARY KEY REFERENCES clients(id) ON DELETE CASCADE,
    snapshot JSONB NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
