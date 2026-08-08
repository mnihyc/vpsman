use super::{
    default_agent_backup_max_archive_bytes, validate_agent_bootstrap_config_shape,
    validate_agent_config_shape, AgentBackupConfig, AgentConfig, AgentRuntimeConfig,
    AgentRuntimeStatusTelemetryPlan,
};
use crate::{
    pair_port_expressions, plan_tunnel, port_forwarding_desired_hash, AgentPortForwardingConfig,
    PortForwardProtocol, PortForwardRule, RuntimeTunnelAdapterCommands, RuntimeTunnelCommand,
    RuntimeTunnelManager, TunnelAddressFamily, TunnelAddressPair, TunnelEndpointSide, TunnelKind,
    TunnelPlanInput,
};

fn explicit_plan(manager: RuntimeTunnelManager) -> crate::TunnelPlan {
    let kind = if manager == RuntimeTunnelManager::AgentBuiltin {
        TunnelKind::Gre
    } else {
        TunnelKind::Wireguard
    };
    plan_tunnel(&TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "tunab".to_string(),
        kind,
        runtime_control: crate::RuntimeTunnelControl {
            manager,
            left_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
                .then(|| "11111111-1111-4111-8111-111111111111".to_string()),
            right_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
                .then(|| "22222222-2222-4222-8222-222222222222".to_string()),
            ..crate::RuntimeTunnelControl::default()
        },
        runtime_topology: crate::RuntimeTunnelTopologyIntent::default(),
        left_client_id: "edge-a".to_string(),
        right_client_id: "edge-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.255.0.0/24".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: TunnelAddressFamily::Ipv4,
        bandwidth_mbps: 100,
        left_mtu: (manager == RuntimeTunnelManager::AgentBuiltin)
            .then(|| crate::default_tunnel_mtu(kind))
            .flatten(),
        right_mtu: (manager == RuntimeTunnelManager::AgentBuiltin)
            .then(|| crate::default_tunnel_mtu(kind))
            .flatten(),
        ospf: None,
    })
    .unwrap()
}

fn command(name: &str) -> RuntimeTunnelCommand {
    RuntimeTunnelCommand {
        argv: vec![format!("/opt/vpsman-adapters/{name}")],
        max_timeout_secs: 10,
        max_output_bytes: 16 * 1024,
    }
}

fn runtime_adapter() -> RuntimeTunnelAdapterCommands {
    RuntimeTunnelAdapterCommands {
        definition_id: "11111111-1111-4111-8111-111111111111".to_string(),
        definition_name: "edge-a-wireguard".to_string(),
        definition_hash: "ab".repeat(32),
        startup: Some(command("start")),
        stop: Some(command("stop")),
        cleanup: None,
        restart: None,
        status: command("status"),
        traffic_limit_apply: None,
    }
}

#[test]
fn validates_default_agent_config_shape() {
    validate_agent_config_shape(&AgentConfig::default()).unwrap();
    validate_agent_bootstrap_config_shape(&AgentConfig::default()).unwrap();
}

#[test]
fn bootstrap_config_rejects_server_managed_port_forwarding() {
    let rules = vec![PortForwardRule {
        id: uuid::Uuid::parse_str("018f89ac-a5ec-7d71-a249-7ccddc0a0001").unwrap(),
        revision: 1,
        name: "web".to_string(),
        protocol: PortForwardProtocol::Tcp,
        target_ip: "192.0.2.10".parse().unwrap(),
        mappings: pair_port_expressions("443", "8443").unwrap(),
        masquerade: true,
    }];
    let mut config = AgentConfig::default();
    config.network.port_forwarding = AgentPortForwardingConfig {
        desired_hash: port_forwarding_desired_hash(&rules),
        rules,
        ..AgentPortForwardingConfig::default()
    };

    validate_agent_config_shape(&config).unwrap();
    assert_eq!(
        validate_agent_bootstrap_config_shape(&config).unwrap_err(),
        "network_port_forwarding_server_managed"
    );
}

