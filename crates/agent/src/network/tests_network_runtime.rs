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
        definition_id: LEFT_ADAPTER_ID.to_string(),
        definition_name: "wireguard-runtime".to_string(),
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
        kind: if manager == RuntimeTunnelManager::AgentBuiltin {
            TunnelKind::Gre
        } else {
            TunnelKind::Wireguard
        },
        runtime_control: RuntimeTunnelControl {
            manager,
            left_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
                .then(|| LEFT_ADAPTER_ID.to_string()),
            right_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
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
        left_mtu: (manager == RuntimeTunnelManager::AgentBuiltin).then_some(1476),
        right_mtu: (manager == RuntimeTunnelManager::AgentBuiltin).then_some(1400),
        ospf: None,
    })
    .unwrap()
}

#[test]
fn custom_adapter_commands_render_only_declared_plan_values() {
    let mut plan = plan(RuntimeTunnelManager::CustomAdapter);
    plan.name = "edge-{kind}".to_string();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let rendered = render_runtime_adapter_command(
        &command(&[
            "/opt/vpsman-adapters/wg-runtime",
            "start",
            "{plan}",
            "{interface}",
            "{local_underlay}",
            "{remote_underlay}",
        ]),
        &plan,
        &endpoint,
    )
    .unwrap();
    assert_eq!(rendered[0], "/opt/vpsman-adapters/wg-runtime");
    assert_eq!(rendered[2], "edge-{kind}");
    assert_eq!(rendered[3], "tunab");
    assert_eq!(rendered[4], "10.0.0.10");
    assert_eq!(rendered[5], "203.0.113.20");
}

#[test]
fn iproute2_tunnel_argv_uses_only_the_endpoint_declared_source_and_destination() {
    let plan = plan(RuntimeTunnelManager::AgentBuiltin);
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
fn iproute2_reconcile_applies_the_local_endpoint_mtu() {
    let config = AgentConfig::default();
    let plan = plan(RuntimeTunnelManager::AgentBuiltin);
    let left = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let right = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Right).unwrap();

    let left_steps = build_iproute2_reconcile_steps(&config, &plan, &left, false).unwrap();
    let right_steps = build_iproute2_reconcile_steps(&config, &plan, &right, false).unwrap();
    let left_mtu = left_steps
        .iter()
        .find(|step| step.label == "runtime_link_mtu")
        .unwrap();
    let right_mtu = right_steps
        .iter()
        .find(|step| step.label == "runtime_link_mtu")
        .unwrap();

    assert_eq!(
        left_mtu.argv,
        ["/sbin/ip", "link", "set", "dev", "tunab", "mtu", "1476"]
    );
    assert_eq!(
        right_mtu.argv,
        ["/sbin/ip", "link", "set", "dev", "tunab", "mtu", "1400"]
    );
    assert!(left_mtu.required);
    assert!(right_mtu.required);
    let left_labels = left_steps.iter().map(|step| step.label).collect::<Vec<_>>();
    assert_eq!(
        &left_labels[1..4],
        [
            "runtime_tunnel_add",
            "runtime_link_mtu",
            "runtime_addr_replace"
        ]
    );
}

#[test]
fn iproute2_link_inspection_keeps_observed_mtu() {
    let link = parse_iproute2_link_json(
        r#"[{"ifname":"tunab","mtu":1476,"linkinfo":{"info_kind":"gre","info_data":{"local":"10.0.0.10","remote":"203.0.113.20","ttl":255}}}]"#,
        "tunab",
    )
    .unwrap();

    assert_eq!(link.mtu, Some(1476));
}

#[test]
fn custom_adapter_renders_all_declared_traffic_limit_values() {
    let mut plan = plan(RuntimeTunnelManager::CustomAdapter);
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
fn custom_adapter_reconcile_uses_snapshot_lifecycle_then_status() {
    let plan = plan(RuntimeTunnelManager::CustomAdapter);
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let steps = build_custom_adapter_steps(&plan, &endpoint, &adapter()).unwrap();
    let labels = steps.iter().map(|step| step.label).collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec!["runtime_adapter_startup", "runtime_adapter_status"]
    );
    assert!(steps.iter().all(|step| step.required));
}

