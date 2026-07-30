CREATE TABLE configuration_presets (
    id UUID PRIMARY KEY,
    behavior TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    description TEXT,
    definition JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT configuration_presets_behavior_check
        CHECK (behavior IN (
            'host_metrics',
            'tunnel_traffic',
            'latency_probe',
            'ospf_update_command',
            'process_inventory',
            'user_sessions',
            'command_execution'
        )),
    CONSTRAINT configuration_presets_kind_check
        CHECK (kind IN ('system', 'custom')),
    CONSTRAINT configuration_presets_name_check
        CHECK (
            length(name) > 0
            AND name = btrim(name)
            AND octet_length(name) <= 256
            AND name !~ '[[:cntrl:]]'
        ),
    CONSTRAINT configuration_presets_description_check
        CHECK (description IS NULL OR octet_length(description) <= 4096),
    CONSTRAINT configuration_presets_definition_object_check
        CHECK (jsonb_typeof(definition) = 'object'),
    CONSTRAINT configuration_presets_default_kind_check
        CHECK (NOT is_default OR kind = 'system'),
    UNIQUE (id, behavior)
);

CREATE UNIQUE INDEX configuration_presets_default_idx
    ON configuration_presets (behavior)
    WHERE is_default;

CREATE UNIQUE INDEX configuration_presets_name_idx
    ON configuration_presets (behavior, lower(name));

CREATE TABLE client_configuration_preset_overrides (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    behavior TEXT NOT NULL,
    preset_id UUID NOT NULL,
    updated_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, behavior),
    FOREIGN KEY (preset_id, behavior)
        REFERENCES configuration_presets(id, behavior)
        ON DELETE RESTRICT
);

CREATE INDEX client_configuration_preset_overrides_preset_idx
    ON client_configuration_preset_overrides (preset_id);

CREATE TABLE network_adapter_definitions (
    id UUID PRIMARY KEY,
    adapter_kind TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    definition JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT network_adapter_definitions_kind_check
        CHECK (adapter_kind IN ('runtime_tunnel', 'routing_cost')),
    CONSTRAINT network_adapter_definitions_name_check
        CHECK (
            length(name) > 0
            AND name = btrim(name)
            AND octet_length(name) <= 256
            AND name !~ '[[:cntrl:]]'
        ),
    CONSTRAINT network_adapter_definitions_description_check
        CHECK (description IS NULL OR octet_length(description) <= 4096),
    CONSTRAINT network_adapter_definitions_definition_object_check
        CHECK (jsonb_typeof(definition) = 'object')
);

CREATE UNIQUE INDEX network_adapter_definitions_name_idx
    ON network_adapter_definitions (adapter_kind, lower(name));

CREATE TABLE client_runtime_config_overrides (
    client_id TEXT PRIMARY KEY REFERENCES clients(id) ON DELETE CASCADE,
    toml TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    updated_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT client_runtime_config_overrides_toml_check
        CHECK (octet_length(toml) > 0 AND octet_length(toml) <= 4194304),
    CONSTRAINT client_runtime_config_overrides_reason_check
        CHECK (octet_length(reason) <= 4096)
);

CREATE TABLE client_runtime_config_apply_state (
    client_id TEXT PRIMARY KEY REFERENCES clients(id) ON DELETE CASCADE,
    applied_version BIGINT,
    applied_content_hash TEXT,
    applied_config JSONB,
    applied_job_id UUID REFERENCES jobs(id) ON DELETE SET NULL,
    applied_at TIMESTAMPTZ,
    pending_version BIGINT,
    pending_content_hash TEXT,
    pending_config JSONB,
    pending_job_id UUID REFERENCES jobs(id) ON DELETE SET NULL,
    pending_reason TEXT,
    pending_status TEXT,
    pending_error TEXT,
    pending_updated_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT client_runtime_config_apply_state_pending_status_check
        CHECK (pending_status IS NULL OR pending_status IN ('queued', 'failed')),
    CONSTRAINT client_runtime_config_apply_state_applied_config_check
        CHECK (applied_config IS NULL OR jsonb_typeof(applied_config) = 'object'),
    CONSTRAINT client_runtime_config_apply_state_pending_config_check
        CHECK (pending_config IS NULL OR jsonb_typeof(pending_config) = 'object'),
    CONSTRAINT client_runtime_config_apply_state_hash_check
        CHECK (
            (applied_content_hash IS NULL OR octet_length(applied_content_hash) <= 128)
            AND (pending_content_hash IS NULL OR octet_length(pending_content_hash) <= 128)
        ),
    CONSTRAINT client_runtime_config_apply_state_reason_check
        CHECK (pending_reason IS NULL OR octet_length(pending_reason) <= 4096),
    CONSTRAINT client_runtime_config_apply_state_error_check
        CHECK (pending_error IS NULL OR octet_length(pending_error) <= 4096)
);

