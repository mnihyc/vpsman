ALTER TABLE schedules
    ADD COLUMN catch_up_policy TEXT NOT NULL DEFAULT 'skip_missed',
    ADD COLUMN catch_up_limit INTEGER NOT NULL DEFAULT 1,
    ADD COLUMN retry_delay_secs BIGINT NOT NULL DEFAULT 300,
    ADD COLUMN max_failures INTEGER NOT NULL DEFAULT 3,
    ADD COLUMN failure_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN last_error TEXT;

ALTER TABLE schedules
    ADD CONSTRAINT schedules_catch_up_policy_check
        CHECK (catch_up_policy IN ('skip_missed', 'run_once', 'run_all_limited')),
    ADD CONSTRAINT schedules_catch_up_limit_check
        CHECK (catch_up_limit BETWEEN 1 AND 25),
    ADD CONSTRAINT schedules_retry_delay_secs_check
        CHECK (retry_delay_secs BETWEEN 1 AND 86400),
    ADD CONSTRAINT schedules_max_failures_check
        CHECK (max_failures BETWEEN 1 AND 100),
    ADD CONSTRAINT schedules_failure_count_check
        CHECK (failure_count >= 0);

CREATE INDEX schedules_policy_due_idx
    ON schedules (enabled, next_run_at, catch_up_policy);

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
