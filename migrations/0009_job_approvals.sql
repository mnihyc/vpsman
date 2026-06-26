CREATE TABLE job_approvals (
    id UUID PRIMARY KEY,
    status TEXT NOT NULL DEFAULT 'pending',
    job_id UUID NOT NULL,
    command_type TEXT NOT NULL,
    selector_expression TEXT NOT NULL,
    target_client_ids TEXT[] NOT NULL,
    target_count INTEGER NOT NULL DEFAULT 0,
    privileged BOOLEAN NOT NULL DEFAULT TRUE,
    destructive BOOLEAN NOT NULL DEFAULT FALSE,
    force_unprivileged BOOLEAN NOT NULL DEFAULT FALSE,
    max_timeout_secs BIGINT NOT NULL DEFAULT 30,
    payload_hash TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    requester_id UUID REFERENCES operators(id) ON DELETE SET NULL,
    requester_username TEXT NOT NULL,
    requester_role TEXT NOT NULL,
    request_reason TEXT,
    risk TEXT NOT NULL DEFAULT 'standard',
    job_request JSONB NOT NULL,
    decision_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    decision_username TEXT,
    decision_reason TEXT,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (status IN ('pending', 'approved', 'rejected')),
    CHECK (target_count >= 0),
    CHECK (max_timeout_secs > 0),
    CHECK (length(trim(command_type)) > 0),
    CHECK (length(trim(requester_username)) > 0),
    CHECK (length(trim(requester_role)) > 0),
    CHECK (length(trim(risk)) BETWEEN 1 AND 64),
    CHECK (
        (status = 'pending' AND decided_at IS NULL)
        OR (status <> 'pending' AND decided_at IS NOT NULL)
    )
);

CREATE INDEX job_approvals_status_requested_idx
    ON job_approvals (status, requested_at DESC, id DESC);

CREATE INDEX job_approvals_job_idx
    ON job_approvals (job_id);

CREATE INDEX job_approvals_requester_idx
    ON job_approvals (requester_username, requested_at DESC);
