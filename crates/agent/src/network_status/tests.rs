use std::{os::unix::fs::PermissionsExt, path::Path};

use super::*;
use crate::network_apply::{
    execute_network_apply_command, execute_network_rollback_command, NetworkApplyInput,
    NetworkRollbackInput,
};
use vpsman_common::{
    payload_hash, plan_tunnel, render_tunnel_endpoint_config, AgentNetworkConfig, BandwidthTier,
    OspfCostPolicy, RuntimeTunnelCommand, RuntimeTunnelControl, RuntimeTunnelManager,
    TunnelConfigBackend, TunnelKind, TunnelPlanInput,
};

#[tokio::test]
async fn reports_managed_file_status_for_applied_and_absent_blocks() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-status-{job_id}"));
    let ifupdown_path = root.join("etc/network/interfaces.d/vpsman-tunnels");
    let bird_path = root.join("etc/bird/vpsman-ospf.conf");
    tokio::fs::create_dir_all(ifupdown_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(bird_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&ifupdown_path, "existing\n")
        .await
        .unwrap();
    tokio::fs::write(&bird_path, "existing bird\n")
        .await
        .unwrap();
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    execute_network_apply_command(NetworkApplyInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: &payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: &payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
    })
    .await
    .unwrap();
    let outputs = execute_network_status_command(NetworkStatusInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
    })
    .await
    .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["type"], "network_status");
    assert_eq!(status["applied"], true);
    assert_eq!(status["files"][0]["managed_block_present"], true);
    assert_eq!(status["files"][0]["expected_block_matches"], true);
    assert_eq!(status["files"][1]["expected_block_matches"], true);
    assert_eq!(status["runtime"]["interface"]["exists"], false);
    assert_eq!(status["runtime"]["bird2"]["skipped"], true);
    assert_eq!(
        status["runtime"]["kernel_namespace"]["real_kernel_namespace"],
        false
    );
    assert_eq!(
        status["runtime"]["kernel"]["reason"],
        "kernel_probes_require_real_root_namespace"
    );
    assert_eq!(
        status["runtime"]["summary"]["real_kernel_namespace_covered"],
        false
    );
    assert_eq!(
        status["runtime"]["summary"]["neighbor_probe_state"],
        "skipped"
    );

    execute_network_rollback_command(NetworkRollbackInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
    })
    .await
    .unwrap();
    let outputs = execute_network_status_command(NetworkStatusInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
    })
    .await
    .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["applied"], false);
    assert_eq!(status["malformed"], false);
    assert_eq!(status["files"][0]["managed_block_present"], false);
    assert_eq!(status["files"][1]["managed_block_present"], false);
}

#[tokio::test]
async fn reports_malformed_or_non_utf8_managed_files_without_writing() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-status-bad-{job_id}"));
    let ifupdown_path = root.join("etc/network/interfaces.d/vpsman-tunnels");
    let bird_path = root.join("etc/bird/vpsman-ospf.conf");
    tokio::fs::create_dir_all(ifupdown_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(bird_path.parent().unwrap())
        .await
        .unwrap();
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    tokio::fs::write(
        &ifupdown_path,
        format!(
            "# vpsman-managed ifupdown begin left-a right-b left-right tunlr\n{}\n",
            endpoint.ifupdown_snippet
        ),
    )
    .await
    .unwrap();
    tokio::fs::write(&bird_path, [0xff, 0xfe, 0xfd])
        .await
        .unwrap();
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    let outputs = execute_network_status_command(NetworkStatusInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
    })
    .await
    .unwrap();

    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["applied"], false);
    assert_eq!(status["malformed"], true);
    assert_eq!(status["files"][0]["managed_block_present"], true);
    assert_eq!(status["files"][0]["managed_block_malformed"], true);
    assert_eq!(status["files"][1]["exists"], true);
    assert_eq!(status["files"][1]["utf8"], false);
    assert_eq!(
        tokio::fs::read_to_string(&ifupdown_path).await.unwrap(),
        format!(
            "# vpsman-managed ifupdown begin left-a right-b left-right tunlr\n{}\n",
            endpoint.ifupdown_snippet
        )
    );
}