CREATE INDEX client_runtime_config_apply_state_pending_job_idx
    ON client_runtime_config_apply_state (pending_job_id)
    WHERE pending_job_id IS NOT NULL;

CREATE INDEX client_runtime_config_apply_state_applied_hash_idx
    ON client_runtime_config_apply_state (client_id, applied_content_hash)
    WHERE applied_content_hash IS NOT NULL;

CREATE TABLE file_transfer_sessions (
    session_id UUID NOT NULL,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    direction TEXT NOT NULL,
    status TEXT NOT NULL,
    path TEXT NOT NULL,
    size_bytes BIGINT,
    progress_bytes BIGINT NOT NULL DEFAULT 0,
    progress_ratio DOUBLE PRECISION,
    sha256_hex TEXT,
    chunk_size_bytes BIGINT,
    last_chunk_size_bytes BIGINT,
    last_chunk_sha256_hex TEXT,
    rate_limit_kbps BIGINT,
    resumed BOOLEAN,
    last_event TEXT NOT NULL,
    last_job_id UUID NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    last_command_type TEXT NOT NULL,
    last_seq INTEGER NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    handoff_available BOOLEAN NOT NULL DEFAULT FALSE,
    handoff_object_key TEXT,
    handoff_download_path TEXT,
    PRIMARY KEY (client_id, session_id),
    CONSTRAINT file_transfer_sessions_direction_check
        CHECK (direction IN ('upload', 'download')),
    CONSTRAINT file_transfer_sessions_status_check
        CHECK (status IN ('started', 'transferring', 'completed', 'aborted', 'unknown')),
    CONSTRAINT file_transfer_sessions_last_event_check
        CHECK (last_event IN (
            'file_transfer_start',
            'file_transfer_chunk_ack',
            'file_transfer_commit',
            'file_transfer_abort',
            'file_transfer_download_start',
            'file_transfer_download_chunk'
        )),
    CONSTRAINT file_transfer_sessions_last_command_type_check
        CHECK (last_command_type IN (
            'file_transfer_start',
            'file_transfer_chunk',
            'file_transfer_commit',
            'file_transfer_abort',
            'file_transfer_download_start',
            'file_transfer_download_chunk'
        ))
);

CREATE INDEX file_transfer_sessions_observed_idx
    ON file_transfer_sessions (observed_at DESC, client_id, session_id);

