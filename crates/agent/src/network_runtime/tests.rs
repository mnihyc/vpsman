use super::*;
use vpsman_common::{
    plan_tunnel, RuntimeTunnelAdapterCommands, RuntimeTunnelControl, TunnelAddressFamily,
    TunnelAddressPair, TunnelPlanInput,
};

const LEFT_ADAPTER_ID: &str = "11111111-1111-4111-8111-111111111111";
const RIGHT_ADAPTER_ID: &str = "22222222-2222-4222-8222-222222222222";

fn command(argv: &[&str]) -> RuntimeTunnelCommand {
    RuntimeTunnelCommand {
        argv: argv.iter().map(|value| value.to_string()).collect(),
        max_timeout_secs: 10,
        max_output_bytes: 16 * 1024,
    }
}

fn adapter() -> RuntimeTunnelAdapterCommands {
    RuntimeTunnelAdapterCommands {
        template_id: LEFT_ADAPTER_ID.to_string(),
        template_name: "wireguard-runtime".to_string(),
        definition_hash: "ab".repeat(32),
        startup: Some(command(&[
            "/opt/vpsman-adapters/wg-runtime",
            "start",
            "{interface}",
            "{local_underlay}",
            "{remote_underlay}",
        ])),
        stop: Some(command(&[
            "/opt/vpsman-adapters/wg-runtime",
            "stop",
            "{interface}",
        ])),
        cleanup: Some(command(&[
            "/opt/vpsman-adapters/wg-runtime",
            "cleanup",
            "{interface}",
        ])),
        restart: None,
        status: command(&[
            "/opt/vpsman-adapters/wg-runtime",
            "status",
            "{interface}",
            "{local_client_id}",
            "{peer_client_id}",
        ]),
        traffic_limit_apply: None,
    }
}

fn plan(manager: RuntimeTunnelManager) -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "edge-link".to_string(),
        interface_name: "tunab".to_string(),
        kind: if manager == RuntimeTunnelManager::AgentIproute2Managed {
            TunnelKind::Gre
        } else {
            TunnelKind::Wireguard
        },
        runtime_control: RuntimeTunnelControl {
            manager,
            left_adapter_template_id: (manager == RuntimeTunnelManager::ExternalManagedAdapter)
                .then(|| LEFT_ADAPTER_ID.to_string()),
            right_adapter_template_id: (manager == RuntimeTunnelManager::ExternalManagedAdapter)
                .then(|| RIGHT_ADAPTER_ID.to_string()),
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: vpsman_common::RuntimeTunnelTopologyIntent::default(),
        left_client_id: "edge-a".to_string(),
        right_client_id: "edge-b".to_string(),
        left_remote_underlay: "203.0.113.20".to_string(),
        right_remote_underlay: "198.51.100.10".to_string(),
        left_local_underlay: Some("10.0.0.10".to_string()),
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
        ospf: None,
    })
    .unwrap()
}

#[test]
fn external_adapter_commands_render_only_declared_plan_values() {
    let plan = plan(RuntimeTunnelManager::ExternalManagedAdapter);
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let rendered =
        render_runtime_adapter_command(adapter().startup.as_ref().unwrap(), &plan, &endpoint)
            .unwrap();
    assert_eq!(rendered[0], "/opt/vpsman-adapters/wg-runtime");
    assert_eq!(rendered[2], "tunab");
    assert_eq!(rendered[3], "10.0.0.10");
    assert_eq!(rendered[4], "203.0.113.20");
}

#[test]
fn iproute2_tunnel_argv_uses_only_the_endpoint_declared_source_and_destination() {
    let plan = plan(RuntimeTunnelManager::AgentIproute2Managed);
    let left = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let right = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Right).unwrap();
    let base = vec!["/usr/sbin/ip".to_string()];

    let left_argv = build_ip_tunnel_argv(&base, "add", &plan, &left).unwrap();
    assert!(left_argv
        .windows(2)
        .any(|pair| pair == ["remote", "203.0.113.20"]));
    assert!(left_argv
        .windows(2)
        .any(|pair| pair == ["local", "10.0.0.10"]));

    let right_argv = build_ip_tunnel_argv(&base, "add", &plan, &right).unwrap();
    assert!(right_argv
        .windows(2)
        .any(|pair| pair == ["remote", "198.51.100.10"]));
    assert!(!right_argv.iter().any(|part| part == "local"));
}

