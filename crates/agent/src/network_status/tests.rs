use super::*;
use vpsman_common::{
    plan_tunnel, AgentNetworkConfig, RuntimeTunnelControl, RuntimeTunnelManager,
    TunnelEndpointSide, TunnelKind, TunnelPlanInput,
};

fn test_plan(manager: RuntimeTunnelManager) -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "edge-link".to_string(),
        interface_name: "tunab".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: RuntimeTunnelControl {
            manager,
            ..RuntimeTunnelControl::default()
        },
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

#[tokio::test]
async fn status_is_scoped_to_the_declared_plan() {
    let job_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("vpsman-network-status-{job_id}"));
    tokio::fs::create_dir_all(root.join("sys/class/net/unrelated-wg0"))
        .await
        .unwrap();
    let config = AgentConfig {
        client_id: "left-a".to_string(),
        network: AgentNetworkConfig {
            root_dir: root.to_string_lossy().to_string(),
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    };
    let plan = test_plan(RuntimeTunnelManager::ExternalObserved);

    let outputs = execute_network_status_command(NetworkStatusInput {
        job_id,
        config: &config,
        plan: &plan,
        runtime_adapter: None,
        side: TunnelEndpointSide::Left,
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
    assert_eq!(status["scope"], "declared_plan_only");
    assert_eq!(status["runtime"]["interface"]["interface"], "tunab");
    assert!(status["runtime"].get("observed_tunnels").is_none());
    assert_eq!(status["runtime"]["summary"]["status"], "drift");
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn status_rejects_a_side_for_another_agent() {
    let config = AgentConfig {
        client_id: "right-b".to_string(),
        ..AgentConfig::default()
    };
    let error = execute_network_status_command(NetworkStatusInput {
        job_id: uuid::Uuid::new_v4(),
        config: &config,
        plan: &test_plan(RuntimeTunnelManager::AgentIproute2Managed),
        runtime_adapter: None,
        side: TunnelEndpointSide::Left,
        max_timeout_secs: 5,
        cancel_token: CommandCancelToken::default(),
    })
    .await
    .unwrap_err();
    assert!(error.to_string().contains("targets left-a"));
}
