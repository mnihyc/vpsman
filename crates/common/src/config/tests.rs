use super::{
    validate_agent_config_shape, validate_hot_config_update, AgentBackupConfig, AgentConfig,
    AgentExecutionConfig, AgentExecutionEnvironmentPolicy, AgentExecutionProcessCleanupPolicy,
    AgentExecutionPtyPolicy, AgentNetworkConfig, AgentNetworkPreset, AgentNoiseConfig,
    AgentNoiseMode, AgentProcessInventorySource, AgentRuntimeStatusTelemetryPlan,
    AgentRuntimeTrafficSource, AgentTelemetryConfig, AgentTelemetrySource, AgentUserSessionsSource,
};
use crate::{
    plan_tunnel, BandwidthTier, OspfCostPolicy, RuntimeTunnelCommand, RuntimeTunnelControl,
    RuntimeTunnelManager, TunnelConfigBackend, TunnelEndpointSide, TunnelKind, TunnelPlanInput,
};

#[test]
fn validates_default_agent_config_shape() {
    validate_agent_config_shape(&AgentConfig::default()).unwrap();
}

#[test]
fn network_telemetry_defaults_stay_enabled_when_network_table_is_present() {
    let config: AgentConfig = toml::from_str(
        r#"
client_id = "edge-a"
display_name = "Edge A"
telemetry_light_secs = 15
telemetry_full_secs = 60
tags = []

[[tcp_endpoints]]
label = "primary"
tcp_addr = "127.0.0.1:9443"
priority = 10

[network]
root_dir = "/tmp/vpsman-network-root"
"#,
    )
    .unwrap();

    assert!(config.network.runtime_status_telemetry_enabled);
    assert_eq!(config.network.runtime_status_telemetry_interval_secs, 60);
    assert!(config.network.latency_monitoring_enabled);
    assert_eq!(config.network.latency_monitoring_interval_secs, 60);
    assert_eq!(config.network.latency_down_windows, 3);
    assert!(!config.network.auto_ospf_enabled);
    assert_eq!(config.network.auto_ospf_min_cost_delta, 5);
    assert_eq!(config.network.auto_ospf_healthy_windows, 2);
}

#[test]
fn validates_backup_limits() {
    let config = AgentConfig {
        backup: AgentBackupConfig {
            max_plaintext_bytes: 1024,
        },
        ..AgentConfig::default()
    };
    validate_agent_config_shape(&config).unwrap();

    let bad_limit = AgentConfig {
        backup: AgentBackupConfig {
            max_plaintext_bytes: 0,
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&bad_limit).unwrap_err(),
        "backup_max_plaintext_bytes_out_of_range"
    );
}

