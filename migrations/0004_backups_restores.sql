CREATE TABLE backup_artifacts (
    id UUID PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    object_key TEXT NOT NULL,
    sha256_hex TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX backup_artifacts_object_key_unique
    ON backup_artifacts (object_key);

CREATE TABLE backup_requests (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    paths TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    include_config BOOLEAN NOT NULL DEFAULT FALSE,
    follow_symlinks BOOLEAN NOT NULL DEFAULT FALSE,
    status TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    command_scope TEXT NOT NULL,
    artifact_id UUID REFERENCES backup_artifacts(id),
    source_job_id UUID REFERENCES jobs(id),
    source_schedule_id UUID REFERENCES schedules(id),
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT backup_requests_status_check
        CHECK (status IN (
            'requested_metadata_only',
            'artifact_metadata_recorded',
            'execution_failed',
            'execution_canceled'
        ))
);

CREATE INDEX backup_requests_client_created_idx
    ON backup_requests (client_id, created_at DESC, id DESC);

CREATE INDEX backup_requests_status_created_idx
    ON backup_requests (status, created_at DESC, id DESC);

CREATE INDEX backup_requests_source_schedule_created_idx
    ON backup_requests (source_schedule_id, client_id, created_at DESC, id DESC)
    WHERE source_schedule_id IS NOT NULL;

CREATE TABLE restore_plans (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    source_backup_request_id UUID NOT NULL REFERENCES backup_requests(id) ON DELETE CASCADE,
    source_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    target_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    paths TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    include_config BOOLEAN NOT NULL DEFAULT FALSE,
    destination_root TEXT,
    status TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    command_scope TEXT NOT NULL,
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT restore_plans_status_check
        CHECK (status IN ('planned_metadata_only'))
);

CREATE INDEX restore_plans_source_created_idx
    ON restore_plans (source_backup_request_id, created_at DESC, id DESC);

CREATE INDEX restore_plans_target_created_idx
    ON restore_plans (target_client_id, created_at DESC, id DESC);

CREATE INDEX restore_plans_status_created_idx
    ON restore_plans (status, created_at DESC, id DESC);

CREATE TABLE migration_links (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    restore_plan_id UUID NOT NULL UNIQUE REFERENCES restore_plans(id) ON DELETE CASCADE,
    source_backup_request_id UUID NOT NULL REFERENCES backup_requests(id) ON DELETE CASCADE,
    source_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    target_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    paths TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    include_config BOOLEAN NOT NULL DEFAULT FALSE,
    destination_root TEXT,
    status TEXT NOT NULL,
    note TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT migration_links_status_check
        CHECK (status IN ('linked_metadata_only'))
);

CREATE INDEX migration_links_source_created_idx
    ON migration_links (source_client_id, created_at DESC, id DESC);

CREATE INDEX migration_links_target_created_idx
    ON migration_links (target_client_id, created_at DESC, id DESC);

CREATE INDEX migration_links_status_created_idx
    ON migration_links (status, created_at DESC, id DESC);

CREATE TABLE backup_policies (
    schedule_id UUID PRIMARY KEY REFERENCES schedules(id) ON DELETE CASCADE,
    retention_days INTEGER NOT NULL DEFAULT 30 CHECK (retention_days BETWEEN 1 AND 3650),
    keep_last INTEGER NOT NULL DEFAULT 7 CHECK (keep_last BETWEEN 1 AND 1000),
    rotation_generation TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
