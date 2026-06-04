CREATE TABLE data_source_presets (
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
    CONSTRAINT data_source_presets_scope_check
        CHECK (scope IN ('built_in', 'shared', 'vps_local')),
    CONSTRAINT data_source_presets_definition_object_check
        CHECK (jsonb_typeof(definition) = 'object'),
    CONSTRAINT data_source_presets_owner_scope_check
        CHECK (
            (scope = 'vps_local' AND owner_client_id IS NOT NULL)
            OR (scope <> 'vps_local' AND owner_client_id IS NULL)
        )
);

CREATE UNIQUE INDEX data_source_presets_global_name_idx
    ON data_source_presets(domain, name, scope)
    WHERE owner_client_id IS NULL;

CREATE UNIQUE INDEX data_source_presets_client_name_idx
    ON data_source_presets(domain, owner_client_id, name)
    WHERE owner_client_id IS NOT NULL;

CREATE UNIQUE INDEX data_source_presets_default_idx
    ON data_source_presets(domain)
    WHERE is_default;

CREATE TABLE client_data_source_preset_assignments (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    domain TEXT NOT NULL,
    preset_id UUID NOT NULL REFERENCES data_source_presets(id) ON DELETE RESTRICT,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, domain)
);

CREATE INDEX client_data_source_preset_assignments_preset_idx
    ON client_data_source_preset_assignments(preset_id);

INSERT INTO data_source_presets (
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
        'builtin:local_filesystem_or_https',
        'built_in',
        TRUE,
        TRUE,
        'Default signed update artifact source using hosted filesystem or HTTPS URL',
        '{"provider":"local_filesystem_or_https"}'::jsonb
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
        'Reserved iperf3 JSON provider adapter preset for fleets that standardize on iperf3',
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
        '{"source":"linux_w_who_preset","user_sessions_command":{"argv":["/usr/bin/w","-h"],"timeout_secs":5,"max_output_bytes":16384}}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-000000000062',
        'user_session_inventory_source',
        'builtin:usr_bin_who',
        'built_in',
        TRUE,
        FALSE,
        'Pinned /usr/bin/who session inventory source',
        '{"source":"linux_w_who_preset","user_sessions_command":{"argv":["/usr/bin/who"],"timeout_secs":5,"max_output_bytes":16384}}'::jsonb
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
        'Reserved S3/MinIO path-style encrypted artifact adapter preset',
        '{"provider":"s3_path_style","requires_server_env":["VPSMAN_OBJECT_ENDPOINT","VPSMAN_OBJECT_BUCKET","VPSMAN_OBJECT_ACCESS_KEY","VPSMAN_OBJECT_SECRET_KEY"]}'::jsonb
    ),
    (
        '00000000-0000-4000-8000-0000000000a1',
        'update_artifact_source',
        'builtin:https_signed_artifact',
        'built_in',
        TRUE,
        FALSE,
        'Signed HTTPS update artifact source preset for externally hosted releases',
        '{"provider":"https_signed_artifact","requires_sha256":true,"requires_signature":true}'::jsonb
    )
ON CONFLICT (id) DO NOTHING;

INSERT INTO client_data_source_preset_assignments (client_id, domain, preset_id)
SELECT clients.id, presets.domain, presets.id
FROM clients
CROSS JOIN data_source_presets presets
WHERE presets.is_default
ON CONFLICT (client_id, domain) DO NOTHING;