#[test]
fn network_telemetry_defaults_stay_enabled() {
    let config = AgentConfig::default();
    assert!(config.network.runtime_status_telemetry_enabled);
    assert_eq!(config.network.runtime_status_telemetry_interval_secs, 60);
    assert!(config.network.latency_monitoring_enabled);
    assert_eq!(config.network.latency_monitoring_interval_secs, 60);
    assert_eq!(config.network.latency_down_windows, 3);
}

#[test]
fn ospf_updater_commands_are_paired_and_bounded() {
    let mut config = AgentConfig::default();
    config.network.ospf_status_command = Some(command("ospf-status"));
    assert_eq!(
        validate_agent_config_shape(&config).unwrap_err(),
        "network_ospf_commands_must_be_configured_together"
    );

    config.network.ospf_update_command = Some(command("ospf-update"));
    validate_agent_config_shape(&config).unwrap();

    config
        .network
        .ospf_update_command
        .as_mut()
        .unwrap()
        .max_timeout_secs = 0;
    assert!(validate_agent_config_shape(&config)
        .unwrap_err()
        .starts_with("network_ospf_update_command"));
}

#[test]
fn backup_limits_remain_bounded() {
    assert!(default_agent_backup_max_archive_bytes() > 0);
    let valid = AgentConfig {
        backup: AgentBackupConfig {
            max_uncompressed_bytes: 1024,
            max_archive_bytes: 4096,
        },
        ..AgentConfig::default()
    };
    validate_agent_config_shape(&valid).unwrap();

    let invalid = AgentConfig {
        backup: AgentBackupConfig {
            max_uncompressed_bytes: 4096,
            max_archive_bytes: 1024,
        },
        ..AgentConfig::default()
    };
    assert_eq!(
        validate_agent_config_shape(&invalid).unwrap_err(),
        "backup_max_archive_bytes_below_uncompressed_limit"
    );
}

#[test]
fn runtime_config_ignores_additive_future_fields() {
    let config: AgentRuntimeConfig = serde_json::from_value(serde_json::json!({
        "version": 42,
        "display_name": "edge-a",
        "future_runtime_section": { "enabled": true },
        "backup": {
            "max_uncompressed_bytes": 1024,
            "max_archive_bytes": 4096,
            "future_backup_policy": "incremental"
        }
    }))
    .unwrap();

    assert_eq!(config.version, 42);
    assert_eq!(config.display_name, "edge-a");
    assert_eq!(config.backup.max_archive_bytes, 4096);
}

#[test]
fn explicit_agent_builtin_plan_needs_no_adapter_snapshot() {
    let mut config = AgentConfig::default();
    config.network.runtime_status_telemetry_plans = vec![AgentRuntimeStatusTelemetryPlan {
        plan_id: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string()),
        topology_identity_hash: "0".repeat(64),
        endpoint_side: TunnelEndpointSide::Left,
        plan: explicit_plan(RuntimeTunnelManager::AgentBuiltin),
        builtin_credentials: None,
        runtime_adapter: None,
        latency_monitoring_enabled: true,
    }];
    validate_agent_config_shape(&config).unwrap();
}

#[test]
fn stateful_builtin_runtime_plans_require_uuid_identity() {
    let mut config = AgentConfig::default();
    let mut plan = explicit_plan(RuntimeTunnelManager::AgentBuiltin);
    plan.kind = TunnelKind::Wireguard;
    config.network.runtime_status_telemetry_plans = vec![AgentRuntimeStatusTelemetryPlan {
        plan_id: None,
        topology_identity_hash: "0".repeat(64),
        endpoint_side: TunnelEndpointSide::Left,
        plan,
        builtin_credentials: None,
        runtime_adapter: None,
        latency_monitoring_enabled: true,
    }];

    assert_eq!(
        validate_agent_config_shape(&config).unwrap_err(),
        "network_runtime_status_telemetry_plan_id_required"
    );
    config.network.runtime_status_telemetry_plans[0].plan_id = Some("not-a-uuid".to_string());
    assert_eq!(
        validate_agent_config_shape(&config).unwrap_err(),
        "network_runtime_status_telemetry_plan_id_invalid"
    );
}

