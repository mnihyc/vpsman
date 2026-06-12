CREATE TABLE schedules (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    name TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    operation JSONB NOT NULL,
    selector_expression TEXT NOT NULL,
    target_client_ids TEXT[] NOT NULL,
    cron_expr TEXT NOT NULL DEFAULT '0 * * * *',
    timezone TEXT NOT NULL DEFAULT 'UTC',
    next_run_at TIMESTAMPTZ NOT NULL,
    last_run_at TIMESTAMPTZ,
    deferred_until TIMESTAMPTZ,
    catch_up_policy TEXT NOT NULL DEFAULT 'skip_missed',
    catch_up_limit INTEGER NOT NULL DEFAULT 1,
    retry_delay_secs BIGINT NOT NULL DEFAULT 300,
    max_failures INTEGER NOT NULL DEFAULT 3,
    failure_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    deleted_at TIMESTAMPTZ,
    deleted_by UUID REFERENCES operators(id) ON DELETE SET NULL,
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
        CHECK (failure_count >= 0),
    CONSTRAINT schedules_timezone_utc CHECK (timezone = 'UTC'),
    CONSTRAINT schedules_cron_expr_not_empty CHECK (length(trim(cron_expr)) > 0),
    CONSTRAINT schedules_target_client_ids_nonempty CHECK (cardinality(target_client_ids) BETWEEN 1 AND 500)
);

CREATE INDEX schedules_due_idx
    ON schedules (enabled, next_run_at, deferred_until)
    WHERE deleted_at IS NULL;

CREATE INDEX schedules_policy_due_idx
    ON schedules (enabled, next_run_at, catch_up_policy)
    WHERE deleted_at IS NULL;

CREATE INDEX schedules_visible_name_idx
    ON schedules (name, id)
    WHERE deleted_at IS NULL;

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
    request_fingerprint TEXT NOT NULL,
    timeout_secs BIGINT NOT NULL DEFAULT 30,
    reconnect_policy JSONB NOT NULL DEFAULT '{"duplicate_delivery":"ignore_completed","resume_outputs":true}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX jobs_scheduled_source_idx
    ON jobs (status, source_schedule_id)
    WHERE source_schedule_id IS NOT NULL;

ALTER TABLE jobs
  ADD CONSTRAINT jobs_status_common_check CHECK (status IN (
    'queued',
    'running',
    'dispatching',
    'completed',
    'partially_completed',
    'failed',
    'timed_out',
    'dispatch_failed',
    'degraded_unprivileged',
    'accepted',
    'rejected_authorization_required',
    'schedule_no_targets'
  ));

CREATE TABLE job_targets (
    job_id UUID NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    message TEXT,
    exit_code INTEGER,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    dispatch_attempts INTEGER NOT NULL DEFAULT 0,
    dispatch_lease_until TIMESTAMPTZ,
    last_dispatch_error TEXT,
    PRIMARY KEY (job_id, client_id)
);

CREATE INDEX job_targets_dispatch_due_idx
    ON job_targets (status, dispatch_lease_until, job_id, client_id)
    WHERE completed_at IS NULL
      AND status IN ('queued', 'dispatching');

ALTER TABLE job_targets
  ADD CONSTRAINT job_targets_status_common_check CHECK (status IN (
    'queued',
    'dispatching',
    'accepted',
    'completed',
    'failed',
    'timed_out',
    'dispatch_failed',
    'degraded_unprivileged',
    'rejected_by_agent',
    'rejected_authorization_required'
  ));

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

CREATE TABLE server_artifacts (
    id UUID PRIMARY KEY,
    domain TEXT NOT NULL,
    object_key TEXT NOT NULL UNIQUE,
    sha256_hex TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    job_id UUID,
    client_id TEXT,
    stream TEXT,
    seq INTEGER,
    backup_request_id UUID,
    backup_artifact_id UUID,
    release_id UUID,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    tombstoned_at TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ,
    CONSTRAINT server_artifacts_status_check CHECK (status IN ('active', 'tombstoned', 'deleted')),
    CONSTRAINT server_artifacts_metadata_object CHECK (jsonb_typeof(metadata) = 'object')
);

CREATE INDEX server_artifacts_domain_status_idx
    ON server_artifacts (domain, status, created_at DESC);

CREATE INDEX server_artifacts_job_idx
    ON server_artifacts (job_id, client_id, seq)
    WHERE job_id IS NOT NULL;

CREATE TABLE server_jobs (
    id UUID PRIMARY KEY,
    job_type TEXT NOT NULL,
    status TEXT NOT NULL,
    expression TEXT,
    preview_hash TEXT,
    matched_count BIGINT NOT NULL DEFAULT 0,
    matched_bytes BIGINT NOT NULL DEFAULT 0,
    deleted_count BIGINT NOT NULL DEFAULT 0,
    deleted_bytes BIGINT NOT NULL DEFAULT 0,
    error TEXT,
    created_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    canceled_at TIMESTAMPTZ,
    CONSTRAINT server_jobs_type_check CHECK (job_type IN ('artifact_cleanup')),
    CONSTRAINT server_jobs_status_check CHECK (status IN ('queued', 'running', 'completed', 'failed', 'canceled')),
    CONSTRAINT server_jobs_metadata_object CHECK (jsonb_typeof(metadata) = 'object')
);

CREATE INDEX server_jobs_status_created_idx
    ON server_jobs (status, created_at ASC);

CREATE TABLE terminal_sessions (
    session_id UUID NOT NULL,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    state TEXT NOT NULL,
    last_status TEXT NOT NULL,
    argv JSONB NOT NULL DEFAULT '[]'::jsonb,
    cwd TEXT,
    cols BIGINT,
    rows BIGINT,
    idle_timeout_secs BIGINT,
    flow_window_bytes BIGINT,
    output_first_seq BIGINT,
    output_next_seq BIGINT,
    output_retained_first_seq BIGINT,
    output_retained_bytes BIGINT,
    output_dropped_bytes BIGINT,
    output_dropped_chunks BIGINT,
    output_replay_truncated BOOLEAN NOT NULL DEFAULT FALSE,
    last_input_seq BIGINT,
    session_exited BOOLEAN NOT NULL DEFAULT FALSE,
    close_reason TEXT,
    last_event TEXT NOT NULL,
    last_job_id UUID NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    last_command_type TEXT NOT NULL,
    last_seq INTEGER NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, session_id),
    CONSTRAINT terminal_sessions_argv_array CHECK (jsonb_typeof(argv) = 'array')
);

CREATE INDEX terminal_sessions_observed_idx
    ON terminal_sessions (observed_at DESC, client_id, session_id);

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