#[tokio::test]
async fn reports_runtime_interface_and_bird2_probe_status() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-runtime-{job_id}"));
    let sysfs = root.join("sys/class/net/tunlr/statistics");
    tokio::fs::create_dir_all(&sysfs).await.unwrap();
    tokio::fs::write(root.join("sys/class/net/tunlr/operstate"), "up\n")
        .await
        .unwrap();
    tokio::fs::write(root.join("sys/class/net/tunlr/mtu"), "1476\n")
        .await
        .unwrap();
    tokio::fs::write(
        root.join("sys/class/net/tunlr/address"),
        "02:00:00:00:00:01\n",
    )
    .await
    .unwrap();
    tokio::fs::write(root.join("sys/class/net/tunlr/type"), "778\n")
        .await
        .unwrap();
    tokio::fs::write(
        root.join("sys/class/net/tunlr/statistics/rx_bytes"),
        "123\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.join("sys/class/net/tunlr/statistics/tx_bytes"),
        "456\n",
    )
    .await
    .unwrap();
    let bird_status = root.join("bird-status");
    write_hook(
        &bird_status,
        "#!/bin/sh\nprintf 'bird interface=%s plan=%s local=%s peer=%s\\n' \"$1\" \"$2\" \"$3\" \"$4\"\nprintf '10.255.0.2 1 full/ptp 00:31 %s 10.255.0.2\\n' \"$1\"\n",
    )
    .await;
    let plan = test_plan();
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            root_dir: root.to_string_lossy().to_string(),
            bird2_status_argv: vec![
                bird_status.to_string_lossy().to_string(),
                "{interface}".to_string(),
                "{plan}".to_string(),
                "{local_client_id}".to_string(),
                "{peer_client_id}".to_string(),
            ],
            status_probe_timeout_secs: 5,
            status_probe_max_output_bytes: 1024,
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    let outputs = execute_network_status_command(NetworkStatusInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
    })
    .await
    .unwrap();

    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    let interface = &status["runtime"]["interface"];
    assert_eq!(interface["exists"], true);
    assert_eq!(interface["operstate"], "up");
    assert_eq!(interface["mtu"], 1476);
    assert_eq!(interface["rx_bytes"], 123);
    assert_eq!(interface["tx_bytes"], 456);
    let bird2 = &status["runtime"]["bird2"];
    assert_eq!(bird2["configured"], true);
    assert_eq!(bird2["success"], true);
    assert_eq!(bird2["exit_code"], 0);
    assert!(bird2["stdout"]["text"]
        .as_str()
        .unwrap()
        .contains("interface=tunlr plan=left-right local=left-a peer=right-b"));
    assert_eq!(bird2["parsed_ospf"]["parsed"], true);
    assert_eq!(bird2["parsed_ospf"]["interface_seen"], true);
    assert_eq!(bird2["parsed_ospf"]["full_neighbor_seen"], true);
    assert_eq!(bird2["parsed_ospf"]["healthy"], true);
    assert_eq!(bird2["parsed_ospf"]["state_counts"]["full"], 1);
    assert_eq!(bird2["healthy"], true);
    assert_eq!(status["runtime"]["summary"]["status"], "healthy");
    assert_eq!(status["runtime"]["summary"]["healthy"], true);
    assert_eq!(status["runtime"]["summary"]["bird2_state"], "healthy");
    assert_eq!(
        status["runtime"]["summary"]["neighbor_probe_state"],
        "skipped"
    );
}