CREATE TABLE runtime_config_patch_generators (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    category TEXT NOT NULL,
    domain TEXT NOT NULL,
    description TEXT NOT NULL,
    field_schema JSONB NOT NULL DEFAULT '{}'::jsonb,
    raw_generator_body TEXT NOT NULL,
    docs_metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    built_in BOOLEAN NOT NULL DEFAULT FALSE,
    actor_id UUID REFERENCES operators(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT runtime_config_patch_generators_name_check CHECK (length(trim(name)) > 0 AND octet_length(name) <= 4096),
    CONSTRAINT runtime_config_patch_generators_category_check CHECK (length(trim(category)) > 0 AND octet_length(category) <= 4096),
    CONSTRAINT runtime_config_patch_generators_domain_check CHECK (length(trim(domain)) > 0 AND octet_length(domain) <= 4096),
    CONSTRAINT runtime_config_patch_generators_description_check CHECK (length(trim(description)) > 0 AND octet_length(description) <= 4096),
    CONSTRAINT runtime_config_patch_generators_body_check CHECK (length(trim(raw_generator_body)) > 0 AND octet_length(raw_generator_body) <= 16384),
    CONSTRAINT runtime_config_patch_generators_schema_object CHECK (jsonb_typeof(field_schema) = 'object'),
    CONSTRAINT runtime_config_patch_generators_docs_object CHECK (jsonb_typeof(docs_metadata) = 'object')
);

INSERT INTO runtime_config_patch_generators (
    id, name, category, domain, description, field_schema, raw_generator_body, docs_metadata, built_in
)
VALUES
    (
        '55555555-5555-4555-8555-555555555555',
        'Autonomous updater enabled',
        'update',
        'agent_update',
        'Enable agent autonomous self-update from an external version manifest.',
        '{"fields":{"unmanaged_version_url":{"type":"string","default":"https://github.com/mnihyc/vpsman/releases/latest/download/version.json"},"unmanaged_interval_secs":{"type":"integer","minimum":300,"maximum":604800,"default":86400},"unmanaged_jitter_secs":{"type":"integer","minimum":0,"maximum":604800,"default":86400},"unmanaged_activate":{"type":"boolean","default":true},"unmanaged_restart_agent":{"type":"boolean","default":true}}}'::jsonb,
        $$[update]
unmanaged_enabled = true
unmanaged_version_url = {{unmanaged_version_url}}
unmanaged_interval_secs = {{unmanaged_interval_secs}}
unmanaged_jitter_secs = {{unmanaged_jitter_secs}}
unmanaged_activate = {{unmanaged_activate}}
unmanaged_restart_agent = {{unmanaged_restart_agent}}
$$,
        '{"expandable":true,"affected_sections":["update"],"patch_only":true,"predefined":true}'::jsonb,
        TRUE
    ),
    (
        '66666666-6666-4666-8666-666666666666',
        'Autonomous updater disabled',
        'update',
        'agent_update',
        'Disable agent autonomous self-update while keeping manifest URL and interval values explicit in agent config.',
        '{"fields":{"unmanaged_version_url":{"type":"string","default":"https://github.com/mnihyc/vpsman/releases/latest/download/version.json"},"unmanaged_interval_secs":{"type":"integer","minimum":300,"maximum":604800,"default":86400},"unmanaged_jitter_secs":{"type":"integer","minimum":0,"maximum":604800,"default":86400},"unmanaged_activate":{"type":"boolean","default":true},"unmanaged_restart_agent":{"type":"boolean","default":true}}}'::jsonb,
        $$[update]
unmanaged_enabled = false
unmanaged_version_url = {{unmanaged_version_url}}
unmanaged_interval_secs = {{unmanaged_interval_secs}}
unmanaged_jitter_secs = {{unmanaged_jitter_secs}}
unmanaged_activate = {{unmanaged_activate}}
unmanaged_restart_agent = {{unmanaged_restart_agent}}
$$,
        '{"expandable":true,"affected_sections":["update"],"patch_only":true,"predefined":true}'::jsonb,
        TRUE
    )
ON CONFLICT (id) DO NOTHING;

CREATE TABLE file_transfer_source_artifacts (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    object_key TEXT NOT NULL,
    sha256_hex TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    created_by UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT file_transfer_source_artifacts_sha256_hex_check
        CHECK (sha256_hex ~ '^[0-9a-f]{64}$'),
    CONSTRAINT file_transfer_source_artifacts_size_check
        CHECK (size_bytes >= 0)
);

CREATE INDEX file_transfer_source_artifacts_created_idx
    ON file_transfer_source_artifacts (created_at DESC, id DESC);

CREATE INDEX file_transfer_source_artifacts_hash_idx
    ON file_transfer_source_artifacts (sha256_hex, size_bytes);

CREATE UNIQUE INDEX file_transfer_source_artifacts_object_key_unique
    ON file_transfer_source_artifacts (object_key);
