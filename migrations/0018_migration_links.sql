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
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX migration_links_source_created_idx
    ON migration_links (source_client_id, created_at DESC, id DESC);

CREATE INDEX migration_links_target_created_idx
    ON migration_links (target_client_id, created_at DESC, id DESC);

CREATE INDEX migration_links_status_created_idx
    ON migration_links (status, created_at DESC, id DESC);