#[tokio::test]
async fn reports_external_adapter_runtime_health_and_drift() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-adapter-status-{job_id}"));
    let sysfs = root.join("sys/class/net/ovpn42/statistics");
    tokio::fs::create_dir_all(&sysfs).await.unwrap();
    tokio::fs::write(root.join("sys/class/net/ovpn42/operstate"), "up\n")
        .await
        .unwrap();
    tokio::fs::write(root.join("sys/class/net/ovpn42/mtu"), "1500\n")
        .await
        .unwrap();
    tokio::fs::write(root.join("sys/class/net/ovpn42/type"), "65534\n")
        .await
        .unwrap();
    tokio::fs::write(root.join("sys/class/net/ovpn42/statistics/rx_bytes"), "9\n")
        .await
        .unwrap();
    tokio::fs::write(
        root.join("sys/class/net/ovpn42/statistics/tx_bytes"),
        "10\n",
    )
    .await
    .unwrap();
    let adapter_status = root.join("adapter-status");
    write_hook(
        &adapter_status,
        "#!/bin/sh\nprintf 'adapter interface=%s plan=%s kind=%s\\n' \"$1\" \"$2\" \"$3\"\n",
    )
    .await;
    let plan = external_adapter_plan(
        "ovpn42",
        RuntimeTunnelCommand {
            argv: vec![
                adapter_status.to_string_lossy().to_string(),
                "{interface}".to_string(),
                "{plan}".to_string(),
                "{kind}".to_string(),
            ],
            ..RuntimeTunnelCommand::default()
        },
    );
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            root_dir: root.to_string_lossy().to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    let outputs = execute_network_status_command(NetworkStatusInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
    })
    .await
    .unwrap();

    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["runtime"]["manager"], "external_managed_adapter");
    assert_eq!(status["runtime"]["adapter"]["success"], true);
    assert!(status["runtime"]["adapter"]["stdout"]["text"]
        .as_str()
        .unwrap()
        .contains("adapter interface=ovpn42 plan=external-left-right kind=openvpn"));
    assert_eq!(status["runtime"]["summary"]["status"], "healthy");
    assert_eq!(status["runtime"]["summary"]["adapter_state"], "healthy");
    assert_eq!(status["runtime"]["summary"]["healthy"], true);
    assert_eq!(status["runtime"]["desired_interfaces"][0]["exists"], true);
}

#[tokio::test]
async fn reports_declared_stale_and_external_import_candidates() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-discovery-{job_id}"));
    create_sysfs_interface(&root, "tunlr", "778", "up").await;
    create_sysfs_interface(&root, "tunold0", "65534", "up").await;
    create_sysfs_interface(&root, "ovpn42", "65534", "up").await;
    create_sysfs_interface(&root, "eth0", "1", "up").await;
    let mut plan = test_plan();
    plan.runtime_topology.stale_interfaces = vec!["tunold0".to_string()];
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            root_dir: root.to_string_lossy().to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    let outputs = execute_network_status_command(NetworkStatusInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
    })
    .await
    .unwrap();

    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["runtime"]["summary"]["status"], "drift");
    assert_eq!(status["runtime"]["summary"]["stale_present_count"], 1);
    assert_eq!(
        status["runtime"]["summary"]["external_import_candidate_count"],
        1
    );
    assert_eq!(
        status["runtime"]["summary"]["external_import_candidate_policy"],
        "observe_only_requires_plan_promotion"
    );
    assert_eq!(
        status["runtime"]["summary"]["reasons"][0],
        "stale_interface_present"
    );
    let observed = status["runtime"]["observed_tunnels"].as_array().unwrap();
    assert!(observed
        .iter()
        .any(|tunnel| tunnel["interface"] == "tunlr" && tunnel["desired"] == true));
    let stale = observed
        .iter()
        .find(|tunnel| tunnel["interface"] == "tunold0")
        .unwrap();
    assert_eq!(stale["declared_stale"], true);
    assert_eq!(
        stale["mutation_policy"],
        "delete_allowed_when_declared_stale"
    );
    let import_candidate = observed
        .iter()
        .find(|tunnel| tunnel["interface"] == "ovpn42")
        .unwrap();
    assert_eq!(import_candidate["import_candidate"], true);
    assert_eq!(import_candidate["promotion_required"], true);
    assert_eq!(
        import_candidate["mutation_policy"],
        "observe_only_import_candidate"
    );
    assert_eq!(
        import_candidate["promotion_hint"],
        "promote_to_external_observed_or_adapter_plan_before_mutation"
    );
    assert!(!observed.iter().any(|tunnel| tunnel["interface"] == "eth0"));
}

