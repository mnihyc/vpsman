CREATE TABLE source_templates (
    id UUID PRIMARY KEY,
    domain TEXT NOT NULL,
    name TEXT NOT NULL,
    scope TEXT NOT NULL,
    built_in BOOLEAN NOT NULL DEFAULT FALSE,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    owner_client_id TEXT REFERENCES clients(id) ON DELETE CASCADE,
    description TEXT,
    definition JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT source_templates_scope_check
        CHECK (scope IN ('built_in', 'shared', 'vps_local')),
    CONSTRAINT source_templates_definition_object_check
        CHECK (jsonb_typeof(definition) = 'object'),
    CONSTRAINT source_templates_owner_scope_check
        CHECK (
            (scope = 'vps_local' AND owner_client_id IS NOT NULL)
            OR (scope <> 'vps_local' AND owner_client_id IS NULL)
        )
);

CREATE UNIQUE INDEX source_templates_global_name_idx
    ON source_templates (domain, name, scope)
    WHERE owner_client_id IS NULL;

CREATE UNIQUE INDEX source_templates_client_name_idx
    ON source_templates (domain, owner_client_id, name)
    WHERE owner_client_id IS NOT NULL;

CREATE UNIQUE INDEX source_templates_default_idx
    ON source_templates (domain)
    WHERE is_default;

CREATE TABLE client_source_template_assignments (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    domain TEXT NOT NULL,
    template_id UUID NOT NULL REFERENCES source_templates(id) ON DELETE RESTRICT,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, domain)
);

CREATE INDEX client_source_template_assignments_template_idx
    ON client_source_template_assignments (template_id);

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

