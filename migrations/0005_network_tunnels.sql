CREATE TABLE tunnels (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    left_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    right_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    left_address INET NOT NULL,
    right_address INET NOT NULL,
    bandwidth_tier TEXT NOT NULL,
    desired_ospf_cost INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE tunnel_plans (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    left_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    right_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    input JSONB NOT NULL,
    plan JSONB NOT NULL,
    recommended_ospf_cost INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'planned',
    left_status TEXT NOT NULL DEFAULT 'planned',
    right_status TEXT NOT NULL DEFAULT 'planned',
    last_apply_job_id UUID REFERENCES jobs(id),
    last_rollback_job_id UUID REFERENCES jobs(id),
    deleted_at TIMESTAMPTZ,
    deleted_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    deleted_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT tunnel_plans_status_check
        CHECK (status IN ('planned', 'applied', 'partially_applied', 'rolled_back', 'partially_rolled_back')),
    CONSTRAINT tunnel_plans_left_status_check
        CHECK (left_status IN ('planned', 'applied', 'rolled_back')),
    CONSTRAINT tunnel_plans_right_status_check
        CHECK (right_status IN ('planned', 'applied', 'rolled_back'))
);

CREATE UNIQUE INDEX tunnel_plans_active_name_idx
    ON tunnel_plans (name)
    WHERE deleted_at IS NULL;

CREATE INDEX tunnel_plans_clients_idx
    ON tunnel_plans (left_client_id, right_client_id);

CREATE INDEX tunnel_plans_status_idx
    ON tunnel_plans (status, updated_at DESC);

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
    plan_name TEXT,
    interface_name TEXT,
    peer_client_id TEXT,
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