#[test]
fn external_adapter_renders_all_declared_traffic_limit_values() {
    let mut plan = plan(RuntimeTunnelManager::ExternalManagedAdapter);
    plan.runtime_control.traffic_limit = RuntimeTunnelTrafficLimit {
        ingress_kbps: Some(10_000),
        egress_kbps: Some(20_000),
        burst_kb: Some(256),
    };
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let rendered = render_runtime_adapter_command(
        &command(&[
            "/opt/vpsman-adapters/wg-runtime",
            "limit",
            "{ingress_kbps}",
            "{egress_kbps}",
            "{burst_kb}",
        ]),
        &plan,
        &endpoint,
    )
    .unwrap();

    assert_eq!(rendered[2..], ["10000", "20000", "256"]);
}

#[test]
fn external_reconcile_uses_snapshot_lifecycle_then_status() {
    let plan = plan(RuntimeTunnelManager::ExternalManagedAdapter);
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let steps = build_external_adapter_steps(&plan, &endpoint, &adapter()).unwrap();
    let labels = steps.iter().map(|step| step.label).collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec!["runtime_adapter_startup", "runtime_adapter_status"]
    );
    assert!(steps.iter().all(|step| step.required));
}

#[test]
fn external_remove_uses_stored_snapshot_even_after_plan_is_omitted() {
    let plan = plan(RuntimeTunnelManager::ExternalManagedAdapter);
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let steps = build_external_adapter_remove_steps(&plan, &endpoint, &adapter()).unwrap();
    let labels = steps.iter().map(|step| step.label).collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec![
            "runtime_adapter_stop",
            "runtime_adapter_cleanup",
            "runtime_adapter_status"
        ]
    );
}

#[tokio::test]
async fn observed_plan_never_runs_mutating_commands() {
    let mut config = AgentConfig {
        client_id: "edge-a".to_string(),
        ..AgentConfig::default()
    };
    config.network.runtime_reconcile_enabled = true;
    config.network.apply_enabled = false;
    let mut observed = plan(RuntimeTunnelManager::ExternalObserved);
    observed.runtime_topology.stale_interfaces = vec!["must-not-delete".to_string()];
    observed.runtime_topology.stale_routes = vec![vpsman_common::RuntimeTunnelRoute {
        destination_cidr: "10.99.0.0/16".to_string(),
        ..Default::default()
    }];
    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &observed,
        runtime_adapter: None,
        side: TunnelEndpointSide::Left,
        max_timeout_secs: 10,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();
    assert_eq!(report["status"], "observed_only");
    assert_eq!(report["commands"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn observed_plan_removal_is_read_only_when_mutation_is_disabled() {
    let mut config = AgentConfig {
        client_id: "edge-a".to_string(),
        ..AgentConfig::default()
    };
    config.network.runtime_reconcile_enabled = true;
    config.network.apply_enabled = false;
    let report = execute_runtime_tunnel_remove_report_cancelable(
        NetworkRuntimeRemoveInput {
            config: &config,
            plan: &plan(RuntimeTunnelManager::ExternalObserved),
            runtime_adapter: None,
            side: TunnelEndpointSide::Left,
            max_timeout_secs: 10,
            effective_uid_override: Some(1000),
        },
        CommandCancelToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(report["status"], "observed_only");
    assert_eq!(report["commands"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn external_adapter_never_inherits_agent_topology_cleanup() {
    let mut config = AgentConfig {
        client_id: "edge-a".to_string(),
        ..AgentConfig::default()
    };
    config.network.runtime_reconcile_enabled = true;
    config.network.apply_enabled = true;
    config.network.runtime_ip_argv = vec!["/bin/false".to_string()];
    let mut external = plan(RuntimeTunnelManager::ExternalManagedAdapter);
    external.runtime_topology.stale_interfaces = vec!["must-not-delete".to_string()];
    external.runtime_topology.stale_routes = vec![vpsman_common::RuntimeTunnelRoute {
        destination_cidr: "10.99.0.0/16".to_string(),
        ..Default::default()
    }];
    let mut snapshot = adapter();
    snapshot.startup = Some(command(&["/bin/true"]));
    snapshot.status = command(&["/bin/true"]);
    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &external,
        runtime_adapter: Some(&snapshot),
        side: TunnelEndpointSide::Left,
        max_timeout_secs: 10,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();
    let labels = report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|command| command["label"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec!["runtime_adapter_startup", "runtime_adapter_status"]
    );
}

#[tokio::test]
async fn external_managed_reconcile_rejects_a_missing_snapshot() {
    let mut config = AgentConfig {
        client_id: "edge-a".to_string(),
        ..AgentConfig::default()
    };
    config.network.runtime_reconcile_enabled = true;
    config.network.apply_enabled = true;
    let error = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan(RuntimeTunnelManager::ExternalManagedAdapter),
        runtime_adapter: None,
        side: TunnelEndpointSide::Left,
        max_timeout_secs: 10,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap_err();
    assert!(error.to_string().contains("adapter snapshot is required"));
}
