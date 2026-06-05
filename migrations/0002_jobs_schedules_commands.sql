CREATE TABLE schedules (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    name TEXT NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    operation JSONB NOT NULL,
    target_clients TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    target_tags TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    interval_secs BIGINT NOT NULL CHECK (interval_secs BETWEEN 1 AND 31536000),
    next_run_at TIMESTAMPTZ NOT NULL,
    last_run_at TIMESTAMPTZ,
    catch_up_policy TEXT NOT NULL DEFAULT 'skip_missed',
    catch_up_limit INTEGER NOT NULL DEFAULT 1,
    retry_delay_secs BIGINT NOT NULL DEFAULT 300,
    max_failures INTEGER NOT NULL DEFAULT 3,
    failure_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT schedules_catch_up_policy_check
        CHECK (catch_up_policy IN ('skip_missed', 'run_once', 'run_all_limited')),
    CONSTRAINT schedules_catch_up_limit_check
        CHECK (catch_up_limit BETWEEN 1 AND 25),
    CONSTRAINT schedules_retry_delay_secs_check
        CHECK (retry_delay_secs BETWEEN 1 AND 86400),
    CONSTRAINT schedules_max_failures_check
        CHECK (max_failures BETWEEN 1 AND 100),
    CONSTRAINT schedules_failure_count_check
        CHECK (failure_count >= 0)
);

CREATE INDEX schedules_due_idx
    ON schedules (enabled, next_run_at);

CREATE INDEX schedules_policy_due_idx
    ON schedules (enabled, next_run_at, catch_up_policy);

CREATE TABLE jobs (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    command_type TEXT NOT NULL,
    privileged BOOLEAN NOT NULL DEFAULT FALSE,
    status TEXT NOT NULL,
    target_count INTEGER NOT NULL DEFAULT 0,
    payload_hash TEXT NOT NULL,
    operation JSONB,
    source_schedule_id UUID REFERENCES schedules(id),
    idempotency_key TEXT,
    reconnect_policy JSONB NOT NULL DEFAULT '{"duplicate_delivery":"ignore_completed","resume_outputs":true,"cancel_on_disconnect":false}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX jobs_scheduled_approval_idx
    ON jobs (status, source_schedule_id)
    WHERE source_schedule_id IS NOT NULL;

CREATE UNIQUE INDEX jobs_actor_idempotency_key_idx
    ON jobs (actor_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

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
    storage TEXT NOT NULL DEFAULT 'inline',
    object_key TEXT,
    data_sha256_hex TEXT,
    data_size_bytes BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (job_id, client_id, seq),
    FOREIGN KEY (job_id, client_id) REFERENCES job_targets(job_id, client_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX job_outputs_object_key_unique
    ON job_outputs (object_key)
    WHERE object_key IS NOT NULL;

CREATE TABLE worker_leases (
    task_name TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    lease_expires_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (length(task_name) BETWEEN 1 AND 120),
    CHECK (length(owner) BETWEEN 1 AND 200)
);

CREATE INDEX worker_leases_expires_idx
    ON worker_leases (lease_expires_at);

CREATE TABLE command_templates (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    scope_kind TEXT NOT NULL,
    scope_value TEXT,
    command_type TEXT NOT NULL,
    operation JSONB NOT NULL,
    defaults JSONB NOT NULL DEFAULT '{}'::jsonb,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (scope_kind IN ('global', 'provider', 'tag', 'client')),
    CHECK (
        (scope_kind = 'global' AND scope_value IS NULL)
        OR (scope_kind <> 'global' AND scope_value IS NOT NULL)
    ),
    CHECK (jsonb_typeof(operation) = 'object'),
    CHECK (jsonb_typeof(defaults) = 'object')
);

CREATE UNIQUE INDEX command_templates_global_name_idx
    ON command_templates (name)
    WHERE scope_kind = 'global';

CREATE UNIQUE INDEX command_templates_scoped_name_idx
    ON command_templates (scope_kind, scope_value, name)
    WHERE scope_kind <> 'global';

CREATE INDEX command_templates_lookup_idx
    ON command_templates (scope_kind, scope_value, command_type, updated_at DESC);