#[test]
fn validates_execution_source_selection() {
    let linux_defaults = AgentConfig {
        execution: AgentExecutionConfig {
            shell_script_argv: vec![
                "/usr/bin/env".to_string(),
                "sh".to_string(),
                "-lc".to_string(),
            ],
            process_proc_root: "/run/vpsman/proc".to_string(),
            ..AgentExecutionConfig::default()
        },
        ..AgentConfig::default()
    };
    validate_agent_config_shape(&linux_defaults).unwrap();

    let custom_sources = AgentConfig {
        execution: AgentExecutionConfig {
            working_directory: Some("/var/empty".to_string()),
            environment_policy: AgentExecutionEnvironmentPolicy::MinimalPath,
            environment_keep: vec!["TERM".to_string()],
            environment_set: [("VPSMAN_EXECUTION_MODE".to_string(), "batch".to_string())].into(),
            pty_policy: AgentExecutionPtyPolicy::Disabled,
            process_cleanup: AgentExecutionProcessCleanupPolicy::DirectChild,
            user_sessions_source: AgentUserSessionsSource::CustomCommand,
            user_sessions_command: Some(RuntimeTunnelCommand {
                argv: vec!["/usr/local/libexec/vpsman-users".to_string()],
                timeout_secs: 2,
                max_output_bytes: 4096,
            }),
            process_inventory_source: AgentProcessInventorySource::CustomCommand,
            process_inventory_command: Some(RuntimeTunnelCommand {
                argv: vec![
                    "/usr/local/libexec/vpsman-processes".to_string(),
                    "{limit}".to_string(),
                ],
                timeout_secs: 2,
                max_output_bytes: 4096,
            }),
            ..AgentExecutionConfig::default()
        },
        ..AgentConfig::default()
    };
    validate_agent_config_shape(&custom_sources).unwrap();

    let relative_shell = AgentConfig {
        execution: AgentExecutionConfig {
            shell_script_argv: vec!["sh".to_string(), "-lc".to_string()],
            ..AgentExecutionConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&relative_shell).unwrap_err(),
        "execution_shell_script_argv_executable_must_be_absolute"
    );

    let missing_user_command = AgentConfig {
        execution: AgentExecutionConfig {
            user_sessions_source: AgentUserSessionsSource::CustomCommand,
            ..AgentExecutionConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&missing_user_command).unwrap_err(),
        "execution_user_sessions_command_required"
    );

    let linux_process_with_command = AgentConfig {
        execution: AgentExecutionConfig {
            process_inventory_command: Some(RuntimeTunnelCommand {
                argv: vec!["/usr/local/libexec/vpsman-processes".to_string()],
                ..RuntimeTunnelCommand::default()
            }),
            ..AgentExecutionConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&linux_process_with_command).unwrap_err(),
        "execution_process_inventory_command_requires_custom_source"
    );

    let relative_process_command = AgentConfig {
        execution: AgentExecutionConfig {
            process_inventory_source: AgentProcessInventorySource::CustomCommand,
            process_inventory_command: Some(RuntimeTunnelCommand {
                argv: vec!["vpsman-processes".to_string()],
                ..RuntimeTunnelCommand::default()
            }),
            ..AgentExecutionConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&relative_process_command).unwrap_err(),
        "execution_process_inventory_argv_executable_must_be_absolute"
    );

    let relative_cwd = AgentConfig {
        execution: AgentExecutionConfig {
            working_directory: Some("tmp".to_string()),
            ..AgentExecutionConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&relative_cwd).unwrap_err(),
        "execution_working_directory_must_be_absolute"
    );

    let bad_env_key = AgentConfig {
        execution: AgentExecutionConfig {
            environment_keep: vec!["1BAD".to_string()],
            ..AgentExecutionConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&bad_env_key).unwrap_err(),
        "execution_environment_keep_key_invalid"
    );
}

#[test]
fn validates_telemetry_source_selection() {
    let linux = AgentConfig {
        telemetry: AgentTelemetryConfig {
            proc_root: "/run/vpsman/proc".to_string(),
            sys_class_net_dir: "/run/vpsman/sys/class/net".to_string(),
            hostname_file: Some("/run/vpsman/hostname".to_string()),
            ..AgentTelemetryConfig::default()
        },
        ..AgentConfig::default()
    };
    validate_agent_config_shape(&linux).unwrap();

    let custom = AgentConfig {
        telemetry: AgentTelemetryConfig {
            source: AgentTelemetrySource::CustomCommand,
            custom_metrics_command: Some(RuntimeTunnelCommand {
                argv: vec![
                    "/usr/local/libexec/vpsman-metrics-source".to_string(),
                    "{client_id}".to_string(),
                ],
                timeout_secs: 2,
                max_output_bytes: 4096,
            }),
            ..AgentTelemetryConfig::default()
        },
        ..AgentConfig::default()
    };
    validate_agent_config_shape(&custom).unwrap();

    let invalid_path = AgentConfig {
        telemetry: AgentTelemetryConfig {
            proc_root: "proc".to_string(),
            ..AgentTelemetryConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_path).unwrap_err(),
        "telemetry_proc_root_must_be_absolute"
    );

    let missing_command = AgentConfig {
        telemetry: AgentTelemetryConfig {
            source: AgentTelemetrySource::LinuxProcfsAndCustomCommand,
            ..AgentTelemetryConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&missing_command).unwrap_err(),
        "telemetry_custom_metrics_command_required"
    );

    let relative_command = AgentConfig {
        telemetry: AgentTelemetryConfig {
            source: AgentTelemetrySource::CustomCommand,
            custom_metrics_command: Some(RuntimeTunnelCommand {
                argv: vec!["vpsman-metrics".to_string()],
                ..RuntimeTunnelCommand::default()
            }),
            ..AgentTelemetryConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&relative_command).unwrap_err(),
        "telemetry_custom_metrics_argv_executable_must_be_absolute"
    );
}

