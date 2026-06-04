CREATE TABLE operators (
    id UUID PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    totp_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    role TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE resource_pools (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    provider TEXT,
    region TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE clients (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    public_key BYTEA NOT NULL,
    pool_id UUID REFERENCES resource_pools(id),
    status TEXT NOT NULL DEFAULT 'unknown',
    agent_version TEXT,
    os_release TEXT,
    arch TEXT,
    last_seen_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE tags (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE client_tags (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    tag_id UUID NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (client_id, tag_id)
);

CREATE TABLE jobs (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    command_type TEXT NOT NULL,
    privileged BOOLEAN NOT NULL DEFAULT FALSE,
    status TEXT NOT NULL,
    target_count INTEGER NOT NULL DEFAULT 0,
    payload_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);

CREATE TABLE job_targets (
    job_id UUID NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    exit_code INTEGER,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    PRIMARY KEY (job_id, client_id)
);

CREATE TABLE job_outputs (
    job_id UUID NOT NULL,
    client_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    stream TEXT NOT NULL,
    data BYTEA NOT NULL,
    exit_code INTEGER,
    done BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (job_id, client_id, seq),
    FOREIGN KEY (job_id, client_id) REFERENCES job_targets(job_id, client_id) ON DELETE CASCADE
);

CREATE TABLE telemetry_samples (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    observed_at TIMESTAMPTZ NOT NULL,
    cpu_load_1 DOUBLE PRECISION NOT NULL,
    memory_total_bytes BIGINT NOT NULL,
    memory_available_bytes BIGINT NOT NULL,
    payload JSONB NOT NULL,
    PRIMARY KEY (client_id, observed_at)
);

CREATE TABLE audit_logs (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    action TEXT NOT NULL,
    target TEXT NOT NULL,
    command_hash TEXT,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE backup_artifacts (
    id UUID PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    object_key TEXT NOT NULL,
    sha256_hex TEXT NOT NULL,
    encrypted BOOLEAN NOT NULL DEFAULT TRUE,
    size_bytes BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

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