INSERT INTO source_templates (
    id, domain, name, scope, built_in, is_default, description, definition
) VALUES
    (
        '00000000-0000-4000-8000-000000000001',
        'telemetry_metrics_source',
        'builtin:linux_procfs',
        'built_in',
        TRUE,
        TRUE,
        'Default low-cost Linux procfs/sysfs telemetry source',
        '{"source":"linux_procfs"}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000002',
        'runtime_traffic_accounting_source',
        'builtin:interface_counters',
        'built_in',
        TRUE,
        TRUE,
        'Default runtime tunnel traffic accounting from interface counters',
        '{"source":"interface_counters"}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000003',
        'latency_probe_source',
        'builtin:linux_ping',
        'built_in',
        TRUE,
        TRUE,
        'Default ICMP latency/loss probe using documented Linux ping candidates',
        '{"source":"linux_ping_preset"}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000004',
        'speed_test_provider',
        'builtin:tcp_throughput',
        'built_in',
        TRUE,
        TRUE,
        'Default bounded two-endpoint TCP throughput provider',
        '{"provider":"tcp_throughput"}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000005',
        'process_inventory_source',
        'builtin:linux_procfs',
        'built_in',
        TRUE,
        TRUE,
        'Default process inventory from configurable Linux procfs root',
        '{"source":"linux_procfs"}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000006',
        'user_session_inventory_source',
        'builtin:linux_w_who',
        'built_in',
        TRUE,
        TRUE,
        'Default user/session inventory using Linux w/who candidates',
        '{"source":"linux_w_who_preset"}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000007',
        'command_execution_policy',
        'builtin:linux_shell_argv',
        'built_in',
        TRUE,
        TRUE,
        'Default Linux shell-script argv prefix policy',
        '{"shell_script_argv":["/bin/sh","-lc"]}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000008',
        'runtime_tunnel_adapter',
        'builtin:agent_iproute2_managed',
        'built_in',
        TRUE,
        TRUE,
        'Default client-managed iproute2/tc runtime tunnel adapter',
        '{"manager":"agent_iproute2_managed"}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000009',
        'backup_object_store',
        'builtin:local_filesystem',
        'built_in',
        TRUE,
        TRUE,
        'Default local filesystem object-store adapter with reserved S3 extension',
        '{"provider":"local_filesystem"}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-00000000000a',
        'update_artifact_source',
        'builtin:external_https_sha256',
        'built_in',
        TRUE,
        TRUE,
        'Default external HTTPS update artifact source with SHA-256 verification',
        '{"provider":"external_https","requires_sha256":true}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000011',
        'telemetry_metrics_source',
        'builtin:host_mounted_procfs',
        'built_in',
        TRUE,
        FALSE,
        'Container or chroot telemetry source reading host-mounted proc/sys trees',
        '{"source":"linux_procfs","proc_root":"/host/proc","sys_class_net_dir":"/host/sys/class/net","hostname_file":"/host/etc/hostname","os_release_file":"/host/etc/os-release"}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000021',
        'runtime_traffic_accounting_source',
        'builtin:vnstat_json',
        'built_in',
        TRUE,
        FALSE,
        'Common vnstat JSON traffic accounting source for provider images with vnstat installed',
        '{"source":"vnstat","vnstat_argv":["/usr/bin/vnstat"]}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000031',
        'latency_probe_source',
        'builtin:usr_bin_ping',
        'built_in',
        TRUE,
        FALSE,
        'Pinned /usr/bin/ping latency/loss probe for hosts where path discovery is undesirable',
        '{"source":"configured_ping_argv","probe_ping_argv":["/usr/bin/ping"]}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000041',
        'speed_test_provider',
        'builtin:iperf3_json_adapter',
        'built_in',
        TRUE,
        FALSE,
        'Reserved iperf3 JSON provider adapter template for fleets that standardize on iperf3',
        '{"provider":"iperf3_json_adapter","server_argv":["/usr/bin/iperf3","--server","--one-off","--json"],"client_argv":["/usr/bin/iperf3","--client","{server_address}","--json"]}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000051',
        'process_inventory_source',
        'builtin:host_mounted_procfs',
        'built_in',
        TRUE,
        FALSE,
        'Process inventory from a host-mounted /proc tree',
        '{"source":"linux_procfs","proc_root":"/host/proc"}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000061',
        'user_session_inventory_source',
        'builtin:usr_bin_w',
        'built_in',
        TRUE,
        FALSE,
        'Pinned /usr/bin/w session inventory source',
        '{"source":"linux_w_who_preset","user_sessions_command":{"argv":["/usr/bin/w","-h"],"max_timeout_secs":5,"max_output_bytes":16384}}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000062',
        'user_session_inventory_source',
        'builtin:usr_bin_who',
        'built_in',
        TRUE,
        FALSE,
        'Pinned /usr/bin/who session inventory source',
        '{"source":"linux_w_who_preset","user_sessions_command":{"argv":["/usr/bin/who"],"max_timeout_secs":5,"max_output_bytes":16384}}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000071',
        'command_execution_policy',
        'builtin:busybox_ash_argv',
        'built_in',
        TRUE,
        FALSE,
        'BusyBox ash shell-script argv prefix for minimal images',
        '{"shell_script_argv":["/bin/ash","-lc"]}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000081',
        'runtime_tunnel_adapter',
        'builtin:agent_iproute2_runtime_reconcile',
        'built_in',
        TRUE,
        FALSE,
        'Client-managed iproute2/tc runtime tunnel adapter with runtime reconciliation enabled',
        '{"manager":"agent_iproute2_managed","runtime_reconcile_enabled":true,"runtime_ip_argv":["/sbin/ip"],"runtime_tc_argv":["/sbin/tc"]}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000091',
        'backup_object_store',
        'builtin:s3_path_style_reserved',
        'built_in',
        TRUE,
        FALSE,
        'Reserved S3/MinIO path-style artifact adapter template',
        '{"provider":"s3_path_style","requires_server_env":["VPSMAN_OBJECT_ENDPOINT","VPSMAN_OBJECT_BUCKET","VPSMAN_OBJECT_ACCESS_KEY","VPSMAN_OBJECT_SECRET_KEY"]}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-0000000000a1',
        'update_artifact_source',
        'builtin:github_release_sha256',
        'built_in',
        TRUE,
        FALSE,
        'GitHub Releases update artifact source using version.json and SHA256SUMS',
        '{"provider":"github_release","requires_sha256":true,"manifest":"version_json_sha256sums"}'::jsonb
    )
ON CONFLICT (id) DO NOTHING;

CREATE TABLE hot_config_patch_generators (
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
    CONSTRAINT hot_config_patch_generators_name_not_empty CHECK (length(trim(name)) > 0),
    CONSTRAINT hot_config_patch_generators_category_not_empty CHECK (length(trim(category)) > 0),
    CONSTRAINT hot_config_patch_generators_domain_not_empty CHECK (length(trim(domain)) > 0),
    CONSTRAINT hot_config_patch_generators_schema_object CHECK (jsonb_typeof(field_schema) = 'object'),
    CONSTRAINT hot_config_patch_generators_docs_object CHECK (jsonb_typeof(docs_metadata) = 'object')
);