#[tokio::test]
async fn reports_external_adapter_status_failure_as_runtime_unhealthy() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-adapter-fail-{job_id}"));
    let sysfs = root.join("sys/class/net/ovpn42/statistics");
    tokio::fs::create_dir_all(&sysfs).await.unwrap();
    tokio::fs::write(root.join("sys/class/net/ovpn42/operstate"), "up\n")
        .await
        .unwrap();
    let adapter_status = root.join("adapter-status-fail");
    write_hook(
        &adapter_status,
        "#!/bin/sh\nprintf 'adapter unhealthy\\n' >&2\nexit 7\n",
    )
    .await;
    let plan = external_adapter_plan(
        "ovpn42",
        RuntimeTunnelCommand {
            argv: vec![adapter_status.to_string_lossy().to_string()],
            ..RuntimeTunnelCommand::default()
        },
    );
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            root_dir: root.to_string_lossy().to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    let outputs = execute_network_status_command(NetworkStatusInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
    })
    .await
    .unwrap();

    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["runtime"]["adapter"]["success"], false);
    assert_eq!(status["runtime"]["adapter"]["exit_code"], 7);
    assert_eq!(status["runtime"]["summary"]["status"], "adapter_unhealthy");
    assert_eq!(status["runtime"]["summary"]["healthy"], false);
    assert_eq!(
        status["runtime"]["summary"]["reasons"][0],
        "adapter_status_failed"
    );
}

#[test]
fn parses_common_bird2_neighbor_states_for_expected_interface() {
    let output = r#"
BIRD 2.0.12 ready.
ospf1:
Router ID       Pri          State      DTime   Interface  Router IP
10.255.0.2        1         full/ptp    00:31   tunlr      10.255.0.2
10.255.0.6        1         init        00:40   tunx       10.255.0.6
"#;

    let parsed = parse_bird2_ospf_status(output, "tunlr");

    assert_eq!(parsed["parsed"], true);
    assert_eq!(parsed["interface_seen"], true);
    assert_eq!(parsed["full_neighbor_seen"], true);
    assert_eq!(parsed["healthy"], true);
    assert_eq!(parsed["neighbor_state_count"], 2);
    assert_eq!(parsed["state_counts"]["full"], 1);
    assert_eq!(parsed["state_counts"]["init"], 1);
}

#[test]
fn bird2_neighbor_parser_does_not_mark_other_interfaces_healthy() {
    let output = r#"
ospf1:
Router ID       Pri          State      DTime   Interface  Router IP
10.255.0.6        1         full/ptp    00:40   other0     10.255.0.6
"#;

    let parsed = parse_bird2_ospf_status(output, "tunlr");

    assert_eq!(parsed["parsed"], true);
    assert_eq!(parsed["interface_seen"], false);
    assert_eq!(parsed["full_neighbor_seen"], false);
    assert_eq!(parsed["healthy"], false);
    assert_eq!(parsed["state_counts"]["full"], 1);
}

fn test_plan() -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "left-right".to_string(),
        interface_name: "tunlr".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 15.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .unwrap()
}

fn external_adapter_plan(interface_name: &str, status: RuntimeTunnelCommand) -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "external-left-right".to_string(),
        interface_name: interface_name.to_string(),
        kind: TunnelKind::Openvpn,
        runtime_control: RuntimeTunnelControl {
            manager: RuntimeTunnelManager::ExternalManagedAdapter,
            status: Some(status),
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 15.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .unwrap()
}

async fn write_hook(path: &Path, contents: &str) {
    tokio::fs::write(path, contents).await.unwrap();
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .await
        .unwrap();
}

async fn create_sysfs_interface(root: &Path, name: &str, link_type: &str, operstate: &str) {
    let base = root.join("sys/class/net").join(name);
    tokio::fs::create_dir_all(base.join("statistics"))
        .await
        .unwrap();
    tokio::fs::write(base.join("type"), format!("{link_type}\n"))
        .await
        .unwrap();
    tokio::fs::write(base.join("operstate"), format!("{operstate}\n"))
        .await
        .unwrap();
    tokio::fs::write(base.join("mtu"), "1500\n").await.unwrap();
}