#[test]
fn runtime_config_rejects_driver_options_on_the_wrong_tunnel_kind() {
    let mut config = AgentConfig::default();
    let mut plan = explicit_plan(RuntimeTunnelManager::AgentBuiltin);
    plan.runtime_control.wireguard.left_listen_port = 51_821;
    config.network.runtime_status_telemetry_plans = vec![AgentRuntimeStatusTelemetryPlan {
        plan_id: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string()),
        topology_identity_hash: "0".repeat(64),
        endpoint_side: TunnelEndpointSide::Left,
        plan,
        builtin_credentials: None,
        runtime_adapter: None,
        latency_monitoring_enabled: true,
    }];

    assert_eq!(
        validate_agent_config_shape(&config).unwrap_err(),
        "network_runtime_status_telemetry_control_invalid"
    );
}

#[test]
fn custom_adapter_plan_requires_the_bound_adapter_snapshot() {
    let mut config = AgentConfig::default();
    let plan = explicit_plan(RuntimeTunnelManager::CustomAdapter);
    config.network.runtime_status_telemetry_plans = vec![AgentRuntimeStatusTelemetryPlan {
        plan_id: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string()),
        topology_identity_hash: "0".repeat(64),
        endpoint_side: TunnelEndpointSide::Left,
        plan: plan.clone(),
        builtin_credentials: None,
        runtime_adapter: None,
        latency_monitoring_enabled: true,
    }];
    assert_eq!(
        validate_agent_config_shape(&config).unwrap_err(),
        "network_runtime_adapter_snapshot_required"
    );

    config.network.runtime_status_telemetry_plans[0].runtime_adapter = Some(runtime_adapter());
    validate_agent_config_shape(&config).unwrap();

    config.network.runtime_status_telemetry_plans[0]
        .runtime_adapter
        .as_mut()
        .unwrap()
        .definition_id = "22222222-2222-4222-8222-222222222222".to_string();
    assert_eq!(
        validate_agent_config_shape(&config).unwrap_err(),
        "network_runtime_adapter_snapshot_binding_mismatch"
    );
}

#[test]
fn observed_plan_rejects_an_adapter_snapshot() {
    let mut config = AgentConfig::default();
    config.network.runtime_status_telemetry_plans = vec![AgentRuntimeStatusTelemetryPlan {
        plan_id: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string()),
        topology_identity_hash: "0".repeat(64),
        endpoint_side: TunnelEndpointSide::Left,
        plan: explicit_plan(RuntimeTunnelManager::ExternalObserved),
        builtin_credentials: None,
        runtime_adapter: Some(runtime_adapter()),
        latency_monitoring_enabled: true,
    }];
    assert_eq!(
        validate_agent_config_shape(&config).unwrap_err(),
        "network_runtime_adapter_snapshot_forbidden"
    );
}

#[test]
fn observed_plan_reconcile_does_not_require_mutation() {
    let mut config = AgentConfig::default();
    config.network.runtime_reconcile_enabled = true;
    config.network.runtime_status_telemetry_plans = vec![AgentRuntimeStatusTelemetryPlan {
        plan_id: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string()),
        topology_identity_hash: "0".repeat(64),
        endpoint_side: TunnelEndpointSide::Left,
        plan: explicit_plan(RuntimeTunnelManager::ExternalObserved),
        builtin_credentials: None,
        runtime_adapter: None,
        latency_monitoring_enabled: true,
    }];

    validate_agent_config_shape(&config).unwrap();
}

#[test]
fn managed_plan_reconcile_requires_mutation() {
    let mut config = AgentConfig::default();
    config.network.runtime_reconcile_enabled = true;
    config.network.runtime_status_telemetry_plans = vec![AgentRuntimeStatusTelemetryPlan {
        plan_id: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string()),
        topology_identity_hash: "0".repeat(64),
        endpoint_side: TunnelEndpointSide::Left,
        plan: explicit_plan(RuntimeTunnelManager::AgentBuiltin),
        builtin_credentials: None,
        runtime_adapter: None,
        latency_monitoring_enabled: true,
    }];

    assert_eq!(
        validate_agent_config_shape(&config).unwrap_err(),
        "network_runtime_reconcile_requires_apply_enabled"
    );
}
