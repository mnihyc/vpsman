use super::*;
use vpsman_common::{
    backend_config_signature_payload, plan_tunnel, render_tunnel_endpoint_backend_config,
    AgentNetworkConfig, BandwidthTier, OspfCostPolicy, TunnelConfigBackend, TunnelKind,
    TunnelPlanInput,
};

#[tokio::test]
async fn applies_managed_network_files_with_backups() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-apply-{job_id}"));
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
    tokio::fs::set_permissions(&ifupdown_path, std::fs::Permissions::from_mode(0o640))
        .await
        .unwrap();
    tokio::fs::set_permissions(&bird_path, std::fs::Permissions::from_mode(0o600))
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

    let outputs = execute_network_apply_command(NetworkApplyInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: &payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: &payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let ifupdown = tokio::fs::read_to_string(&ifupdown_path).await.unwrap();
    let bird = tokio::fs::read_to_string(&bird_path).await.unwrap();
    assert!(ifupdown.contains("existing"));
    assert!(ifupdown.contains("vpsman-managed ifupdown begin left-a right-b left-right tunlr"));
    assert!(ifupdown.contains("remote 203.0.113.20 local 198.51.100.10"));
    assert!(bird.contains("vpsman-managed bird2 begin left-a right-b left-right tunlr"));
    assert!(root
        .join("etc/network/interfaces.d")
        .read_dir()
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("vpsman-backup")));
    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["type"], "network_apply");
    assert_eq!(status["rollback_available"], true);
}

#[tokio::test]
async fn applies_netplan_backend_files_with_config_signature_hash() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-netplan-{job_id}"));
    let netplan_path = root.join("etc/netplan/90-vpsman-tunnels.yaml");
    let bird_path = root.join("etc/bird/vpsman-ospf.conf");
    tokio::fs::create_dir_all(netplan_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(bird_path.parent().unwrap())
        .await
        .unwrap();
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let backend_config = render_tunnel_endpoint_backend_config(
        &plan,
        TunnelEndpointSide::Left,
        TunnelConfigBackend::Netplan,
    )
    .unwrap();
    let config_hash = payload_hash(&backend_config_signature_payload(&backend_config));
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            backend: TunnelConfigBackend::Netplan,
            root_dir: root.to_string_lossy().to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    let outputs = execute_network_apply_command(NetworkApplyInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Netplan,
        config_sha256_hex: Some(&config_hash),
        ifupdown_sha256_hex: &payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: &payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let netplan = tokio::fs::read_to_string(&netplan_path).await.unwrap();
    assert!(netplan.contains("vpsman-managed netplan begin left-a right-b left-right tunlr"));
    assert!(netplan.contains("mode: gre"));
    assert!(netplan.contains("local: 198.51.100.10"));
    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["config_backend"], "netplan");
    assert_eq!(
        status["applied_files"][0]["path"],
        "/etc/netplan/90-vpsman-tunnels.yaml"
    );
}

#[tokio::test]
async fn runtime_apply_rejects_bird2_when_reconcile_is_degraded_without_override() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-runtime-gate-{job_id}"));
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
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            runtime_ip_argv: vec!["/bin/echo".to_string()],
            runtime_tc_argv: vec!["/bin/echo".to_string()],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    let error = execute_network_apply_command(NetworkApplyInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: &payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: &payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("runtime tunnel is not ready for Bird2 routing update"));
    assert!(!ifupdown_path.exists());
    assert!(!bird_path.exists());
}

#[tokio::test]
async fn runtime_apply_records_degraded_routing_gate_when_override_enabled() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-runtime-gate-allowed-{job_id}"));
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
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            allow_routing_without_runtime_ready: true,
            root_dir: root.to_string_lossy().to_string(),
            runtime_ip_argv: vec!["/bin/echo".to_string()],
            runtime_tc_argv: vec!["/bin/echo".to_string()],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    let outputs = execute_network_apply_command(NetworkApplyInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: &payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: &payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(
        status["runtime_reconcile"]["status"],
        "degraded_unprivileged"
    );
    assert_eq!(status["routing_gate"]["status"], "degraded_allowed");
    assert_eq!(
        status["routing_gate"]["reason"],
        "allow_routing_without_runtime_ready"
    );
    assert!(tokio::fs::read_to_string(&bird_path)
        .await
        .unwrap()
        .contains("vpsman-managed bird2 begin"));
}

