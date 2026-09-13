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
        build_runtime_compensation_steps(&config, &plan, &endpoint, None, false).unwrap();
    assert!(unproven.is_empty());
    assert_eq!(reason, Some("no_plan_owned_link_created"));

    let (created, reason) =
        build_runtime_compensation_steps(&config, &plan, &endpoint, None, true).unwrap();
    assert_eq!(reason, None);
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].label, "runtime_compensate_link_delete");
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
