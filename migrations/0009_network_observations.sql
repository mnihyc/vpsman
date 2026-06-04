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
    metadata JSONB NOT NULL DEFAULT '{}',
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (job_id, client_id, seq),
    FOREIGN KEY (job_id, client_id) REFERENCES job_targets(job_id, client_id) ON DELETE CASCADE
);

CREATE INDEX network_observations_kind_observed_idx
    ON network_observations (kind, observed_at DESC, id DESC);

CREATE INDEX network_observations_plan_observed_idx
    ON network_observations (plan_name, observed_at DESC, id DESC);
