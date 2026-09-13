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
        dynamic_bandwidth: false,
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
fn iproute2_address_inspection_matches_real_peer_fields_without_relaxing_ownership() {
    for (local, peer, prefix, wrong_peer, family) in [
        ("10.255.0.0", "10.255.0.1", 31, "10.255.0.2", "inet"),
        ("fd00:ffff::", "fd00:ffff::1", 127, "fd00:ffff::2", "inet6"),
    ] {
        let observed = serde_json::json!([{
            "ifname": "tunab",
            "addr_info": [
                {"family": "inet6", "local": "fe80::1", "prefixlen": 64},
                {"family": family, "local": local, "address": peer, "prefixlen": prefix}
            ]
        }]);
        let addresses = parse_iproute2_addr_json(&observed.to_string(), "tunab").unwrap();
        assert!(matching_existing_iproute2_address(&addresses, local, peer, prefix).is_some());
        assert!(
            matching_existing_iproute2_address(&addresses, local, wrong_peer, prefix).is_none()
        );
        assert!(matching_existing_iproute2_address(&addresses, wrong_peer, peer, prefix).is_none());
        assert!(matching_existing_iproute2_address(&addresses, local, peer, prefix - 1).is_none());
    }
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
        previous_plan: None,
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
        previous_plan: None,
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
        previous_plan: None,
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
        build_runtime_compensation_steps(&config, &plan, &endpoint, None, false, false).unwrap();
    assert!(unproven.is_empty());
    assert_eq!(reason, Some("no_plan_owned_link_created"));

    let (created, reason) =
        build_runtime_compensation_steps(&config, &plan, &endpoint, None, true, false).unwrap();
    assert_eq!(reason, None);
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].label, "runtime_compensate_link_delete");
}

#[test]
fn builtin_compensation_ownership_matrix_deletes_only_new_resources() {
    let config = AgentConfig::default();
    let mut plan = plan(RuntimeTunnelManager::AgentBuiltin);
    plan.kind = TunnelKind::Fou;
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    for (created_link, created_listener) in
        [(false, false), (true, false), (false, true), (true, true)]
    {
        let (steps, reason) = build_runtime_compensation_steps(
            &config,
            &plan,
            &endpoint,
            None,
            created_link,
            created_listener,
        )
        .unwrap();
        assert_eq!(
            steps.len(),
            usize::from(created_link) + usize::from(created_listener)
        );
        assert_eq!(reason.is_none(), created_link || created_listener);
        assert_eq!(
            steps
                .iter()
                .any(|step| step.argv.windows(2).any(|pair| pair == ["link", "delete"])),
            created_link
        );
        assert_eq!(
            steps
                .iter()
                .any(|step| step.argv.windows(2).any(|pair| pair == ["fou", "del"])),
            created_listener
        );
    }
}

#[test]
fn fou_listener_idempotency_requires_exact_compatible_kernel_evidence() {
    let mut plan = plan(RuntimeTunnelManager::AgentBuiltin);
    plan.kind = TunnelKind::Fou;
    let valid = serde_json::json!({
        "port": plan.runtime_control.fou.port,
        "ipproto": plan.runtime_control.fou.ipproto,
        "family": "inet"
    });
    let report = |listener: serde_json::Value| {
        serde_json::json!({
            "success": true, "stdout": {"text": serde_json::json!([listener]).to_string()}
        })
    };
    assert!(fou_listener_matches(&plan, &report(valid.clone())));
    let mut no_family = valid.clone();
    no_family.as_object_mut().unwrap().remove("family");
    assert!(fou_listener_matches(&plan, &report(no_family)));
    for (field, value) in [
        ("port", serde_json::json!(0)),
        ("ipproto", serde_json::json!(255)),
        ("family", serde_json::json!("inet6")),
        ("gue", serde_json::json!(true)),
        ("local", serde_json::json!("192.0.2.1")),
        ("peer", serde_json::json!("192.0.2.2")),
        ("peer_port", serde_json::json!(12345)),
        ("dev", serde_json::json!("eth0")),
    ] {
        let mut incompatible = valid.clone();
        incompatible[field] = value;
        assert!(
            !fou_listener_matches(&plan, &report(incompatible)),
            "accepted {field}"
        );
    }
    for evidence in [
        serde_json::json!({"success": true, "stdout": {"text": "not JSON"}}),
        serde_json::json!({"success": true, "stdout": {"text": "[]"}}),
        serde_json::json!({"success": true, "stdout": {"text": valid.to_string()}}),
    ] {
        assert!(!fou_listener_matches(&plan, &evidence));
    }
}

fn hooked_builtin_plan() -> TunnelPlan {
    let mut plan = plan(RuntimeTunnelManager::AgentBuiltin);
    plan.runtime_control.hooks.left = vpsman_common::RuntimeTunnelEndpointHooks {
        pre_start: Some(command(&["/bin/echo", "pre-start:{interface}"])),
        post_start: Some(command(&["/bin/echo", "post-start:{interface}"])),
        pre_shutdown: Some(command(&["/bin/echo", "pre-shutdown:{interface}"])),
        post_shutdown: Some(command(&["/bin/echo", "post-shutdown:{interface}"])),
    };
    plan
}

fn builtin_hook_test_config() -> AgentConfig {
    let mut config = AgentConfig {
        client_id: "edge-a".to_string(),
        ..AgentConfig::default()
    };
    config.network.apply_enabled = true;
    config.network.runtime_reconcile_enabled = true;
    config.network.root_dir = format!(".tmp/runtime-hooks-{}", uuid::Uuid::new_v4());
    config.network.runtime_ip_argv = vec!["/bin/true".to_string()];
    config.network.runtime_tc_argv = vec!["/bin/true".to_string()];
    config
}

async fn reconcile_hook_test(config: &AgentConfig, plan: &TunnelPlan) -> serde_json::Value {
    execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config,
        plan_id: None,
        plan,
        previous_plan: None,
        builtin_credentials: None,
        runtime_adapter: None,
        side: TunnelEndpointSide::Left,
        max_timeout_secs: 10,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap()
}