#[test]
fn validates_network_apply_root() {
    let valid = AgentConfig {
        network: AgentNetworkConfig {
            apply_enabled: true,
            root_dir: "/tmp/vpsman-network-root".to_string(),
            validate_enabled: true,
            hook_timeout_secs: 5,
            ifupdown_validate_argv: vec!["/usr/bin/true".to_string()],
            bird2_status_argv: vec![
                "/usr/sbin/birdc".to_string(),
                "show".to_string(),
                "ospf".to_string(),
                "interface".to_string(),
                "{interface}".to_string(),
            ],
            probe_ping_argv: vec!["/usr/bin/ping".to_string()],
            status_probe_timeout_secs: 5,
            status_probe_max_output_bytes: 4096,
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    validate_agent_config_shape(&valid).unwrap();

    let invalid = AgentConfig {
        network: AgentNetworkConfig {
            apply_enabled: true,
            root_dir: "relative".to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid).unwrap_err(),
        "network_root_dir_must_be_absolute"
    );

    let invalid_hook = AgentConfig {
        network: AgentNetworkConfig {
            apply_enabled: true,
            root_dir: "/tmp/vpsman-network-root".to_string(),
            validate_enabled: true,
            ifupdown_validate_argv: vec!["true".to_string()],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_hook).unwrap_err(),
        "network_ifupdown_validate_argv_executable_must_be_absolute"
    );

    let invalid_reload = AgentConfig {
        network: AgentNetworkConfig {
            apply_enabled: true,
            root_dir: "/tmp/vpsman-network-root".to_string(),
            reload_enabled: true,
            reload_argv: vec![vec!["/usr/bin/true".to_string()]],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_reload).unwrap_err(),
        "network_reload_requires_validation"
    );

    for preset in [
        AgentNetworkPreset::DebianIfupdown2Bird2,
        AgentNetworkPreset::DebianIfupdownBird2,
    ] {
        let preset_validation = AgentConfig {
            network: AgentNetworkConfig {
                apply_enabled: true,
                preset: Some(preset),
                root_dir: "/tmp/vpsman-network-root".to_string(),
                validate_enabled: true,
                reload_enabled: true,
                ..AgentNetworkConfig::default()
            },
            ..AgentConfig::default()
        };
        validate_agent_config_shape(&preset_validation).unwrap();
    }

    for (backend, preset) in [
        (
            TunnelConfigBackend::Netplan,
            AgentNetworkPreset::DebianNetplanBird2,
        ),
        (
            TunnelConfigBackend::SystemdNetworkd,
            AgentNetworkPreset::DebianSystemdNetworkdBird2,
        ),
    ] {
        let preset_validation = AgentConfig {
            network: AgentNetworkConfig {
                apply_enabled: true,
                backend,
                preset: Some(preset),
                root_dir: "/tmp/vpsman-network-root".to_string(),
                validate_enabled: true,
                reload_enabled: true,
                ..AgentNetworkConfig::default()
            },
            ..AgentConfig::default()
        };
        validate_agent_config_shape(&preset_validation).unwrap();
    }

    let invalid_backend_preset = AgentConfig {
        network: AgentNetworkConfig {
            apply_enabled: true,
            backend: TunnelConfigBackend::Netplan,
            preset: Some(AgentNetworkPreset::DebianIfupdown2Bird2),
            root_dir: "/tmp/vpsman-network-root".to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_backend_preset).unwrap_err(),
        "network_backend_preset_mismatch"
    );

    let invalid_status_probe = AgentConfig {
        network: AgentNetworkConfig {
            root_dir: "/tmp/vpsman-network-root".to_string(),
            bird2_status_argv: vec!["birdc".to_string(), "show".to_string()],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_status_probe).unwrap_err(),
        "network_bird2_status_argv_executable_must_be_absolute"
    );

    let invalid_probe_ping = AgentConfig {
        network: AgentNetworkConfig {
            root_dir: "/tmp/vpsman-network-root".to_string(),
            probe_ping_argv: vec!["ping".to_string()],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_probe_ping).unwrap_err(),
        "network_probe_ping_argv_executable_must_be_absolute"
    );

    let invalid_bird2_reload = AgentConfig {
        network: AgentNetworkConfig {
            root_dir: "/tmp/vpsman-network-root".to_string(),
            validate_enabled: true,
            reload_enabled: true,
            bird2_validate_argv: vec!["/usr/bin/true".to_string()],
            bird2_reload_argv: vec![vec!["birdc".to_string(), "configure".to_string()]],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_bird2_reload).unwrap_err(),
        "network_bird2_reload_argv_executable_must_be_absolute"
    );

    let invalid_status_output_limit = AgentConfig {
        network: AgentNetworkConfig {
            root_dir: "/tmp/vpsman-network-root".to_string(),
            status_probe_max_output_bytes: 100,
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_status_output_limit).unwrap_err(),
        "network_status_probe_max_output_bytes_out_of_range"
    );

    let runtime_without_apply = AgentConfig {
        network: AgentNetworkConfig {
            root_dir: "/tmp/vpsman-network-root".to_string(),
            runtime_reconcile_enabled: true,
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&runtime_without_apply).unwrap_err(),
        "network_runtime_reconcile_requires_apply_enabled"
    );

    let invalid_runtime_ip = AgentConfig {
        network: AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            root_dir: "/tmp/vpsman-network-root".to_string(),
            runtime_ip_argv: vec!["ip".to_string()],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_runtime_ip).unwrap_err(),
        "network_runtime_ip_argv_executable_must_be_absolute"
    );

    let invalid_runtime_output_limit = AgentConfig {
        network: AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            root_dir: "/tmp/vpsman-network-root".to_string(),
            runtime_command_max_output_bytes: 100,
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_runtime_output_limit).unwrap_err(),
        "network_runtime_command_max_output_bytes_out_of_range"
    );

    let telemetry_plan = plan_tunnel(&TunnelPlanInput {
        name: "adapter-a-b".to_string(),
        interface_name: "ovpn42".to_string(),
        kind: TunnelKind::Openvpn,
        runtime_control: RuntimeTunnelControl {
            manager: RuntimeTunnelManager::ExternalManagedAdapter,
            status: Some(RuntimeTunnelCommand {
                argv: vec!["/usr/local/libexec/vpsman-adapter".to_string()],
                ..RuntimeTunnelCommand::default()
            }),
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "edge-a".to_string(),
        right_client_id: "edge-b".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "198.51.100.11".to_string(),
        address_pool_cidr: "10.42.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(crate::TunnelAddressPair {
            left: "10.42.0.0".to_string(),
            right: "10.42.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 12.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .unwrap();
    let valid_runtime_status_telemetry = AgentConfig {
        network: AgentNetworkConfig {
            root_dir: "/tmp/vpsman-network-root".to_string(),
            runtime_status_telemetry_plans: vec![AgentRuntimeStatusTelemetryPlan {
                plan_id: Some("plan-a".to_string()),
                endpoint_side: TunnelEndpointSide::Left,
                plan: telemetry_plan.clone(),
                traffic_source: AgentRuntimeTrafficSource::InterfaceCounters,
                traffic_command: None,
                latency_monitoring_enabled: true,
                auto_ospf_enabled: false,
                auto_ospf_updater: None,
            }],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    validate_agent_config_shape(&valid_runtime_status_telemetry).unwrap();

    let custom_runtime_status_telemetry = AgentConfig {
        network: AgentNetworkConfig {
            root_dir: "/tmp/vpsman-network-root".to_string(),
            runtime_status_telemetry_plans: vec![AgentRuntimeStatusTelemetryPlan {
                plan_id: Some("plan-a".to_string()),
                endpoint_side: TunnelEndpointSide::Left,
                plan: telemetry_plan.clone(),
                traffic_source: AgentRuntimeTrafficSource::CustomCommand,
                traffic_command: Some(RuntimeTunnelCommand {
                    argv: vec![
                        "/usr/local/libexec/vpsman-traffic-source".to_string(),
                        "{interface}".to_string(),
                    ],
                    timeout_secs: 2,
                    max_output_bytes: 1024,
                }),
                latency_monitoring_enabled: true,
                auto_ospf_enabled: false,
                auto_ospf_updater: None,
            }],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    validate_agent_config_shape(&custom_runtime_status_telemetry).unwrap();

    let invalid_custom_runtime_status_telemetry = AgentConfig {
        network: AgentNetworkConfig {
            root_dir: "/tmp/vpsman-network-root".to_string(),
            runtime_status_telemetry_plans: vec![AgentRuntimeStatusTelemetryPlan {
                plan_id: Some("plan-a".to_string()),
                endpoint_side: TunnelEndpointSide::Left,
                plan: telemetry_plan.clone(),
                traffic_source: AgentRuntimeTrafficSource::CustomCommand,
                traffic_command: None,
                latency_monitoring_enabled: true,
                auto_ospf_enabled: false,
                auto_ospf_updater: None,
            }],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_custom_runtime_status_telemetry).unwrap_err(),
        "network_runtime_traffic_custom_command_required"
    );

    let mut invalid_plan = telemetry_plan;
    invalid_plan.runtime_control.status.as_mut().unwrap().argv = vec!["vpsman-adapter".to_string()];
    let invalid_runtime_status_telemetry = AgentConfig {
        network: AgentNetworkConfig {
            root_dir: "/tmp/vpsman-network-root".to_string(),
            runtime_status_telemetry_plans: vec![AgentRuntimeStatusTelemetryPlan {
                plan_id: Some("plan-a".to_string()),
                endpoint_side: TunnelEndpointSide::Left,
                plan: invalid_plan,
                traffic_source: AgentRuntimeTrafficSource::InterfaceCounters,
                traffic_command: None,
                latency_monitoring_enabled: true,
                auto_ospf_enabled: false,
                auto_ospf_updater: None,
            }],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid_runtime_status_telemetry).unwrap_err(),
        "network_runtime_status_telemetry_control_invalid"
    );
}

#[test]
fn rejects_hot_config_identity_and_secret_changes() {
    let current = AgentConfig {
        auth: super::AgentAuthConfig {
            command_timeout_secs: 30,
            ..Default::default()
        },
        noise: AgentNoiseConfig {
            mode: AgentNoiseMode::EnrolledIk,
            client_private_key_hex: Some("22".repeat(32)),
            server_public_key_hex: Some("33".repeat(32)),
        },
        ..AgentConfig::default()
    };
    let mut updated = current.clone();
    updated.display_name = "new display".to_string();
    updated.auth.command_timeout_secs = 60;
    validate_hot_config_update(&current, &updated).unwrap();

    updated.client_id = "other".to_string();
    assert_eq!(
        validate_hot_config_update(&current, &updated).unwrap_err(),
        "hot_config_cannot_change_client_id"
    );

    let mut updated = current.clone();
    updated.update.unmanaged_interval_secs = 3600;
    validate_hot_config_update(&current, &updated).unwrap();
}