CREATE INDEX hot_config_patch_generators_category_idx
    ON hot_config_patch_generators (category, name);

INSERT INTO hot_config_patch_generators (
    id, name, category, domain, description, field_schema, raw_generator_body, docs_metadata, built_in
)
VALUES
    (
        '11111111-1111-4111-8111-111111111111',
        'Telemetry source',
        'telemetry',
        'metrics',
        'Switch telemetry collection source and optional Linux paths.',
        '{"fields":{"source":{"type":"string","enum":["linux_procfs","custom_command","linux_procfs_and_custom_command"],"default":"linux_procfs"},"proc_root":{"type":"string","default":"/proc"},"sys_class_net_dir":{"type":"string","default":"/sys/class/net"}}}'::jsonb,
        $$[telemetry]
source = {{source}}
proc_root = {{proc_root}}
sys_class_net_dir = {{sys_class_net_dir}}
$$,
        '{"expandable":true,"affected_sections":["telemetry"],"patch_only":true,"predefined":true}'::jsonb,
        TRUE
    ),
    (
        '22222222-2222-4222-8222-222222222222',
        'Execution policy',
        'execution',
        'command',
        'Set command execution environment and PTY policy.',
        '{"fields":{"environment_policy":{"type":"string","enum":["inherit","clean","minimal_path"],"default":"inherit"},"pty_policy":{"type":"string","enum":["native_pty","disabled"],"default":"native_pty"}}}'::jsonb,
        $$[execution]
environment_policy = {{environment_policy}}
pty_policy = {{pty_policy}}
$$,
        '{"expandable":true,"affected_sections":["execution"],"patch_only":true,"predefined":true}'::jsonb,
        TRUE
    ),
    (
        '33333333-3333-4333-8333-333333333333',
        'Runtime tunnel adapter',
        'network',
        'runtime',
        'Adjust runtime tunnel adapter safety and reconciliation flags.',
        '{"fields":{"apply_enabled":{"type":"boolean"},"runtime_reconcile_enabled":{"type":"boolean"},"runtime_command_timeout_secs":{"type":"number","minimum":1,"maximum":120}}}'::jsonb,
        $$[network]
apply_enabled = {{apply_enabled}}
runtime_reconcile_enabled = {{runtime_reconcile_enabled}}
runtime_command_timeout_secs = {{runtime_command_timeout_secs}}
$$,
        '{"expandable":true,"affected_sections":["network"],"patch_only":true,"predefined":true}'::jsonb,
        TRUE
    ),
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
    ),
    (
        '44444444-4444-4444-8444-444444444444',
        'Routing daemon adapter',
        'network',
        'routing',
        'Configure interval latency monitoring and the agent-level fallback external OSPF cost updater. Tunnel-local updaters remain higher priority.',
        '{"fields":{"latency_monitoring_enabled":{"type":"boolean","default":true},"latency_monitoring_interval_secs":{"type":"number","minimum":15,"maximum":3600,"default":60},"latency_down_windows":{"type":"number","minimum":1,"maximum":60,"default":3},"auto_ospf_enabled":{"type":"boolean","default":false},"auto_ospf_min_cost_delta":{"type":"number","minimum":1,"maximum":65535,"default":5},"auto_ospf_healthy_windows":{"type":"number","minimum":1,"maximum":10,"default":2},"updater_argv":{"type":"array","default":["/usr/local/libexec/vpsman-ospf-cost"]}}}'::jsonb,
        $$[network]
latency_monitoring_enabled = {{latency_monitoring_enabled}}
latency_monitoring_interval_secs = {{latency_monitoring_interval_secs}}
latency_down_windows = {{latency_down_windows}}
auto_ospf_enabled = {{auto_ospf_enabled}}
auto_ospf_min_cost_delta = {{auto_ospf_min_cost_delta}}
auto_ospf_healthy_windows = {{auto_ospf_healthy_windows}}
auto_ospf_updater = { argv = {{updater_argv}}, max_timeout_secs = 10, max_output_bytes = 16384 }
$$,
        '{"expandable":true,"affected_sections":["network"],"patch_only":true,"predefined":true}'::jsonb,
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
