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
    recommended_ospf_cost INTEGER,
    ospf_status TEXT NOT NULL DEFAULT 'disabled',
    left_ospf_status TEXT NOT NULL DEFAULT 'disabled',
    right_ospf_status TEXT NOT NULL DEFAULT 'disabled',
    desired_ospf_cost INTEGER,
    left_current_ospf_cost INTEGER,
    right_current_ospf_cost INTEGER,
    left_ospf_job_id UUID,
    right_ospf_job_id UUID,
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
        CHECK (right_current_ospf_cost IS NULL OR right_current_ospf_cost BETWEEN 1 AND 65535)
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

CREATE TABLE network_observations (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    role TEXT,
    plan_id UUID NOT NULL REFERENCES tunnel_plans(id) ON DELETE RESTRICT,
    topology_identity_hash TEXT NOT NULL,
    plan_name TEXT NOT NULL,
    interface_name TEXT NOT NULL,
    peer_client_id TEXT NOT NULL,
    target TEXT,
    healthy BOOLEAN,
    latency_avg_ms DOUBLE PRECISION,
    packet_loss_ratio DOUBLE PRECISION,
    throughput_mbps DOUBLE PRECISION,
    bytes BIGINT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (job_id, client_id, seq),
    FOREIGN KEY (job_id, client_id) REFERENCES job_targets(job_id, client_id) ON DELETE CASCADE
);

CREATE INDEX network_observations_kind_observed_idx
    ON network_observations (kind, observed_at DESC, id DESC);

CREATE INDEX network_observations_plan_observed_idx
    ON network_observations (plan_name, observed_at DESC, id DESC);

CREATE INDEX network_observations_plan_identity_observed_idx
    ON network_observations (plan_id, topology_identity_hash, observed_at DESC, id DESC);
