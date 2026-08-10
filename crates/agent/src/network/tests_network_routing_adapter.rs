use super::*;
use vpsman_common::{plan_tunnel, RuntimeTunnelControl, TunnelKind, TunnelPlanInput};

fn test_plan() -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "edge-link".to_string(),
        interface_name: "tunab".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: RuntimeTunnelControl::default(),
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.255.0.0/24".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        left_mtu: Some(1476),
        right_mtu: Some(1476),
        ospf: None,
    })
    .unwrap()
}

fn command(argv: &[String]) -> RuntimeTunnelCommand {
    RuntimeTunnelCommand {
        argv: argv.to_vec(),
        ..RuntimeTunnelCommand::default()
    }
}

fn adapter(status: &std::path::Path, update: &std::path::Path) -> RoutingCostAdapterCommands {
    RoutingCostAdapterCommands {
        source: vpsman_common::RoutingCostCommandSource::PlanOverride,
        definition_id: "routing-adapter-a".to_string(),
        definition_name: "Test routing adapter".to_string(),
        definition_hash: "a".repeat(64),
        status: command(&[
            "/bin/sh".to_string(),
            status.to_string_lossy().to_string(),
            "{plan_id}".to_string(),
            "{interface}".to_string(),
            "{endpoint_side}".to_string(),
            "{local_underlay}".to_string(),
            "{remote_underlay}".to_string(),
        ]),
        update: command(&[
            "/bin/sh".to_string(),
            update.to_string_lossy().to_string(),
            "{plan_id}".to_string(),
            "{interface}".to_string(),
            "{endpoint_side}".to_string(),
            "{expected_current_cost}".to_string(),
            "{desired_cost}".to_string(),
        ]),
    }
}

#[test]
fn routing_adapter_argv_contains_exact_endpoint_and_cost_evidence() {
    let mut plan = test_plan();
    plan.name = "edge-{plan_id}-{desired_cost}-{kind}".to_string();
    plan.left_remote_underlay = "203.0.113.20".to_string();
    plan.left_local_underlay = Some("10.0.0.10".to_string());
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let rendered = render_routing_adapter_command(
        &command(&[
            "/opt/operator/routing-cost".to_string(),
            "{plan_id}".to_string(),
            "{plan}".to_string(),
            "{interface}".to_string(),
            "{endpoint_side}".to_string(),
            "{local_underlay}".to_string(),
            "{remote_underlay}".to_string(),
            "{local_address}".to_string(),
            "{remote_address}".to_string(),
            "{expected_current_cost}".to_string(),
            "{desired_cost}".to_string(),
        ]),
        "plan-a",
        &plan,
        &endpoint,
        TunnelEndpointSide::Left,
        Some(20),
        Some(30),
    )
    .unwrap();

    assert_eq!(
        rendered,
        [
            "/opt/operator/routing-cost",
            "plan-a",
            "edge-{plan_id}-{desired_cost}-{kind}",
            "tunab",
            "left",
            "10.0.0.10",
            "203.0.113.20",
            "10.255.0.0",
            "10.255.0.1",
            "20",
            "30",
        ]
    );
}