#[tokio::test]
async fn rejects_disabled_or_hash_mismatched_apply() {
    let job_id = uuid::Uuid::new_v4();
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: false,
            root_dir: "/tmp".to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    assert!(execute_network_apply_command(NetworkApplyInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: &payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: &payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .is_err());

    let config = AgentConfig {
        network: AgentNetworkConfig {
            apply_enabled: true,
            root_dir: "/tmp".to_string(),
            ..AgentNetworkConfig::default()
        },
        ..config
    };
    assert!(execute_network_apply_command(NetworkApplyInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: &"00".repeat(32),
        bird2_sha256_hex: &payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .is_err());
}

#[tokio::test]
async fn runs_validation_and_reload_hooks_after_apply() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-hooks-{job_id}"));
    let ifupdown_path = root.join("etc/network/interfaces.d/vpsman-tunnels");
    let bird_path = root.join("etc/bird/vpsman-ospf.conf");
    tokio::fs::create_dir_all(ifupdown_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(bird_path.parent().unwrap())
        .await
        .unwrap();
    let hook_log = root.join("hook.log");
    let validate_hook = root.join("validate-hook");
    let reload_hook = root.join("reload-hook");
    write_hook(
        &validate_hook,
        &format!(
            "#!/bin/sh\nprintf 'validate\\n' >> '{}'\n",
            hook_log.display()
        ),
    )
    .await;
    write_hook(
        &reload_hook,
        &format!(
            "#!/bin/sh\nprintf 'reload\\n' >> '{}'\n",
            hook_log.display()
        ),
    )
    .await;
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            validate_enabled: true,
            reload_enabled: true,
            hook_timeout_secs: 5,
            ifupdown_validate_argv: vec![validate_hook.to_string_lossy().to_string()],
            reload_argv: vec![vec![reload_hook.to_string_lossy().to_string()]],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    let outputs = execute_network_apply_command(NetworkApplyInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: &payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: &payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let log = tokio::fs::read_to_string(&hook_log).await.unwrap();
    assert_eq!(log, "validate\nreload\n");
    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["validation"][0]["label"], "ifupdown_validate");
    assert_eq!(status["reload"][0]["label"], "reload_0");
}

#[tokio::test]
async fn rolls_back_files_when_validation_hook_fails() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-validate-fail-{job_id}"));
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
    tokio::fs::set_permissions(&ifupdown_path, std::fs::Permissions::from_mode(0o640))
        .await
        .unwrap();
    tokio::fs::set_permissions(&bird_path, std::fs::Permissions::from_mode(0o600))
        .await
        .unwrap();
    let bad_hook = root.join("bad-hook");
    write_hook(&bad_hook, "#!/bin/sh\nexit 17\n").await;
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            validate_enabled: true,
            hook_timeout_secs: 5,
            ifupdown_validate_argv: vec![bad_hook.to_string_lossy().to_string()],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    assert!(execute_network_apply_command(NetworkApplyInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: &payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: &payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .is_err());
    assert_eq!(
        tokio::fs::read_to_string(&ifupdown_path).await.unwrap(),
        "existing\n"
    );
    assert_eq!(
        tokio::fs::read_to_string(&bird_path).await.unwrap(),
        "existing bird\n"
    );
    assert_eq!(mode(&ifupdown_path).await, 0o640);
    assert_eq!(mode(&bird_path).await, 0o600);
}

#[tokio::test]
async fn managed_file_apply_deadline_leaves_existing_file_unchanged() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-deadline-{job_id}"));
    let bird_path = root.join("etc/bird/vpsman-ospf.conf");
    tokio::fs::create_dir_all(bird_path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&bird_path, "existing bird\n")
        .await
        .unwrap();

    let planned = prepare_file_update(
        &bird_path,
        MANAGED_BIRD2_FILE,
        "# vpsman-managed bird2 begin left-a right-b left-right tunlr\nnext\n# vpsman-managed bird2 end left-a right-b left-right tunlr\n",
    )
    .await
    .unwrap();
    let error = match apply_updates_with_rollback(
        &[planned],
        time::Instant::now() - Duration::from_millis(1),
        "network apply",
    )
    .await
    {
        Ok(_) => panic!("expired network deadline unexpectedly applied updates"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("network apply timed out"));
    assert_eq!(
        tokio::fs::read_to_string(&bird_path).await.unwrap(),
        "existing bird\n"
    );

    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn updates_only_bird2_managed_block_for_ospf_cost_change() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-ospf-update-{job_id}"));
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
    let hook_log = root.join("ospf-hook.log");
    let validate_hook = root.join("bird-validate-hook");
    let reload_hook = root.join("bird-reload-hook");
    write_hook(
        &validate_hook,
        &format!(
            "#!/bin/sh\nprintf 'bird-validate\\n' >> '{}'\n",
            hook_log.display()
        ),
    )
    .await;
    write_hook(
        &reload_hook,
        &format!(
            "#!/bin/sh\nprintf 'bird-reload\\n' >> '{}'\n",
            hook_log.display()
        ),
    )
    .await;
    let apply_config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            validate_enabled: true,
            reload_enabled: true,
            hook_timeout_secs: 5,
            bird2_validate_argv: vec![validate_hook.to_string_lossy().to_string()],
            bird2_reload_argv: vec![vec![reload_hook.to_string_lossy().to_string()]],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    execute_network_apply_command(NetworkApplyInput {
        job_id,
        config: &apply_config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: &payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: &payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();
    let ifupdown_before = tokio::fs::read_to_string(&ifupdown_path).await.unwrap();
    let current_ospf_cost = plan.recommended_ospf_cost;
    let recommended_ospf_cost = current_ospf_cost + 10;
    let mut proposed_plan = plan;
    proposed_plan.recommended_ospf_cost = recommended_ospf_cost;
    let proposed_endpoint =
        render_tunnel_endpoint_config(&proposed_plan, TunnelEndpointSide::Left).unwrap();

    let outputs = execute_network_ospf_cost_update_command(NetworkOspfCostUpdateInput {
        job_id,
        config: &config,
        plan: &proposed_plan,
        side: TunnelEndpointSide::Left,
        current_ospf_cost,
        recommended_ospf_cost,
        bird2_sha256_hex: &payload_hash(proposed_endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    assert_eq!(
        tokio::fs::read_to_string(&ifupdown_path).await.unwrap(),
        ifupdown_before
    );
    let bird = tokio::fs::read_to_string(&bird_path).await.unwrap();
    assert!(bird.contains(&format!("cost {recommended_ospf_cost};")));
    assert!(!bird.contains(&format!("cost {current_ospf_cost};")));
    assert_eq!(
        tokio::fs::read_to_string(&hook_log).await.unwrap(),
        "bird-validate\nbird-reload\n"
    );
    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["type"], "network_ospf_cost_update");
    assert_eq!(status["applied_files"][0]["path"], MANAGED_BIRD2_FILE);
    assert_eq!(status["rollback_mode"], "apply_previous_cost");
}

#[tokio::test]
async fn ospf_cost_update_requires_runtime_link_when_runtime_reconcile_enabled() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-ospf-runtime-gate-{job_id}"));
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
    let apply_config = AgentConfig {
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
        config: &apply_config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: &payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: &payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let current_ospf_cost = plan.recommended_ospf_cost;
    let recommended_ospf_cost = current_ospf_cost + 12;
    let mut proposed_plan = plan;
    proposed_plan.recommended_ospf_cost = recommended_ospf_cost;
    let proposed_endpoint =
        render_tunnel_endpoint_config(&proposed_plan, TunnelEndpointSide::Left).unwrap();
    let runtime_config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };

    let error = execute_network_ospf_cost_update_command(NetworkOspfCostUpdateInput {
        job_id,
        config: &runtime_config,
        plan: &proposed_plan,
        side: TunnelEndpointSide::Left,
        current_ospf_cost,
        recommended_ospf_cost,
        bird2_sha256_hex: &payload_hash(proposed_endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("is not present before Bird2 OSPF cost update"));

    tokio::fs::create_dir_all(root.join("sys/class/net/tunlr"))
        .await
        .unwrap();
    let outputs = execute_network_ospf_cost_update_command(NetworkOspfCostUpdateInput {
        job_id,
        config: &runtime_config,
        plan: &proposed_plan,
        side: TunnelEndpointSide::Left,
        current_ospf_cost,
        recommended_ospf_cost,
        bird2_sha256_hex: &payload_hash(proposed_endpoint.bird2_interface_snippet.as_bytes()),
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["routing_gate"]["status"], "ready");
    assert_eq!(status["routing_gate"]["source"], "sysfs");
    assert_eq!(status["routing_gate"]["interface"], "tunlr");
}

#[tokio::test]
async fn removes_managed_blocks_during_operator_rollback() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-rollback-{job_id}"));
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
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let outputs = execute_network_rollback_command(NetworkRollbackInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let ifupdown = tokio::fs::read_to_string(&ifupdown_path).await.unwrap();
    let bird = tokio::fs::read_to_string(&bird_path).await.unwrap();
    assert_eq!(ifupdown, "existing\n\n");
    assert_eq!(bird, "existing bird\n\n");
    assert!(!ifupdown.contains("vpsman-managed ifupdown begin"));
    assert!(!bird.contains("vpsman-managed bird2 begin"));
    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["type"], "network_rollback");
    assert_eq!(status["changed"], true);
    assert_eq!(status["removed_files"][0]["changed"], true);
}

#[tokio::test]
async fn rollback_reports_runtime_tunnel_remove_when_runtime_reconcile_enabled() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-runtime-rollback-{job_id}"));
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
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        display_name: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            allow_routing_without_runtime_ready: true,
            root_dir: root.to_string_lossy().to_string(),
            runtime_ip_argv: vec!["/bin/echo".to_string()],
            runtime_tc_argv: vec!["/bin/echo".to_string()],
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
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();
    tokio::fs::create_dir_all(root.join("sys/class/net/tunlr"))
        .await
        .unwrap();

    let outputs = execute_network_rollback_command(NetworkRollbackInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["type"], "network_rollback");
    assert_eq!(status["runtime_remove"]["status"], "degraded_unprivileged");
    let commands = status["runtime_remove"]["commands"].as_array().unwrap();
    let labels = commands
        .iter()
        .map(|command| command["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"runtime_link_delete"));
    assert!(commands
        .iter()
        .any(|command| command["label"] == "runtime_link_delete"
            && command["skipped"].as_bool() == Some(true)
            && command["reason"] == "agent_unprivileged"));
}

#[tokio::test]
async fn rollback_is_idempotent_when_managed_block_is_absent() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-rollback-idem-{job_id}"));
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

    let outputs = execute_network_rollback_command(NetworkRollbackInput {
        job_id,
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    assert_eq!(
        tokio::fs::read_to_string(&ifupdown_path).await.unwrap(),
        "existing\n"
    );
    assert_eq!(
        tokio::fs::read_to_string(&bird_path).await.unwrap(),
        "existing bird\n"
    );
    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["changed"], false);
    assert!(status["pre_rollback"].as_array().unwrap().is_empty());
    assert!(status["validation"].as_array().unwrap().is_empty());
    assert!(status["reload"].as_array().unwrap().is_empty());
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
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
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

async fn mode(path: &Path) -> u32 {
    tokio::fs::metadata(path)
        .await
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}
