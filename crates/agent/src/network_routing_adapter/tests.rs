use std::os::unix::fs::PermissionsExt;

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
        ospf: None,
    })
    .unwrap()
}

#[test]
fn routing_adapter_request_preserves_nat_specific_endpoint_underlay() {
    let mut plan = test_plan();
    plan.left_remote_underlay = "203.0.113.20".to_string();
    plan.left_local_underlay = Some("10.0.0.10".to_string());
    let adapter = adapter_request(
        &NetworkRoutingAdapterInput {
            job_id: uuid::Uuid::new_v4(),
            client_id: "left-a",
            plan_id: "plan-a",
            plan: &plan,
            side: TunnelEndpointSide::Left,
            adapter: &adapter(
                std::path::Path::new("/bin/true"),
                std::path::Path::new("/bin/true"),
            ),
            expected_current_cost: Some(20),
            desired_cost: None,
            max_timeout_secs: 10,
            cancel_token: CommandCancelToken::default(),
        },
        RoutingCostAdapterOperation::Status,
        None,
    );

    assert_eq!(adapter.local_underlay.as_deref(), Some("10.0.0.10"));
    assert_eq!(adapter.remote_underlay, "203.0.113.20");
}

fn write_script(path: &std::path::Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

fn adapter(status: &std::path::Path, update: &std::path::Path) -> RoutingCostAdapterCommands {
    RoutingCostAdapterCommands {
        template_id: "routing-adapter-a".to_string(),
        template_name: "Test routing adapter".to_string(),
        definition_hash: "a".repeat(64),
        status: RuntimeTunnelCommand {
            argv: vec![status.to_string_lossy().to_string()],
            ..RuntimeTunnelCommand::default()
        },
        update: RuntimeTunnelCommand {
            argv: vec![update.to_string_lossy().to_string()],
            ..RuntimeTunnelCommand::default()
        },
    }
}

#[tokio::test]
async fn apply_is_compare_and_set_and_verifies_the_result() {
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
            "#!/bin/sh\ncat >/dev/null\ncost=$(cat '{}')\nprintf '{{\"contract_version\":1,\"interface_name\":\"tunab\",\"ready\":true,\"current_cost\":%s,\"applied_cost\":null,\"adapter_version\":\"test\",\"message\":null}}\\n' \"$cost\"\n",
            state.display()
        ),
    );
    write_script(
        &update,
        &format!(
            "#!/bin/sh\npayload=$(cat)\nprintf '%s' \"$payload\" | grep -q '\"desired_cost\":30' || exit 2\nprintf '30' > '{}'\nprintf '{{\"contract_version\":1,\"interface_name\":\"tunab\",\"ready\":true,\"current_cost\":20,\"applied_cost\":30,\"adapter_version\":\"test\",\"message\":null}}\\n'\n",
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
    assert_eq!(result.before.unwrap().current_cost, Some(20));
    assert_eq!(result.after.current_cost, Some(30));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn apply_rejects_a_stale_confirmation_before_update() {
    let root = std::env::temp_dir().join(format!("vpsman-routing-stale-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let status = root.join("status.sh");
    let update = root.join("update.sh");
    write_script(
        &status,
        "#!/bin/sh\ncat >/dev/null\nprintf '{\"contract_version\":1,\"interface_name\":\"tunab\",\"ready\":true,\"current_cost\":21,\"applied_cost\":null,\"adapter_version\":null,\"message\":null}\\n'\n",
    );
    write_script(&update, "#!/bin/sh\nexit 99\n");
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
    let _ = std::fs::remove_dir_all(root);
}