#[test]
fn custom_adapter_remove_uses_stored_snapshot_even_after_plan_is_omitted() {
    let plan = plan(RuntimeTunnelManager::CustomAdapter);
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let steps = build_custom_adapter_remove_steps(&plan, &endpoint, &adapter()).unwrap();
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
        plan_id: None,
        plan: &observed,
        builtin_credentials: None,
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
            plan_id: None,
            plan: &plan(RuntimeTunnelManager::ExternalObserved),
            builtin_credentials: None,
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
async fn custom_adapter_never_inherits_agent_topology_cleanup() {
    let mut config = AgentConfig {
        client_id: "edge-a".to_string(),
        ..AgentConfig::default()
    };
    config.network.runtime_reconcile_enabled = true;
    config.network.apply_enabled = true;
    config.network.runtime_ip_argv = vec!["/bin/false".to_string()];
    let mut custom = plan(RuntimeTunnelManager::CustomAdapter);
    custom.runtime_topology.stale_interfaces = vec!["must-not-delete".to_string()];
    custom.runtime_topology.stale_routes = vec![vpsman_common::RuntimeTunnelRoute {
        destination_cidr: "10.99.0.0/16".to_string(),
        ..Default::default()
    }];
    let mut snapshot = adapter();
    snapshot.startup = Some(command(&["/bin/true"]));
    snapshot.status = command(&["/bin/true"]);
    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan_id: None,
        plan: &custom,
        builtin_credentials: None,
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
async fn custom_adapter_reconcile_rejects_a_missing_snapshot() {
    let mut config = AgentConfig {
        client_id: "edge-a".to_string(),
        ..AgentConfig::default()
    };
    config.network.runtime_reconcile_enabled = true;
    config.network.apply_enabled = true;
    let error = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan_id: None,
        plan: &plan(RuntimeTunnelManager::CustomAdapter),
        builtin_credentials: None,
        runtime_adapter: None,
        side: TunnelEndpointSide::Left,
        max_timeout_secs: 10,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap_err();
    assert!(error.to_string().contains("adapter snapshot is required"));
}

#[test]
fn builtin_failure_compensation_requires_proven_link_creation() {
    let config = AgentConfig::default();
    let plan = plan(RuntimeTunnelManager::AgentBuiltin);
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();

    let (unproven, reason) =
        build_runtime_compensation_steps(&config, &plan, &endpoint, None, false).unwrap();
    assert!(unproven.is_empty());
    assert_eq!(reason, Some("no_plan_owned_link_created"));

    let (created, reason) =
        build_runtime_compensation_steps(&config, &plan, &endpoint, None, true).unwrap();
    assert_eq!(reason, None);
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].label, "runtime_compensate_link_delete");
}

#[test]
fn builtin_traffic_limit_reconcile_clears_directions_that_are_now_unlimited() {
    let base = vec!["/sbin/tc".to_string()];
    let cleared =
        build_traffic_limit_steps(&base, "wg0", &RuntimeTunnelTrafficLimit::default()).unwrap();
    assert_eq!(
        cleared.iter().map(|step| step.label).collect::<Vec<_>>(),
        vec![
            "runtime_traffic_egress_clear",
            "runtime_traffic_ingress_clear"
        ]
    );
    assert!(cleared.iter().all(|step| step.required));

    let ingress_only = build_traffic_limit_steps(
        &base,
        "wg0",
        &RuntimeTunnelTrafficLimit {
            ingress_kbps: Some(10_000),
            egress_kbps: None,
            burst_kb: None,
        },
    )
    .unwrap();
    assert_eq!(ingress_only[0].label, "runtime_traffic_egress_clear");
    assert_eq!(ingress_only[1].label, "runtime_traffic_ingress_qdisc");
    assert_eq!(ingress_only[2].label, "runtime_traffic_ingress_filter");
}

#[test]
fn traffic_limit_clear_accepts_only_explicit_already_absent_evidence() {
    let mut absent = serde_json::json!({
        "success": false,
        "timed_out": false,
        "killed_for_output_limit": false,
        "stderr": {"text": "Error: Cannot find specified qdisc on specified device.\n"}
    });
    accept_idempotent_traffic_clear("runtime_traffic_ingress_clear", &mut absent);
    assert_eq!(absent["success"], true);
    assert_eq!(absent["reason"], "qdisc_already_absent");

    let mut denied = serde_json::json!({
        "success": false,
        "timed_out": false,
        "killed_for_output_limit": false,
        "stderr": {"text": "RTNETLINK answers: Operation not permitted\n"}
    });
    accept_idempotent_traffic_clear("runtime_traffic_egress_clear", &mut denied);
    assert_eq!(denied["success"], false);
    assert!(denied.get("reason").is_none());
}
