CREATE TABLE history_retention_policies (
    domain TEXT PRIMARY KEY,
    retention_days INTEGER NOT NULL,
    prune_limit INTEGER NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    metadata_only BOOLEAN NOT NULL DEFAULT FALSE,
    export_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    notes TEXT,
    updated_by UUID REFERENCES operators(id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (domain IN (
        'audit_logs',
        'telemetry_samples',
        'telemetry_rollups',
        'job_outputs',
        'backup_artifacts',
        'network_observations',
        'topology_history'
    )),
    CHECK (retention_days BETWEEN 1 AND 3650),
    CHECK (prune_limit BETWEEN 1 AND 100000),
    CHECK (notes IS NULL OR length(notes) <= 1000)
);

CREATE INDEX history_retention_policies_updated_idx
    ON history_retention_policies (updated_at DESC, domain);
