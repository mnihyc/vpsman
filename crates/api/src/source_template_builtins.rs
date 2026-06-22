pub(crate) struct BuiltInSourceTemplate {
    pub(crate) id: &'static str,
    pub(crate) domain: &'static str,
    pub(crate) name: &'static str,
    pub(crate) is_default: bool,
    pub(crate) description: &'static str,
    pub(crate) definition: serde_json::Value,
}

pub(crate) const SOURCE_TEMPLATE_DOMAINS: &[&str] = &[
    "telemetry_metrics_source",
    "runtime_traffic_accounting_source",
    "latency_probe_source",
    "speed_test_provider",
    "process_inventory_source",
    "user_session_inventory_source",
    "command_execution_policy",
    "process_supervisor_policy",
    "runtime_tunnel_adapter",
    "traffic_limit_status_source",
    "routing_daemon_adapter",
    "backup_object_store",
    "restore_path_mapping",
    "update_artifact_source",
    "update_restart_policy",
    "update_rollback_heartbeat_source",
];

pub(crate) fn builtin_source_templates() -> Vec<BuiltInSourceTemplate> {
    vec![
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000001",
            domain: "telemetry_metrics_source",
            name: "builtin:linux_procfs",
            is_default: true,
            description: "Default low-cost Linux procfs/sysfs telemetry source",
            definition: serde_json::json!({"source": "linux_procfs"}),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000011",
            domain: "telemetry_metrics_source",
            name: "builtin:host_mounted_procfs",
            is_default: false,
            description: "Container or chroot telemetry source reading host-mounted proc/sys trees",
            definition: serde_json::json!({
                "source": "linux_procfs",
                "proc_root": "/host/proc",
                "sys_class_net_dir": "/host/sys/class/net",
                "hostname_file": "/host/etc/hostname",
                "os_release_file": "/host/etc/os-release"
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000002",
            domain: "runtime_traffic_accounting_source",
            name: "builtin:interface_counters",
            is_default: true,
            description: "Default runtime tunnel traffic accounting from interface counters",
            definition: serde_json::json!({"source": "interface_counters"}),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000021",
            domain: "runtime_traffic_accounting_source",
            name: "builtin:vnstat_json",
            is_default: false,
            description:
                "Common vnstat JSON traffic accounting source for provider images with vnstat installed",
            definition: serde_json::json!({
                "source": "vnstat",
                "vnstat_argv": ["/usr/bin/vnstat"]
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000003",
            domain: "latency_probe_source",
            name: "builtin:linux_ping",
            is_default: true,
            description: "Default ICMP latency/loss probe using Linux ping template candidates",
            definition: serde_json::json!({"source": "linux_ping_preset"}),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000031",
            domain: "latency_probe_source",
            name: "builtin:usr_bin_ping",
            is_default: false,
            description:
                "Pinned /usr/bin/ping latency/loss probe for hosts where path discovery is undesirable",
            definition: serde_json::json!({
                "source": "configured_ping_argv",
                "probe_ping_argv": ["/usr/bin/ping"]
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000004",
            domain: "speed_test_provider",
            name: "builtin:tcp_throughput",
            is_default: true,
            description: "Default bounded two-endpoint TCP throughput provider",
            definition: serde_json::json!({"provider": "tcp_throughput"}),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000041",
            domain: "speed_test_provider",
            name: "builtin:iperf3_json_adapter",
            is_default: false,
            description:
                "Reserved iperf3 JSON provider adapter template for fleets that standardize on iperf3",
            definition: serde_json::json!({
                "provider": "iperf3_json_adapter",
                "server_argv": ["/usr/bin/iperf3", "--server", "--one-off", "--json"],
                "client_argv": ["/usr/bin/iperf3", "--client", "{server_address}", "--json"]
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000005",
            domain: "process_inventory_source",
            name: "builtin:linux_procfs",
            is_default: true,
            description: "Default process inventory from configurable Linux procfs root",
            definition: serde_json::json!({"source": "linux_procfs"}),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000051",
            domain: "process_inventory_source",
            name: "builtin:host_mounted_procfs",
            is_default: false,
            description: "Process inventory from a host-mounted /proc tree",
            definition: serde_json::json!({
                "source": "linux_procfs",
                "proc_root": "/host/proc"
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000006",
            domain: "user_session_inventory_source",
            name: "builtin:linux_w_who",
            is_default: true,
            description: "Default user/session inventory using Linux w/who candidates",
            definition: serde_json::json!({"source": "linux_w_who_preset"}),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000061",
            domain: "user_session_inventory_source",
            name: "builtin:usr_bin_w",
            is_default: false,
            description: "Pinned /usr/bin/w session inventory source",
            definition: serde_json::json!({
                "source": "linux_w_who_preset",
                "user_sessions_command": {
                    "argv": ["/usr/bin/w", "-h"],
                    "max_timeout_secs": 5,
                    "max_output_bytes": 16384
                }
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000062",
            domain: "user_session_inventory_source",
            name: "builtin:usr_bin_who",
            is_default: false,
            description: "Pinned /usr/bin/who session inventory source",
            definition: serde_json::json!({
                "source": "linux_w_who_preset",
                "user_sessions_command": {
                    "argv": ["/usr/bin/who"],
                    "max_timeout_secs": 5,
                    "max_output_bytes": 16384
                }
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000007",
            domain: "command_execution_policy",
            name: "builtin:linux_shell_argv",
            is_default: true,
            description: "Default Linux shell-script argv prefix policy",
            definition: serde_json::json!({
                "shell_script_argv": ["/bin/sh", "-lc"],
                "environment_policy": "inherit",
                "pty_policy": "native_pty",
                "process_cleanup": "process_group"
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000071",
            domain: "command_execution_policy",
            name: "builtin:busybox_ash_argv",
            is_default: false,
            description: "BusyBox ash shell-script argv prefix for minimal images",
            definition: serde_json::json!({
                "shell_script_argv": ["/bin/ash", "-lc"],
                "environment_policy": "minimal_path",
                "environment_keep": ["TERM"],
                "pty_policy": "native_pty",
                "process_cleanup": "process_group"
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000072",
            domain: "command_execution_policy",
            name: "builtin:clean_batch_execution",
            is_default: false,
            description: "Clean environment batch execution with PTY disabled",
            definition: serde_json::json!({
                "shell_script_argv": ["/bin/sh", "-lc"],
                "environment_policy": "clean",
                "environment_keep": ["PATH", "HOME", "LANG", "LC_ALL"],
                "pty_policy": "disabled",
                "process_cleanup": "process_group"
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000008",
            domain: "runtime_tunnel_adapter",
            name: "builtin:agent_iproute2_managed",
            is_default: true,
            description: "Default client-managed iproute2/tc runtime tunnel adapter",
            definition: serde_json::json!({"manager": "agent_iproute2_managed"}),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000081",
            domain: "runtime_tunnel_adapter",
            name: "builtin:agent_iproute2_runtime_reconcile",
            is_default: false,
            description:
                "Client-managed iproute2/tc runtime tunnel adapter with runtime reconciliation enabled",
            definition: serde_json::json!({
                "manager": "agent_iproute2_managed",
                "runtime_reconcile_enabled": true,
                "runtime_ip_argv": ["/sbin/ip"],
                "runtime_tc_argv": ["/sbin/tc"]
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000082",
            domain: "process_supervisor_policy",
            name: "builtin:agent_supervisor",
            is_default: true,
            description:
                "Default agent-managed process supervisor policy with capability-derived limit evidence",
            definition: serde_json::json!({
                "source": "agent_supervisor",
                "restart_policy_source": "process_run_policy",
                "limit_source": "agent_capability_snapshot"
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000083",
            domain: "traffic_limit_status_source",
            name: "builtin:tunnel_plan_tc_status",
            is_default: true,
            description:
                "Default traffic-limit readiness derived from tunnel plan runtime control and telemetry",
            definition: serde_json::json!({
                "source": "tunnel_plan_runtime_control",
                "status_source": "network_status_and_telemetry"
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000084",
            domain: "routing_daemon_adapter",
            name: "builtin:bird2_ospf",
            is_default: true,
            description:
                "Default Bird2 OSPF adapter for topology evidence, neighbor checks, and cost updates",
            definition: serde_json::json!({
                "provider": "bird2",
                "workflow": "network_ospf_cost_update",
                "status_source": "bird2_status"
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000009",
            domain: "backup_object_store",
            name: "builtin:local_filesystem",
            is_default: true,
            description: "Default local filesystem object-store adapter with reserved S3 extension",
            definition: serde_json::json!({"provider": "local_filesystem"}),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000091",
            domain: "backup_object_store",
            name: "builtin:s3_path_style_reserved",
            is_default: false,
            description: "Reserved S3/MinIO path-style backup artifact adapter template",
            definition: serde_json::json!({
                "provider": "s3_path_style",
                "requires_server_env": [
                    "VPSMAN_OBJECT_ENDPOINT",
                    "VPSMAN_OBJECT_BUCKET",
                    "VPSMAN_OBJECT_ACCESS_KEY",
                    "VPSMAN_OBJECT_SECRET_KEY"
                ]
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-000000000092",
            domain: "restore_path_mapping",
            name: "builtin:explicit_restore_paths",
            is_default: true,
            description:
                "Default restore and migration path mapping from operator-reviewed restore plans",
            definition: serde_json::json!({
                "source": "restore_plan",
                "mapping_mode": "explicit_paths",
                "supports_agent_local_archive": true,
                "supports_post_restore_hooks": true
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-00000000000a",
            domain: "update_artifact_source",
            name: "builtin:external_https_sha256",
            is_default: true,
            description:
                "Default external HTTPS update artifact source with SHA-256 verification",
            definition: serde_json::json!({
                "provider": "external_https",
                "requires_sha256": true
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-0000000000a1",
            domain: "update_artifact_source",
            name: "builtin:github_release_sha256",
            is_default: false,
            description:
                "GitHub Releases update artifact source using version.json download URLs and SHA256SUMS",
            definition: serde_json::json!({
                "provider": "github_release",
                "requires_sha256": true,
                "manifest": "version_json_download_urls_sha256sums"
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-0000000000a2",
            domain: "update_restart_policy",
            name: "builtin:agent_self_restart",
            is_default: true,
            description:
                "Default agent self-restart policy using the deployed service or manual supervisor hook",
            definition: serde_json::json!({
                "source": "agent_update_activate",
                "restart_method": "agent_configured",
                "fallback": "manual_supervisor"
            }),
        },
        BuiltInSourceTemplate {
            id: "00000000-0000-4000-8000-0000000000a3",
            domain: "update_rollback_heartbeat_source",
            name: "builtin:update_heartbeat_marker",
            is_default: true,
            description:
                "Default update rollback heartbeat evidence from agent activation heartbeat markers",
            definition: serde_json::json!({
                "source": "agent_update_heartbeat",
                "health_gate": "heartbeat_verified"
            }),
        },
    ]
}