fn write_script(path: &std::path::Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

#[tokio::test]
async fn apply_uses_argv_exit_code_message_and_verified_status() {
    let root =
        std::env::temp_dir().join(format!("vpsman-routing-adapter-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("cost");
    std::fs::write(&state, "20").unwrap();
    let status = root.join("status.sh");
    let update = root.join("update.sh");
    write_script(
        &status,
        &format!(
            "#!/bin/sh\n[ \"$1\" = plan-a ] || exit 11\n[ \"$2\" = tunab ] || exit 12\n[ \"$3\" = left ] || exit 13\n[ -z \"$4\" ] || exit 14\n[ \"$5\" = 198.51.100.10 ] || exit 15\nif read -r unexpected; then exit 16; fi\ncat '{}'\n",
            state.display()
        ),
    );
    write_script(
        &update,
        &format!(
            "#!/bin/sh\n[ \"$1\" = plan-a ] || exit 21\n[ \"$2\" = tunab ] || exit 22\n[ \"$3\" = left ] || exit 23\n[ \"$4\" = 20 ] || exit 24\n[ \"$5\" = 30 ] || exit 25\nprintf '%s' \"$5\" > '{}'\nprintf 'cost updated to %s\\n' \"$5\"\n",
            state.display()
        ),
    );

    let outputs = execute_network_routing_adapter_command(NetworkRoutingAdapterInput {
        job_id: uuid::Uuid::new_v4(),
        client_id: "left-a",
        plan_id: "plan-a",
        plan: &test_plan(),
        side: TunnelEndpointSide::Left,
        adapter: &adapter(&status, &update),
        expected_current_cost: Some(20),
        desired_cost: Some(30),
        max_timeout_secs: 10,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();
    let result: RoutingCostAdapterJobResult = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(result.previous_cost, Some(20));
    assert_eq!(result.current_cost, 30);
    assert_eq!(result.message.as_deref(), Some("cost updated to 30"));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn status_returns_the_numeric_cost_without_running_update() {
    let root = std::env::temp_dir().join(format!("vpsman-routing-status-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let marker = root.join("update-ran");
    let status = root.join("status.sh");
    let update = root.join("update.sh");
    write_script(&status, "#!/bin/sh\nprintf '17\\n'\n");
    write_script(
        &update,
        &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );

    let outputs = execute_network_routing_adapter_command(NetworkRoutingAdapterInput {
        job_id: uuid::Uuid::new_v4(),
        client_id: "left-a",
        plan_id: "plan-a",
        plan: &test_plan(),
        side: TunnelEndpointSide::Left,
        adapter: &adapter(&status, &update),
        expected_current_cost: None,
        desired_cost: None,
        max_timeout_secs: 10,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let result: RoutingCostAdapterJobResult = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(result.operation, RoutingCostAdapterOperation::Status);
    assert_eq!(result.previous_cost, None);
    assert_eq!(result.current_cost, 17);
    assert_eq!(result.message, None);
    assert!(!marker.exists());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn apply_without_a_recorded_cost_uses_the_observed_baseline() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-routing-initial-apply-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let state = root.join("cost");
    std::fs::write(&state, "20").unwrap();
    let status = root.join("status.sh");
    let update = root.join("update.sh");
    write_script(&status, &format!("#!/bin/sh\ncat '{}'\n", state.display()));
    write_script(
        &update,
        &format!("#!/bin/sh\nprintf '%s' \"$5\" > '{}'\n", state.display()),
    );

    let outputs = execute_network_routing_adapter_command(NetworkRoutingAdapterInput {
        job_id: uuid::Uuid::new_v4(),
        client_id: "left-a",
        plan_id: "plan-a",
        plan: &test_plan(),
        side: TunnelEndpointSide::Left,
        adapter: &adapter(&status, &update),
        expected_current_cost: None,
        desired_cost: Some(30),
        max_timeout_secs: 10,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let result: RoutingCostAdapterJobResult = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(result.previous_cost, Some(20));
    assert_eq!(result.current_cost, 30);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn apply_rejects_a_stale_confirmation_before_update() {
    let root = std::env::temp_dir().join(format!("vpsman-routing-stale-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let marker = root.join("update-ran");
    let status = root.join("status.sh");
    let update = root.join("update.sh");
    write_script(&status, "#!/bin/sh\nprintf '21\\n'\n");
    write_script(
        &update,
        &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );
    let error = execute_network_routing_adapter_command(NetworkRoutingAdapterInput {
        job_id: uuid::Uuid::new_v4(),
        client_id: "left-a",
        plan_id: "plan-a",
        plan: &test_plan(),
        side: TunnelEndpointSide::Left,
        adapter: &adapter(&status, &update),
        expected_current_cost: Some(20),
        desired_cost: Some(30),
        max_timeout_secs: 10,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("stale routing cost confirmation"));
    assert!(!marker.exists());
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn apply_reports_nonzero_exit_output_without_running_verification() {
    let root =
        std::env::temp_dir().join(format!("vpsman-routing-failure-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let status = root.join("status.sh");
    let update = root.join("update.sh");
    write_script(&status, "#!/bin/sh\nprintf '20\\n'\n");
    write_script(
        &update,
        "#!/bin/sh\nprintf 'FRR rejected the update\\n'\nprintf 'invalid interface\\n' >&2\nexit 7\n",
    );

    let error = execute_network_routing_adapter_command(NetworkRoutingAdapterInput {
        job_id: uuid::Uuid::new_v4(),
        client_id: "left-a",
        plan_id: "plan-a",
        plan: &test_plan(),
        side: TunnelEndpointSide::Left,
        adapter: &adapter(&status, &update),
        expected_current_cost: Some(20),
        desired_cost: Some(30),
        max_timeout_secs: 10,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("exited with Some(7)"));
    assert!(message.contains("FRR rejected the update"));
    assert!(message.contains("invalid interface"));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn apply_rejects_an_unverified_result() {
    let root = std::env::temp_dir().join(format!("vpsman-routing-verify-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let status_calls = root.join("status-calls");
    std::fs::write(&status_calls, "0").unwrap();
    let status = root.join("status.sh");
    let update = root.join("update.sh");
    write_script(
        &status,
        &format!(
            "#!/bin/sh\ncalls=$(cat '{}')\nprintf '%s' $((calls + 1)) > '{}'
if [ \"$calls\" = 0 ]; then printf '20\\n'; else printf '29\\n'; fi\n",
            status_calls.display(),
            status_calls.display()
        ),
    );
    write_script(&update, "#!/bin/sh\nprintf 'update accepted\\n'\n");

    let error = execute_network_routing_adapter_command(NetworkRoutingAdapterInput {
        job_id: uuid::Uuid::new_v4(),
        client_id: "left-a",
        plan_id: "plan-a",
        plan: &test_plan(),
        side: TunnelEndpointSide::Left,
        adapter: &adapter(&status, &update),
        expected_current_cost: Some(20),
        desired_cost: Some(30),
        max_timeout_secs: 10,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("routing cost verification failed: desired 30, observed 29"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn status_requires_one_valid_numeric_cost() {
    assert_eq!(parse_status_cost(b"42\n").unwrap(), 42);
    for invalid in [b"".as_slice(), b"none", b"0", b"42 ready", b"65536"] {
        assert!(parse_status_cost(invalid).is_err(), "accepted {invalid:?}");
    }
}
