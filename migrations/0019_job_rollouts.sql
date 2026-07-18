CREATE TABLE job_rollouts (
    job_id UUID PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'running',
    canary_client_ids TEXT[] NOT NULL,
    batch_size INTEGER NOT NULL,
    max_failures INTEGER NOT NULL,
    pause_after_canary BOOLEAN NOT NULL DEFAULT TRUE,
    batch_delay_secs BIGINT NOT NULL DEFAULT 0,
    current_batch INTEGER NOT NULL DEFAULT 0,
    total_batches INTEGER NOT NULL,
    failure_baseline INTEGER NOT NULL DEFAULT 0,
    pause_reason TEXT,
    next_batch_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    CONSTRAINT job_rollouts_status_check
        CHECK (status IN ('running', 'paused', 'completed', 'aborted')),
    CONSTRAINT job_rollouts_canary_nonempty
        CHECK (cardinality(canary_client_ids) BETWEEN 1 AND 25),
    CONSTRAINT job_rollouts_batch_size_check
        CHECK (batch_size BETWEEN 1 AND 100),
    CONSTRAINT job_rollouts_max_failures_check
        CHECK (max_failures BETWEEN 0 AND 100),
    CONSTRAINT job_rollouts_batch_delay_check
        CHECK (batch_delay_secs BETWEEN 0 AND 86400),
    CONSTRAINT job_rollouts_batch_index_check
        CHECK (current_batch >= 0 AND total_batches >= 1 AND current_batch < total_batches),
    CONSTRAINT job_rollouts_failure_baseline_check
        CHECK (failure_baseline >= 0),
    CONSTRAINT job_rollouts_terminal_shape_check
        CHECK (
            (status IN ('completed', 'aborted') AND completed_at IS NOT NULL)
            OR (status IN ('running', 'paused') AND completed_at IS NULL)
        )
);

CREATE TABLE job_rollout_targets (
    job_id UUID NOT NULL,
    client_id TEXT NOT NULL,
    batch_index INTEGER NOT NULL,
    PRIMARY KEY (job_id, client_id),
    FOREIGN KEY (job_id, client_id)
        REFERENCES job_targets(job_id, client_id)
        ON DELETE CASCADE,
    CONSTRAINT job_rollout_targets_batch_index_check CHECK (batch_index >= 0)
);

CREATE INDEX job_rollouts_active_idx
    ON job_rollouts (status, next_batch_at, updated_at, job_id)
    WHERE completed_at IS NULL;

CREATE INDEX job_rollout_targets_batch_idx
    ON job_rollout_targets (job_id, batch_index, client_id);