async fn remove_hook_test(config: &AgentConfig, plan: &TunnelPlan) -> serde_json::Value {
    execute_runtime_tunnel_remove_report_cancelable(
        NetworkRuntimeRemoveInput {
            config,
            plan_id: None,
            plan,
            builtin_credentials: None,
            runtime_adapter: None,
            side: TunnelEndpointSide::Left,
            max_timeout_secs: 10,
            effective_uid_override: Some(0),
        },
        CommandCancelToken::default(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn builtin_start_hooks_surround_native_configuration_and_expand_arguments() {
    let config = builtin_hook_test_config();
    let plan = hooked_builtin_plan();
    let report = reconcile_hook_test(&config, &plan).await;
    assert_eq!(report["status"], "converged");
    let commands = report["commands"].as_array().unwrap();
    assert_eq!(commands.first().unwrap()["label"], "runtime_hook_pre_start");
    assert_eq!(commands.last().unwrap()["label"], "runtime_hook_post_start");
    assert_eq!(
        commands.first().unwrap()["stdout"]["text"],
        "pre-start:tunab\n"
    );
    assert!(commands
        .iter()
        .any(|item| item["label"] == "runtime_tunnel_add"));
    assert_eq!(report["hook_failures"], 0);
}

#[tokio::test]
async fn builtin_pre_start_failure_does_not_start_or_compensate_and_disabled_apply_skips_hooks() {
    let mut config = builtin_hook_test_config();
    let mut plan = hooked_builtin_plan();
    plan.runtime_control.hooks.left.pre_start = Some(command(&["/bin/false"]));
    let report = reconcile_hook_test(&config, &plan).await;
    assert_eq!(report["status"], "failed");
    assert_eq!(report["commands"].as_array().unwrap().len(), 1);
    assert_eq!(report["commands"][0]["label"], "runtime_hook_pre_start");
    assert!(report["compensation"].is_null());
    config.network.apply_enabled = false;
    let skipped = reconcile_hook_test(&config, &plan).await;
    assert_eq!(skipped["status"], "skipped");
    assert!(skipped["commands"].is_null());
}

#[tokio::test]
async fn builtin_post_start_failure_preserves_native_convergence_without_compensation() {
    let config = builtin_hook_test_config();
    let mut plan = hooked_builtin_plan();
    plan.runtime_control.hooks.left.post_start = Some(command(&["/bin/false"]));
    let report = reconcile_hook_test(&config, &plan).await;
    assert_eq!(report["status"], "converged");
    assert_eq!(report["hook_failures"], 1);
    assert!(report["compensation"].is_null());
    assert_eq!(
        report["commands"].as_array().unwrap().last().unwrap()["success"],
        false
    );
}

#[tokio::test]
async fn builtin_intact_tunnel_reconcile_does_not_replay_hooks() {
    let mut config = builtin_hook_test_config();
    let plan = hooked_builtin_plan();
    let root = Path::new(&config.network.root_dir);
    tokio::fs::create_dir_all(root.join("sys/class/net/tunab"))
        .await
        .unwrap();
    config.network.runtime_ip_argv = vec!["/bin/sh".to_string(), "-c".to_string(),
        r#"case "$3" in link) printf '%s' '{"mtu":1476,"linkinfo":{"info_kind":"gre","info_data":{"local":"10.0.0.10","remote":"203.0.113.20","ttl":255}}}' ;; addr) printf '%s' '{"addr_info":[{"local":"10.255.0.0","peer":"10.255.0.1","prefixlen":31}]}' ;; esac"#.to_string(), "runtime-native".to_string()];
    let report = reconcile_hook_test(&config, &plan).await;
    assert_eq!(report["status"], "converged");
    assert_eq!(
        hook_failure_count(report["commands"].as_array().unwrap()),
        0
    );
    assert!(!report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["label"].as_str().unwrap().starts_with("runtime_hook_")));
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn builtin_shutdown_hooks_abort_before_native_teardown_but_post_failure_is_removed() {
    let config = builtin_hook_test_config();
    let mut plan = hooked_builtin_plan();
    let root = Path::new(&config.network.root_dir);
    tokio::fs::create_dir_all(root.join("sys/class/net/tunab"))
        .await
        .unwrap();
    plan.runtime_control.hooks.left.pre_shutdown = Some(command(&["/bin/false"]));
    let blocked = remove_hook_test(&config, &plan).await;
    assert_eq!(blocked["status"], "failed");
    assert_eq!(blocked["commands"].as_array().unwrap().len(), 1);
    plan.runtime_control.hooks.left.pre_shutdown = Some(command(&["/bin/true"]));
    plan.runtime_control.hooks.left.post_shutdown = Some(command(&["/bin/false"]));
    let removed = remove_hook_test(&config, &plan).await;
    assert_eq!(removed["status"], "removed");
    assert_eq!(removed["hook_failures"], 1);
    let commands = removed["commands"].as_array().unwrap();
    assert_eq!(
        commands.first().unwrap()["label"],
        "runtime_hook_pre_shutdown"
    );
    assert_eq!(
        commands.last().unwrap()["label"],
        "runtime_hook_post_shutdown"
    );
    tokio::fs::remove_dir_all(root).await.unwrap();
    let absent = remove_hook_test(&config, &plan).await;
    assert_eq!(absent["status"], "removed");
    assert!(!absent["commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["label"].as_str().unwrap().starts_with("runtime_hook_")));
}

#[tokio::test]
async fn lifecycle_hook_execution_errors_are_retained_as_failure_evidence() {
    let plan = hooked_builtin_plan();
    let mut reports = Vec::new();
    let invalid = command(&["/path/that/does/not/exist"]);
    assert!(
        !run_lifecycle_hook(
            Some(&invalid),
            "runtime_hook_post_start",
            &plan,
            TunnelEndpointSide::Left,
            &mut reports,
            CommandCancelToken::default()
        )
        .await
    );
    assert_eq!(reports[0]["success"], false);
    assert!(reports[0]["error"]
        .as_str()
        .unwrap()
        .contains("failed to run runtime tunnel command"));
}

#[tokio::test]
async fn openvpn_remove_rejects_missing_owned_pid_before_hooks_or_native_mutation() {
    let mut config = builtin_hook_test_config();
    let root = std::path::PathBuf::from(&config.network.root_dir);
    let link_path = root.join("sys/class/net/tunab");
    tokio::fs::create_dir_all(&link_path).await.unwrap();
    let pre_marker = root.join("pre-shutdown-called");
    let post_marker = root.join("post-shutdown-called");
    let native_marker = root.join("native-called");
    let marker_command = |path: &Path| {
        command(&[
            "/bin/sh",
            "-c",
            "printf called > \"$1\"",
            "ownership-test-marker",
            path.to_str().unwrap(),
        ])
    };
    let mut plan = hooked_builtin_plan();
    plan.kind = TunnelKind::Openvpn;
    plan.runtime_control.hooks.left.pre_shutdown = Some(marker_command(&pre_marker));
    plan.runtime_control.hooks.left.post_shutdown = Some(marker_command(&post_marker));
    config.network.runtime_ip_argv = marker_command(&native_marker).argv;
    config.network.runtime_tc_argv = marker_command(&native_marker).argv;
    config.network.runtime_openvpn_argv = marker_command(&native_marker).argv;
    let plan_id = uuid::Uuid::new_v4().to_string();
    let pid_path = crate::state_dir::agent_state_dir()
        .unwrap()
        .join("network-tunnels")
        .join(&plan_id)
        .join("left/openvpn.pid");
    assert!(!pid_path.exists());

    let error = execute_runtime_tunnel_remove_report_cancelable(
        NetworkRuntimeRemoveInput {
            config: &config,
            plan_id: Some(&plan_id),
            plan: &plan,
            builtin_credentials: None,
            runtime_adapter: None,
            side: TunnelEndpointSide::Left,
            max_timeout_secs: 10,
            effective_uid_override: Some(0),
        },
        CommandCancelToken::default(),
    )
    .await
    .unwrap_err();
    let pre_called = pre_marker.exists();
    let post_called = post_marker.exists();
    let native_called = native_marker.exists();
    let link_preserved = link_path.exists();
    tokio::fs::remove_dir_all(&root).await.unwrap();

    assert_eq!(
        error.to_string(),
        "OpenVPN interface exists without an owned plan process"
    );
    assert!(!pre_called, "ownership rejection must precede pre_shutdown");
    assert!(
        !post_called,
        "ownership rejection must not run post_shutdown"
    );
    assert!(
        !native_called,
        "ownership rejection must not mutate native state"
    );
    assert!(link_preserved, "the unowned interface must remain intact");
}

#[tokio::test]
async fn openvpn_restart_uses_previous_shutdown_hook_and_keeps_old_files_on_abort() {
    let mut config = builtin_hook_test_config();
    config.network.runtime_openvpn_argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "printf 'OpenVPN 2.6.12 test\\n'".to_string(),
    ];
    let mut previous = hooked_builtin_plan();
    previous.kind = TunnelKind::Openvpn;
    previous.runtime_control.hooks.left.pre_shutdown =
        Some(command(&["/bin/sh", "-c", "printf old-shutdown; exit 1"]));
    let mut desired = previous.clone();
    desired.runtime_control.hooks.left.pre_shutdown = Some(command(&["/bin/true"]));
    desired.runtime_control.openvpn.left_config_override = Some("verb 4".to_string());
    let plan_id = uuid::Uuid::new_v4().to_string();
    let credentials = vpsman_common::TunnelEndpointBuiltinCredentials::Openvpn {
        generation: 1,
        local_private_key_pem: "old-key".to_string(),
        local_certificate_pem: "old-certificate".to_string(),
        peer_issuer_certificate_pem: "old-peer-ca".to_string(),
        peer_certificate_sha256_fingerprint: "old-fingerprint".to_string(),
    };
    let endpoint = render_tunnel_endpoint_config(&previous, TunnelEndpointSide::Left).unwrap();
    let prepared = openvpn::prepare_openvpn_state(
        Some(&plan_id),
        &previous,
        &endpoint,
        Some(&credentials),
        &semver::Version::new(2, 6, 12),
    )
    .await
    .unwrap();
    openvpn::write_openvpn_state(&prepared).await.unwrap();
    let old_config = tokio::fs::read(&prepared.config_path).await.unwrap();
    tokio::fs::write(
        prepared.endpoint_dir.join("applied.sha256"),
        &prepared.config_hash,
    )
    .await
    .unwrap();
    // No real daemon is needed: the pre-hook must abort before any PID signal.
    // This out-of-range Linux PID also cannot target another running process.
    let pid = "2147483647";
    tokio::fs::write(&prepared.pid_path, pid).await.unwrap();
    let root = Path::new(&config.network.root_dir);
    tokio::fs::create_dir_all(root.join("proc").join(pid))
        .await
        .unwrap();
    tokio::fs::create_dir_all(root.join("sys/class/net/tunab"))
        .await
        .unwrap();
    tokio::fs::write(
        root.join("proc").join(pid).join("cmdline"),
        format!("openvpn\0--config\0{}\0", prepared.config_path.display()),
    )
    .await
    .unwrap();
    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan_id: Some(&plan_id),
        plan: &desired,
        previous_plan: Some(&previous),
        builtin_credentials: Some(&credentials),
        runtime_adapter: None,
        side: TunnelEndpointSide::Left,
        max_timeout_secs: 10,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();
    assert_eq!(report["reason"], "lifecycle_pre_hook_failed");
    let hook = report["commands"].as_array().unwrap().last().unwrap();
    assert_eq!(hook["label"], "runtime_hook_pre_shutdown");
    assert_eq!(hook["stdout"]["text"], "old-shutdown");
    assert_eq!(
        tokio::fs::read(&prepared.config_path).await.unwrap(),
        old_config
    );
    assert_eq!(
        tokio::fs::read_to_string(prepared.endpoint_dir.join("openvpn.key"))
            .await
            .unwrap(),
        "old-key"
    );
    assert!(root.join("sys/class/net/tunab").exists());
    openvpn::cleanup_openvpn_state(Some(&plan_id), TunnelEndpointSide::Left)
        .await
        .unwrap();
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn openvpn_restart_runs_old_shutdown_then_new_start_and_does_not_replay_hook_only_edits() {
    let mut config = builtin_hook_test_config();
    let root = std::path::PathBuf::from(&config.network.root_dir);
    // The replacement driver publishes the same PID/config/link evidence as a
    // daemon, without requiring OpenVPN privileges or a second network host.
    config.network.runtime_openvpn_argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        r#"if [ "$2" = --version ]; then printf 'OpenVPN 2.6.12 test\n'; exit; fi
if [ "$2" = --config ]; then
  mkdir -p "$1/sys/class/net/tunab" "$1/proc/2147483647"
  printf 'openvpn\000--config\000%s\000' "$3" > "$1/proc/2147483647/cmdline"
  printf 2147483647 > "${3%/*}/openvpn.pid"
fi"#
        .to_string(),
        "openvpn-test-driver".to_string(),
        root.to_string_lossy().to_string(),
    ];
    let mut previous = hooked_builtin_plan();
    previous.kind = TunnelKind::Openvpn;
    previous.runtime_control.hooks.left.pre_shutdown =
        Some(command(&["/bin/echo", "old-pre-shutdown"]));
    previous.runtime_control.hooks.left.post_shutdown = Some(command(&[
        "/bin/sh",
        "-c",
        "printf old-post-shutdown; exit 1",
    ]));
    let mut desired = previous.clone();
    desired.runtime_control.openvpn.left_config_override = Some("verb 4".to_string());
    desired.runtime_control.hooks.left.pre_shutdown = Some(command(&["/bin/false"]));
    desired.runtime_control.hooks.left.pre_start = Some(command(&["/bin/echo", "new-pre-start"]));
    desired.runtime_control.hooks.left.post_start = Some(command(&["/bin/echo", "new-post-start"]));
    let plan_id = uuid::Uuid::new_v4().to_string();
    let credentials = vpsman_common::TunnelEndpointBuiltinCredentials::Openvpn {
        generation: 1,
        local_private_key_pem: "key".to_string(),
        local_certificate_pem: "certificate".to_string(),
        peer_issuer_certificate_pem: "peer-ca".to_string(),
        peer_certificate_sha256_fingerprint: "fingerprint".to_string(),
    };
    let endpoint = render_tunnel_endpoint_config(&previous, TunnelEndpointSide::Left).unwrap();
    let prepared = openvpn::prepare_openvpn_state(
        Some(&plan_id),
        &previous,
        &endpoint,
        Some(&credentials),
        &semver::Version::new(2, 6, 12),
    )
    .await
    .unwrap();
    openvpn::write_openvpn_state(&prepared).await.unwrap();
    tokio::fs::write(
        prepared.endpoint_dir.join("applied.sha256"),
        &prepared.config_hash,
    )
    .await
    .unwrap();
    let mut daemon = tokio::process::Command::new("/bin/sleep")
        .arg("60")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = daemon.id().unwrap();
    tokio::fs::write(&prepared.pid_path, pid.to_string())
        .await
        .unwrap();
    tokio::fs::create_dir_all(root.join("proc").join(pid.to_string()))
        .await
        .unwrap();
    tokio::fs::create_dir_all(root.join("sys/class/net/tunab"))
        .await
        .unwrap();
    tokio::fs::write(
        root.join("proc").join(pid.to_string()).join("cmdline"),
        format!("openvpn\0--config\0{}\0", prepared.config_path.display()),
    )
    .await
    .unwrap();
    let link_path = root.join("sys/class/net/tunab");
    let reaper = tokio::spawn(async move {
        let status = daemon.wait().await.unwrap();
        tokio::fs::remove_dir(link_path).await.unwrap();
        status
    });
    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan_id: Some(&plan_id),
        plan: &desired,
        previous_plan: Some(&previous),
        builtin_credentials: Some(&credentials),
        runtime_adapter: None,
        side: TunnelEndpointSide::Left,
        max_timeout_secs: 10,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();
    assert_eq!(report["status"], "converged", "{report}");
    assert_eq!(report["hook_failures"], 1);
    assert!(!reaper.await.unwrap().success());
    let hook_outputs = report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["label"].as_str().unwrap().starts_with("runtime_hook_"))
        .map(|item| item["stdout"]["text"].as_str().unwrap().trim().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        hook_outputs,
        [
            "old-pre-shutdown",
            "old-post-shutdown",
            "new-pre-start",
            "new-post-start"
        ]
    );
    assert!(report["compensation"].is_null());
    assert!(tokio::fs::read_to_string(&prepared.config_path)
        .await
        .unwrap()
        .lines()
        .any(|line| line == "verb 4"));
    let mut hook_only = desired.clone();
    hook_only.runtime_control.hooks.left.pre_start = Some(command(&["/bin/false"]));
    hook_only.runtime_control.hooks.left.post_start = Some(command(&["/bin/false"]));
    let no_restart = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan_id: Some(&plan_id),
        plan: &hook_only,
        previous_plan: Some(&desired),
        builtin_credentials: Some(&credentials),
        runtime_adapter: None,
        side: TunnelEndpointSide::Left,
        max_timeout_secs: 10,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();
    assert_eq!(no_restart["status"], "converged", "{no_restart}");
    assert_eq!(
        no_restart["existing_link_validation"]["config_hash_matches"],
        true
    );
    assert!(!no_restart["commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["label"] == "runtime_openvpn_start"
            || item["label"].as_str().unwrap().starts_with("runtime_hook_")));
    openvpn::cleanup_openvpn_state(Some(&plan_id), TunnelEndpointSide::Left)
        .await
        .unwrap();
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[test]
fn configured_hook_budgets_extend_native_budget_without_changing_default() {
    let plain = plan(RuntimeTunnelManager::AgentBuiltin);
    assert_eq!(
        runtime_transition_timeout(10, &plain, None, TunnelEndpointSide::Left, false),
        10
    );
    let hooked = hooked_builtin_plan();
    assert_eq!(
        runtime_transition_timeout(10, &hooked, Some(&plain), TunnelEndpointSide::Left, false),
        30
    );
    assert_eq!(
        runtime_transition_timeout(10, &hooked, None, TunnelEndpointSide::Left, true),
        30
    );
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

// These tests exercise the actual reconciler, real iproute2 and real OpenVPN.
// Run only in a disposable Docker container with private network/PID namespaces,
// NET_ADMIN, iproute2, OpenVPN, OpenSSL and /dev/net/tun. Never use host networking.
#[cfg(target_os = "linux")]
mod isolated_linux_failures {
    use super::*;
    use std::{path::PathBuf, process::Output};
    use vpsman_common::TunnelEndpointBuiltinCredentials;

    fn require_isolated_network() {
        assert_eq!(
            std::env::var("VPSMAN_TEST_ISOLATED_NETWORK").as_deref(),
            Ok("1"),
            "set VPSMAN_TEST_ISOLATED_NETWORK=1 only in a disposable private-network container"
        );
        assert!(
            Path::new("/.dockerenv").exists(),
            "Docker isolation is required"
        );
        assert_eq!(
            unsafe { libc::geteuid() },
            0,
            "container NET_ADMIN is required"
        );
    }

    async fn native(program: &str, args: &[&str]) -> Output {
        // Each helper is bounded separately; the full reconcile has a 30s budget.
        tokio::time::timeout(
            Duration::from_secs(10),
            tokio::process::Command::new(program)
                .args(args)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("native test helper timed out")
        .expect("native test helper executable is required")
    }

    async fn checked_native(program: &str, args: &[&str]) -> Output {
        let output = native(program, args).await;
        assert!(
            output.status.success(),
            "{program} {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn unique_interface() -> String {
        // Linux interface names have a 15-byte limit; 4 + 8 stays below it.
        format!("vpsf{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
    }

    fn available_udp_port() -> u16 {
        // Ask this private namespace for a free port instead of sharing a fixed one.
        std::net::UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    struct PreservedResources {
        interface: String,
        link_before: Vec<u8>,
        process: tokio::process::Child,
    }

    impl PreservedResources {
        async fn new() -> Self {
            require_isolated_network();
            let interface = unique_interface();
            checked_native("/sbin/ip", &["link", "add", &interface, "type", "dummy"]).await;
            let link_before =
                checked_native("/sbin/ip", &["-j", "-d", "link", "show", "dev", &interface])
                    .await
                    .stdout;
            let process = tokio::process::Command::new("/bin/sleep")
                // Outlast the bounded reconcile; kill_on_drop also covers assertions.
                .arg("120")
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            Self {
                interface,
                link_before,
                process,
            }
        }

        async fn assert_preserved(&mut self) {
            assert_eq!(
                checked_native(
                    "/sbin/ip",
                    &["-j", "-d", "link", "show", "dev", &self.interface]
                )
                .await
                .stdout,
                self.link_before,
                "failed apply changed a pre-existing unrelated interface"
            );
            assert!(
                self.process.try_wait().unwrap().is_none(),
                "failed apply killed an unrelated process"
            );
        }
    }

    impl Drop for PreservedResources {
        fn drop(&mut self) {
            // This exact randomly named link was created by this fixture.
            let _ = std::process::Command::new("/sbin/ip")
                .args(["link", "delete", "dev", &self.interface])
                .output();
        }
    }

    fn config() -> AgentConfig {
        let mut config = AgentConfig {
            client_id: "edge-a".to_string(),
            ..Default::default()
        };
        config.network.apply_enabled = true;
        config.network.runtime_reconcile_enabled = true;
        // Use this container's real /proc and /sys for ownership and link evidence.
        config.network.root_dir = "/".to_string();
        // Keep the production defaults: /sbin/ip, /sbin/tc and /usr/sbin/openvpn.
        // Command rendering requires absolute executables, including in fixtures.
        config.network.runtime_command_timeout_secs = 5;
        config
    }

    fn isolated_plan(kind: TunnelKind) -> TunnelPlan {
        let mut plan = plan(RuntimeTunnelManager::AgentBuiltin);
        plan.kind = kind;
        plan.interface_name = unique_interface();
        if kind == TunnelKind::Openvpn {
            // A local listener needs no external peer to create its TUN device.
            plan.left_local_underlay = Some("127.0.0.1".to_string());
            plan.runtime_control.openvpn.port = available_udp_port();
        } else if kind == TunnelKind::Sit {
            // SIT carries IPv6 inside IPv4, so use an IPv6 inner-address fixture.
            plan.ipv4_tunnel = None;
            plan.ipv6_tunnel = Some(TunnelAddressPair {
                left: "fd00:ffff::".to_string(),
                right: "fd00:ffff::1".to_string(),
                prefix_len: 127,
            });
            plan.left_tunnel_address = "fd00:ffff::".to_string();
            plan.right_tunnel_address = "fd00:ffff::1".to_string();
            plan.tunnel_prefix_len = 127;
            plan.latency_primary_family = TunnelAddressFamily::Ipv6;
        }
        plan
    }

    async fn reconcile(
        config: &AgentConfig,
        plan: &TunnelPlan,
        plan_id: Option<&str>,
        credentials: Option<&TunnelEndpointBuiltinCredentials>,
    ) -> Result<serde_json::Value> {
        execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
            config,
            plan_id,
            plan,
            previous_plan: None,
            builtin_credentials: credentials,
            runtime_adapter: None,
            side: TunnelEndpointSide::Left,
            max_timeout_secs: 30,
            effective_uid_override: None,
        })
        .await
    }

    fn report_step<'a>(report: &'a serde_json::Value, label: &str) -> &'a serde_json::Value {
        report["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["label"] == label)
            .unwrap_or_else(|| panic!("missing {label}: {report}"))
    }

    async fn assert_link_absent(plan: &TunnelPlan) {
        assert!(!runtime_link_exists(Path::new("/"), &plan.interface_name).await);
        assert!(
            !native("/sbin/ip", &["link", "show", "dev", &plan.interface_name])
                .await
                .status
                .success()
        );
    }

    fn fail_mtu_argv() -> Vec<String> {
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            r#"if [ "$1" = link ] && [ "$2" = set ] && [ "$5" = mtu ]; then
  printf 'injected MTU apply failure\n' >&2; exit 23
fi
exec /sbin/ip "$@""#
                .to_string(),
            "injected-ip-mtu-failure".to_string(),
        ]
    }

    #[tokio::test]
    #[ignore = "requires disposable Docker private networking, naturally unavailable kernel FOU, NET_ADMIN and VPSMAN_TEST_ISOLATED_NETWORK=1"]
    async fn fou_actual_missing_kernel_capability_fails_without_leaking_resources() {
        let mut preserved = PreservedResources::new().await;
        let plan = isolated_plan(TunnelKind::Fou);
        let before = native("/sbin/ip", &["fou", "show"]).await;
        // Containers share the host kernel. Assert the observed environment
        // assumption instead of unloading modules or silently skipping coverage.
        assert!(
            !before.status.success(),
            "this test requires naturally unavailable kernel FOU"
        );
        let kernel_error = String::from_utf8_lossy(&before.stderr);
        assert!(
            kernel_error.contains("No such file or directory")
                || kernel_error.contains("Operation not supported"),
            "expected absent FOU support, not a generic permission/tool failure: {kernel_error}"
        );
        let report = reconcile(&config(), &plan, None, None).await.unwrap();
        assert_eq!(report["status"], "failed", "{report}");
        assert_eq!(report_step(&report, "runtime_fou_add")["success"], false);
        assert!(!report["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["label"] == "runtime_tunnel_add"));
        assert_link_absent(&plan).await;
        let after = native("/sbin/ip", &["fou", "show"]).await;
        assert_eq!(after.status.code(), before.status.code());
        assert_eq!(after.stdout, before.stdout, "FOU listener state changed");
        preserved.assert_preserved().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable Docker private networking, GRE/IPIP/SIT, NET_ADMIN and VPSMAN_TEST_ISOLATED_NETWORK=1"]
    async fn iproute2_builtin_apply_failure_ownership_matrix() {
        let mut preserved = PreservedResources::new().await;
        let mut config = config();
        config.network.runtime_ip_argv = fail_mtu_argv();
        for kind in [TunnelKind::Gre, TunnelKind::Ipip, TunnelKind::Sit] {
            let plan = isolated_plan(kind);
            let report = reconcile(&config, &plan, None, None).await.unwrap();
            assert_eq!(report["status"], "failed", "{kind:?}: {report}");
            assert_eq!(report_step(&report, "runtime_tunnel_add")["success"], true);
            assert_eq!(report_step(&report, "runtime_link_mtu")["success"], false);
            assert_eq!(report["compensation"]["status"], "completed", "{report}");
            assert_link_absent(&plan).await;
            preserved.assert_preserved().await;

            // Retry through the same reconciler with the failed command restored.
            // Its rendered addresses must pass the next attempt's ownership check.
            let mut native_config = config.clone();
            native_config.network.runtime_ip_argv = AgentConfig::default().network.runtime_ip_argv;
            let retried = reconcile(&native_config, &plan, None, None).await.unwrap();
            assert_eq!(retried["status"], "converged", "{kind:?}: {retried}");
            let before = checked_native(
                "/sbin/ip",
                &["-j", "-d", "link", "show", "dev", &plan.interface_name],
            )
            .await
            .stdout;
            let report = reconcile(&config, &plan, None, None).await.unwrap();
            assert_eq!(report["status"], "failed", "{kind:?}: {report}");
            assert_eq!(report_step(&report, "runtime_link_mtu")["success"], false);
            assert_eq!(
                report["compensation"]["status"], "not_available",
                "{report}"
            );
            assert_eq!(
                checked_native(
                    "/sbin/ip",
                    &["-j", "-d", "link", "show", "dev", &plan.interface_name]
                )
                .await
                .stdout,
                before,
                "failed apply must preserve a validated pre-existing {kind:?} link"
            );
            preserved.assert_preserved().await;
            checked_native("/sbin/ip", &["link", "delete", "dev", &plan.interface_name]).await;
        }
    }

    fn invalid_credentials() -> TunnelEndpointBuiltinCredentials {
        TunnelEndpointBuiltinCredentials::Openvpn {
            generation: 1,
            local_private_key_pem: "deliberately invalid key".to_string(),
            local_certificate_pem: "deliberately invalid certificate".to_string(),
            peer_issuer_certificate_pem: "deliberately invalid CA".to_string(),
            peer_certificate_sha256_fingerprint: "00".repeat(32),
        }
    }

    fn endpoint_dir(plan_id: &str) -> PathBuf {
        crate::state_dir::agent_state_dir()
            .unwrap()
            .join("network-tunnels")
            .join(plan_id)
            .join("left")
    }

    fn assert_no_owned_process(plan_id: &str) {
        let expected = endpoint_dir(plan_id).join("openvpn.conf");
        let expected = expected.to_string_lossy();
        for entry in std::fs::read_dir("/proc").unwrap().flatten() {
            if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
                continue;
            }
            let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
                continue;
            };
            let args = cmdline.split(|byte| *byte == 0).collect::<Vec<_>>();
            assert!(
                !args
                    .windows(2)
                    .any(|pair| pair[0] == b"--config" && pair[1] == expected.as_bytes()),
                "failed apply leaked an OpenVPN process for {}",
                expected
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires disposable Docker private networking, NET_ADMIN and VPSMAN_TEST_ISOLATED_NETWORK=1"]
    async fn openvpn_missing_executable_preserves_existing_interface_and_process() {
        let mut preserved = PreservedResources::new().await;
        let mut plan = isolated_plan(TunnelKind::Openvpn);
        // Even a same-name pre-existing link must be untouched by preflight failure.
        plan.interface_name = preserved.interface.clone();
        let plan_id = uuid::Uuid::new_v4().to_string();
        let mut config = config();
        config.network.runtime_openvpn_argv = vec![std::env::current_dir()
            .unwrap()
            .join(format!(".missing-openvpn-{plan_id}"))
            .to_str()
            .unwrap()
            .to_string()];
        let error = reconcile(&config, &plan, Some(&plan_id), Some(&invalid_credentials()))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("runtime_openvpn_version")
                && error.to_string().contains("No such file"),
            "{error:#}"
        );
        assert!(
            !endpoint_dir(&plan_id).exists(),
            "preflight failure wrote runtime state"
        );
        assert_no_owned_process(&plan_id);
        preserved.assert_preserved().await;
    }

    #[tokio::test]
    #[ignore = "requires disposable Docker private networking, OpenVPN, NET_ADMIN and VPSMAN_TEST_ISOLATED_NETWORK=1"]
    async fn openvpn_real_invalid_credentials_startup_failure_leaks_no_process_or_link() {
        let mut preserved = PreservedResources::new().await;
        let plan = isolated_plan(TunnelKind::Openvpn);
        let plan_id = uuid::Uuid::new_v4().to_string();
        let report = reconcile(
            &config(),
            &plan,
            Some(&plan_id),
            Some(&invalid_credentials()),
        )
        .await
        .unwrap();
        assert_eq!(report["status"], "failed", "{report}");
        assert_eq!(
            report_step(&report, "runtime_openvpn_version")["success"],
            true
        );
        // OpenVPN daemonizes before loading TLS credentials: its launcher can
        // succeed while asynchronous daemon initialization fails readiness.
        assert_eq!(
            report_step(&report, "runtime_openvpn_start")["success"],
            true
        );
        assert_eq!(
            report_step(&report, "runtime_openvpn_interface_ready")["success"],
            false
        );
        assert_link_absent(&plan).await;
        assert_no_owned_process(&plan_id);
        preserved.assert_preserved().await;
        openvpn::cleanup_openvpn_state(Some(&plan_id), TunnelEndpointSide::Left)
            .await
            .unwrap();
    }

    async fn generated_credentials(plan_id: &str) -> TunnelEndpointBuiltinCredentials {
        let directory = PathBuf::from(".tmp").join(format!("openvpn-isolated-{plan_id}"));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let key = directory.join("key.pem");
        let cert = directory.join("cert.pem");
        // Disposable TLS fixture only. RSA-2048 satisfies OpenVPN's default TLS
        // security level, and one day is sufficient for this single test run.
        checked_native(
            "/usr/bin/openssl",
            &[
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                key.to_str().unwrap(),
                "-out",
                cert.to_str().unwrap(),
                "-days",
                "1",
                "-subj",
                "/CN=isolated-runtime-test",
            ],
        )
        .await;
        let key_pem = tokio::fs::read_to_string(&key).await.unwrap();
        let cert_pem = tokio::fs::read_to_string(&cert).await.unwrap();
        tokio::fs::remove_dir_all(&directory).await.unwrap();
        TunnelEndpointBuiltinCredentials::Openvpn {
            generation: 1,
            local_private_key_pem: key_pem,
            local_certificate_pem: cert_pem.clone(),
            peer_issuer_certificate_pem: cert_pem,
            // No peer handshake is performed; the fingerprint only contributes to the state hash.
            peer_certificate_sha256_fingerprint: "00".repeat(32),
        }
    }

    #[tokio::test]
    #[ignore = "requires disposable Docker private networking, OpenVPN, OpenSSL, TUN, NET_ADMIN and VPSMAN_TEST_ISOLATED_NETWORK=1"]
    async fn openvpn_real_started_daemon_is_stopped_after_injected_apply_failure() {
        let mut preserved = PreservedResources::new().await;
        let plan = isolated_plan(TunnelKind::Openvpn);
        let plan_id = uuid::Uuid::new_v4().to_string();
        let credentials = generated_credentials(&plan_id).await;
        let mut config = config();
        config.network.runtime_ip_argv = fail_mtu_argv();
        let report = reconcile(&config, &plan, Some(&plan_id), Some(&credentials))
            .await
            .unwrap();
        assert_eq!(report["status"], "failed", "{report}");
        assert_eq!(
            report_step(&report, "runtime_openvpn_start")["success"],
            true
        );
        assert_eq!(
            report_step(&report, "runtime_openvpn_interface_ready")["success"],
            true
        );
        assert_eq!(report_step(&report, "runtime_link_mtu")["success"], false);
        assert_link_absent(&plan).await;
        assert_no_owned_process(&plan_id);
        preserved.assert_preserved().await;
        openvpn::cleanup_openvpn_state(Some(&plan_id), TunnelEndpointSide::Left)
            .await
            .unwrap();
    }
}

const MANAGER_TEST_KINDS: [TunnelKind; 8] = [
    TunnelKind::Gre,
    TunnelKind::Ipip,
    TunnelKind::Sit,
    TunnelKind::Fou,
    TunnelKind::Wireguard,
    TunnelKind::Openvpn,
    TunnelKind::TunTap,
    TunnelKind::Custom,
];

#[tokio::test]
async fn external_observed_never_mutates_or_compensates_for_any_tunnel_kind() {
    let config = builtin_hook_test_config();
    let root = Path::new(&config.network.root_dir);
    for preexisting in [false, true] {
        if preexisting {
            tokio::fs::create_dir_all(root.join("sys/class/net/tunab"))
                .await
                .unwrap();
        }
        for kind in MANAGER_TEST_KINDS {
            let mut plan = plan(RuntimeTunnelManager::ExternalObserved);
            plan.kind = kind;
            let report = reconcile_hook_test(&config, &plan).await;
            assert_eq!(report["status"], "observed_only", "{kind:?}: {report}");
            assert_eq!(report["link_existed_before"], preexisting);
            assert!(report["commands"].as_array().unwrap().is_empty());
            assert!(report["compensation"].is_null());
        }
    }
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn custom_adapter_failure_uses_only_declared_compensation_for_any_tunnel_kind() {
    let config = builtin_hook_test_config();
    let root = std::env::current_dir()
        .unwrap()
        .join(&config.network.root_dir);
    let missing = root.join("missing-adapter-start");
    let mut snapshot = adapter();
    snapshot.startup = Some(command(&[missing.to_str().unwrap()]));
    snapshot.stop = Some(command(&["/bin/echo", "declared-stop"]));
    snapshot.cleanup = Some(command(&["/bin/echo", "declared-cleanup"]));
    for preexisting in [false, true] {
        if preexisting {
            tokio::fs::create_dir_all(root.join("sys/class/net/tunab"))
                .await
                .unwrap();
        }
        for kind in MANAGER_TEST_KINDS {
            let mut plan = plan(RuntimeTunnelManager::CustomAdapter);
            plan.kind = kind;
            let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
                config: &config,
                plan_id: None,
                plan: &plan,
                previous_plan: None,
                builtin_credentials: None,
                runtime_adapter: Some(&snapshot),
                side: TunnelEndpointSide::Left,
                max_timeout_secs: 10,
                effective_uid_override: Some(0),
            })
            .await
            .unwrap();
            assert_eq!(report["status"], "failed", "{kind:?}: {report}");
            assert_eq!(report["link_existed_before"], preexisting);
            let commands = report["commands"].as_array().unwrap();
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0]["label"], "runtime_adapter_startup");
            assert!(commands[0]["error"].is_string());
            let compensation = &report["compensation"];
            assert_eq!(compensation["status"], "completed");
            assert_eq!(compensation["commands"].as_array().unwrap().len(), 2);
            assert_eq!(
                compensation["commands"][0]["label"],
                "runtime_adapter_compensate_stop"
            );
            assert_eq!(
                compensation["commands"][0]["argv"],
                serde_json::json!(["/bin/echo", "declared-stop"])
            );
            assert_eq!(
                compensation["commands"][1]["label"],
                "runtime_adapter_compensate_cleanup"
            );
            assert_eq!(
                compensation["commands"][1]["argv"],
                serde_json::json!(["/bin/echo", "declared-cleanup"])
            );
            // The adapter contract, not native interface ownership, determines its
            // cleanup commands; this fixture deliberately declares no deletion.
            assert_eq!(root.join("sys/class/net/tunab").exists(), preexisting);
        }
    }
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn fou_reconcile_command_fixtures_compensate_only_resources_created_by_the_attempt() {
    // Exercise the actual reconciler and runner with recorded command fixtures.
    // These wrappers model command outcomes, not native kernel FOU support.
    for (scenario, failed_label, compensation_label) in [
        (
            "new_listener",
            "runtime_tunnel_add",
            Some("runtime_compensate_fou_delete"),
        ),
        (
            "reused_listener",
            "runtime_link_mtu",
            Some("runtime_compensate_link_delete"),
        ),
        ("incompatible_listener", "runtime_fou_add", None),
    ] {
        let mut config = builtin_hook_test_config();
        let root = std::env::current_dir()
            .unwrap()
            .join(&config.network.root_dir);
        tokio::fs::create_dir_all(&root).await.unwrap();
        let calls_path = root.join("commands.log");
        let mut plan = plan(RuntimeTunnelManager::AgentBuiltin);
        plan.kind = TunnelKind::Fou;
        let listeners = serde_json::json!([{
            "port": plan.runtime_control.fou.port,
            "ipproto": plan.runtime_control.fou.ipproto,
            "family": if scenario == "incompatible_listener" { "inet6" } else { "inet" },
        }]);
        config.network.runtime_ip_argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            r#"fixture_log=$1
fixture_case=$2
fixture_listeners=$3
shift 3
printf '%s\n' "$*" >> "$fixture_log"
case "$1 $2" in
    'fou add') test "$fixture_case" = 'new_listener' ;;
    '-j fou') printf '%s\n' "$fixture_listeners" ;;
    'tunnel add') test "$fixture_case" = 'reused_listener' ;;
    'link set') exit 1 ;;
    'fou del'|'link delete'|'link show') exit 0 ;;
    *) exit 99 ;;
esac"#
                .to_string(),
            "fou-command-fixture".to_string(),
            calls_path.to_str().unwrap().to_string(),
            scenario.to_string(),
            listeners.to_string(),
        ];

        let report = reconcile_hook_test(&config, &plan).await;

        assert_eq!(report["status"], "failed", "{scenario}: {report}");
        assert_eq!(report["link_existed_before"], false);
        let commands = report["commands"].as_array().unwrap();
        let failed = commands
            .iter()
            .find(|command| command["label"] == failed_label)
            .unwrap();
        assert_eq!(failed["success"], false, "{scenario}: {report}");
        let fou_add = commands
            .iter()
            .find(|command| command["label"] == "runtime_fou_add")
            .unwrap();
        assert_eq!(fou_add["success"], scenario != "incompatible_listener");
        assert_eq!(
            fou_add["accepted_existing_listener"].as_bool() == Some(true),
            scenario == "reused_listener"
        );
        if let Some(tunnel_add) = commands
            .iter()
            .find(|command| command["label"] == "runtime_tunnel_add")
        {
            assert_eq!(tunnel_add["success"], scenario == "reused_listener");
        }
        let compensation = &report["compensation"];
        assert_eq!(
            compensation["status"],
            if compensation_label.is_some() {
                "completed"
            } else {
                "not_available"
            }
        );
        assert_eq!(
            compensation["commands"]
                .as_array()
                .unwrap()
                .iter()
                .map(|command| command["label"].as_str().unwrap())
                .collect::<Vec<_>>(),
            compensation_label.into_iter().collect::<Vec<_>>()
        );
        let calls = tokio::fs::read_to_string(&calls_path).await.unwrap();
        assert_eq!(
            calls.lines().any(|line| line.starts_with("tunnel add ")),
            scenario != "incompatible_listener",
            "{scenario}: {calls}"
        );
        let expected_deletions = match scenario {
            "new_listener" => vec![format!("fou del port {}", plan.runtime_control.fou.port)],
            "reused_listener" => vec![format!("link delete dev {}", plan.interface_name)],
            _ => Vec::new(),
        };
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.starts_with("fou del ") || line.starts_with("link delete "))
                .collect::<Vec<_>>(),
            expected_deletions,
            "{scenario}: {calls}"
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
