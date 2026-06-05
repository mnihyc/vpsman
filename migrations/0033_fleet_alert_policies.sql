CREATE TABLE fleet_alert_policies (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    scope_kind TEXT NOT NULL,
    scope_value TEXT,
    memory_available_warning_ratio DOUBLE PRECISION,
    memory_available_critical_ratio DOUBLE PRECISION,
    disk_available_warning_ratio DOUBLE PRECISION,
    disk_available_critical_ratio DOUBLE PRECISION,
    cpu_load_warning DOUBLE PRECISION,
    cpu_load_critical DOUBLE PRECISION,
    priority INTEGER NOT NULL DEFAULT 0,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    notes TEXT,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (scope_kind IN ('global', 'provider', 'tag', 'client')),
    CHECK (
        (scope_kind = 'global' AND scope_value IS NULL)
        OR (scope_kind <> 'global' AND scope_value IS NOT NULL)
    ),
    CHECK (
        memory_available_warning_ratio IS NULL
        OR (memory_available_warning_ratio > 0 AND memory_available_warning_ratio < 1)
    ),
    CHECK (
        memory_available_critical_ratio IS NULL
        OR (memory_available_critical_ratio > 0 AND memory_available_critical_ratio < 1)
    ),
    CHECK (
        disk_available_warning_ratio IS NULL
        OR (disk_available_warning_ratio > 0 AND disk_available_warning_ratio < 1)
    ),
    CHECK (
        disk_available_critical_ratio IS NULL
        OR (disk_available_critical_ratio > 0 AND disk_available_critical_ratio < 1)
    ),
    CHECK (cpu_load_warning IS NULL OR cpu_load_warning > 0),
    CHECK (cpu_load_critical IS NULL OR cpu_load_critical > 0),
    CHECK (
        memory_available_warning_ratio IS NOT NULL
        OR memory_available_critical_ratio IS NOT NULL
        OR disk_available_warning_ratio IS NOT NULL
        OR disk_available_critical_ratio IS NOT NULL
        OR cpu_load_warning IS NOT NULL
        OR cpu_load_critical IS NOT NULL
    )
);

CREATE INDEX fleet_alert_policies_match_idx
    ON fleet_alert_policies (
        enabled,
        scope_kind,
        scope_value,
        priority DESC,
        updated_at DESC
    );
