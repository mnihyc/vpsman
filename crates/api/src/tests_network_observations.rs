use super::*;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    Json,
};
use tokio::sync::broadcast;
use vpsman_common::{
    observed_ospf_cost, payload_hash, plan_tunnel, render_tunnel_endpoint_config, BandwidthTier,
    CommandOutput, JobCommand, OspfCostPolicy, OutputStream, TunnelConfigBackend,
    TunnelEndpointSide, TunnelKind, TunnelPlan, TunnelPlanInput,
};

use crate::{gateway_client::GatewayDispatchClient, state::EnrollmentSettings};

#[tokio::test]
async fn records_network_observation_summaries_from_status_outputs() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    repo.record_network_observations(
        job_id,
        "left-a",
        &[CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "network_probe",
                "plan": "edge-a-edge-b",
                "interface": "tunab",
                "peer_client_id": "right-b",
                "target": "10.255.0.1",
                "parsed": {
                    "healthy": true,
                    "latency_avg_ms": 17.25,
                    "packet_loss_ratio": 0.02
                }
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        }],
    )
    .await
    .unwrap();

    let observations = repo.list_network_observations(10).await.unwrap();

    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].kind, "network_probe");
    assert_eq!(observations[0].plan_name.as_deref(), Some("edge-a-edge-b"));
    assert_eq!(observations[0].latency_avg_ms, Some(17.25));
    assert_eq!(observations[0].packet_loss_ratio, Some(0.02));
    assert_eq!(observations[0].healthy, Some(true));
}

#[tokio::test]
async fn rolls_up_network_observation_trends_by_plan_and_endpoint() {
    let repo = Repository::Memory(MemoryState::default());
    let job_id = Uuid::new_v4();
    repo.record_network_observations(
        job_id,
        "left-a",
        &[
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "network_probe",
                    "plan": "edge-a-edge-b",
                    "interface": "tunab",
                    "peer_client_id": "right-b",
                    "target": "10.255.0.1",
                    "parsed": {
                        "healthy": true,
                        "latency_avg_ms": 10.0,
                        "packet_loss_ratio": 0.0
                    }
                }))
                .unwrap(),
                exit_code: Some(0),
                done: true,
            },
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "network_probe",
                    "plan": "edge-a-edge-b",
                    "interface": "tunab",
                    "peer_client_id": "right-b",
                    "target": "10.255.0.1",
                    "parsed": {
                        "healthy": false,
                        "latency_avg_ms": 30.0,
                        "packet_loss_ratio": 0.10
                    }
                }))
                .unwrap(),
                exit_code: Some(0),
                done: true,
            },
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "network_speed_test",
                    "role": "client",
                    "plan": "edge-a-edge-b",
                    "interface": "tunab",
                    "peer_client_id": "right-b",
                    "server_address": "10.255.0.1",
                    "port": 5201,
                    "success": true,
                    "bytes": 1048576,
                    "throughput_mbps": 40.0
                }))
                .unwrap(),
                exit_code: Some(0),
                done: true,
            },
        ],
    )
    .await
    .unwrap();

    let trends = repo.list_network_observation_trends(10).await.unwrap();
    let probe = trends
        .iter()
        .find(|trend| trend.kind == "network_probe")
        .unwrap();
    let speed = trends
        .iter()
        .find(|trend| trend.kind == "network_speed_test")
        .unwrap();

    assert_eq!(probe.plan_name.as_deref(), Some("edge-a-edge-b"));
    assert_eq!(probe.client_id, "left-a");
    assert_eq!(probe.peer_client_id.as_deref(), Some("right-b"));
    assert_eq!(probe.sample_count, 2);
    assert_eq!(probe.healthy_count, 1);
    assert_eq!(probe.degraded_count, 1);
    assert_eq!(probe.latency_avg_ms, Some(20.0));
    assert_eq!(probe.latency_min_ms, Some(10.0));
    assert_eq!(probe.latency_max_ms, Some(30.0));
    assert_eq!(probe.packet_loss_avg_ratio, Some(0.05));
    assert_eq!(speed.sample_count, 1);
    assert_eq!(speed.throughput_avg_mbps, Some(40.0));
    assert_eq!(speed.throughput_max_mbps, Some(40.0));
    assert_eq!(speed.bytes_total, 1_048_576);
}

#[tokio::test]
async fn topology_graph_combines_plans_endpoint_state_and_observation_trends() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "left-a".to_string(),
                display_name: "left-a".to_string(),
                status: "connected".to_string(),
                tags: vec!["bgp".to_string()],
                capabilities: Default::default(),
            },
            AgentView {
                id: "right-b".to_string(),
                display_name: "right-b".to_string(),
                status: "stale".to_string(),
                tags: vec!["bgp".to_string(), "provider:test".to_string()],
                capabilities: Default::default(),
            },
        ]);
    }
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let plan = test_plan();
    let input = TunnelPlanInput {
        name: plan.name.clone(),
        interface_name: plan.interface_name.clone(),
        kind: plan.kind,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: plan.left_client_id.clone(),
        right_client_id: plan.right_client_id.clone(),
        left_underlay: plan.left_underlay.clone(),
        right_underlay: plan.right_underlay.clone(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 18.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    };
    repo.record_tunnel_plan(&input, &plan, &operator)
        .await
        .unwrap();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    repo.record_tunnel_plan_execution(
        Uuid::new_v4(),
        &JobCommand::NetworkApply {
            plan: Box::new(plan.clone()),
            side: TunnelEndpointSide::Left,
            config_backend: TunnelConfigBackend::Ifupdown,
            config_sha256_hex: None,
            ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
            bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        },
        "completed",
    )
    .await
    .unwrap();

    let job_id = Uuid::new_v4();
    repo.record_network_observations(
        job_id,
        "left-a",
        &[
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "network_probe",
                    "plan": "edge-a-edge-b",
                    "interface": "tunab",
                    "peer_client_id": "right-b",
                    "target": "10.255.0.1",
                    "parsed": {
                        "healthy": false,
                        "latency_avg_ms": 42.0,
                        "packet_loss_ratio": 0.04
                    }
                }))
                .unwrap(),
                exit_code: Some(0),
                done: true,
            },
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "network_speed_test",
                    "role": "client",
                    "plan": "edge-a-edge-b",
                    "interface": "tunab",
                    "peer_client_id": "right-b",
                    "server_address": "10.255.0.1",
                    "port": 5201,
                    "success": true,
                    "bytes": 2097152,
                    "throughput_mbps": 80.0
                }))
                .unwrap(),
                exit_code: Some(0),
                done: true,
            },
        ],
    )
    .await
    .unwrap();

    let Json(graph) = crate::routes_network::get_topology_graph(
        State(test_state(repo)),
        HeaderMap::new(),
        Query(HistoryQuery { limit: Some(10) }),
    )
    .await
    .unwrap();

    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].plan_name, "edge-a-edge-b");
    assert_eq!(graph.edges[0].health, "degraded");
    assert_eq!(graph.edges[0].status, "partially_applied");
    assert!(graph.edges[0].convergence_blocked);
    assert_eq!(
        graph.edges[0].offline_client_ids,
        vec!["right-b".to_string()]
    );
    assert_eq!(
        graph.edges[0].server_drift_reasons,
        vec!["endpoint_not_connected:right-b:stale".to_string()]
    );
    assert_eq!(graph.edges[0].sample_count, 2);
    assert_eq!(graph.edges[0].degraded_count, 1);
    assert_eq!(graph.edges[0].latency_avg_ms, Some(42.0));
    assert_eq!(graph.edges[0].packet_loss_avg_ratio, Some(0.04));
    assert_eq!(graph.edges[0].throughput_avg_mbps, Some(80.0));
    assert!(graph.edges[0].cost_delta.is_some());
    let left = graph
        .nodes
        .iter()
        .find(|node| node.client_id == "left-a")
        .unwrap();
    assert_eq!(left.tunnel_count, 1);
    assert_eq!(left.degraded_tunnel_count, 1);
}

#[tokio::test]
async fn topology_graph_marks_offline_runtime_endpoint_without_agent_observation() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "left-a".to_string(),
                display_name: "left-a".to_string(),
                status: "connected".to_string(),
                tags: vec!["bgp".to_string()],
                capabilities: Default::default(),
            },
            AgentView {
                id: "right-b".to_string(),
                display_name: "right-b".to_string(),
                status: "offline".to_string(),
                tags: vec!["bgp".to_string()],
                capabilities: Default::default(),
            },
        ]);
    }
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let plan = test_plan();
    let input = TunnelPlanInput {
        name: plan.name.clone(),
        interface_name: plan.interface_name.clone(),
        kind: plan.kind,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: plan.left_client_id.clone(),
        right_client_id: plan.right_client_id.clone(),
        left_underlay: plan.left_underlay.clone(),
        right_underlay: plan.right_underlay.clone(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 18.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    };
    repo.record_tunnel_plan(&input, &plan, &operator)
        .await
        .unwrap();

    let graph = repo.topology_graph(10).await.unwrap();

    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].health, "degraded");
    assert!(graph.edges[0].convergence_blocked);
    assert_eq!(graph.edges[0].sample_count, 0);
    assert_eq!(graph.edges[0].degraded_count, 0);
    assert_eq!(
        graph.edges[0].offline_client_ids,
        vec!["right-b".to_string()]
    );
    assert_eq!(
        graph.edges[0].server_drift_reasons,
        vec!["endpoint_not_connected:right-b:offline".to_string()]
    );
    let right = graph
        .nodes
        .iter()
        .find(|node| node.client_id == "right-b")
        .unwrap();
    assert_eq!(right.degraded_tunnel_count, 1);
}

#[tokio::test]
async fn topology_graph_exposes_runtime_status_coverage_and_drift_policy() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "left-a".to_string(),
                display_name: "left-a".to_string(),
                status: "connected".to_string(),
                tags: vec!["bgp".to_string()],
                capabilities: Default::default(),
            },
            AgentView {
                id: "right-b".to_string(),
                display_name: "right-b".to_string(),
                status: "connected".to_string(),
                tags: vec!["bgp".to_string()],
                capabilities: Default::default(),
            },
        ]);
    }
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let plan = test_plan();
    repo.record_tunnel_plan(
        &TunnelPlanInput {
            name: plan.name.clone(),
            interface_name: plan.interface_name.clone(),
            kind: plan.kind,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: plan.left_client_id.clone(),
            right_client_id: plan.right_client_id.clone(),
            left_underlay: plan.left_underlay.clone(),
            right_underlay: plan.right_underlay.clone(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 18.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        },
        &plan,
        &operator,
    )
    .await
    .unwrap();

    let job_id = Uuid::new_v4();
    repo.record_network_observations(
        job_id,
        "left-a",
        &[CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "network_status",
                "plan": "edge-a-edge-b",
                "interface": "tunab",
                "peer_client_id": "right-b",
                "runtime": {
                    "summary": {
                        "status": "drift",
                        "healthy": false,
                        "reasons": ["desired_interface_missing", "stale_interface_present"],
                        "adapter_state": "not_applicable",
                        "bird2_state": "routing_unhealthy",
                        "kernel_link_probe_state": "success",
                        "neighbor_probe_state": "failed",
                        "route_probe_state": "skipped",
                        "real_kernel_namespace_covered": true,
                        "desired_missing_count": 1,
                        "stale_present_count": 1,
                        "external_import_candidate_count": 0
                    }
                }
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        }],
    )
    .await
    .unwrap();

    let graph = repo.topology_graph(10).await.unwrap();
    let edge = &graph.edges[0];

    assert_eq!(edge.health, "degraded");
    assert_eq!(
        edge.topology_drift_policy,
        "observe_runtime_drift_before_apply"
    );
    assert_eq!(edge.topology_drift_action, "inspect_runtime_status");
    assert_eq!(edge.runtime_state, "drift");
    assert_eq!(
        edge.runtime_reasons,
        vec![
            "desired_interface_missing".to_string(),
            "stale_interface_present".to_string()
        ]
    );
    assert_eq!(edge.adapter_state, "not_applicable");
    assert_eq!(edge.routing_state, "routing_unhealthy");
    assert_eq!(edge.kernel_link_probe_state, "success");
    assert_eq!(edge.kernel_neighbor_probe_state, "failed");
    assert_eq!(edge.kernel_route_probe_state, "skipped");
    assert!(edge.kernel_namespace_covered);
    assert_eq!(edge.desired_missing_count, 1);
    assert_eq!(edge.stale_present_count, 1);
    assert_eq!(edge.import_candidate_count, 0);
}

#[tokio::test]
async fn recommends_ospf_cost_from_probe_and_speed_trends() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let plan = test_plan();
    repo.record_tunnel_plan(
        &TunnelPlanInput {
            name: plan.name.clone(),
            interface_name: plan.interface_name.clone(),
            kind: plan.kind,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: plan.left_client_id.clone(),
            right_client_id: plan.right_client_id.clone(),
            left_underlay: plan.left_underlay.clone(),
            right_underlay: plan.right_underlay.clone(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 18.0,
            packet_loss_ratio: 0.0,
            preference: 0.5,
            ospf_policy: OspfCostPolicy::default(),
        },
        &plan,
        &operator,
    )
    .await
    .unwrap();
    let job_id = Uuid::new_v4();
    repo.record_network_observations(
        job_id,
        "left-a",
        &[
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "network_probe",
                    "plan": "edge-a-edge-b",
                    "interface": "tunab",
                    "peer_client_id": "right-b",
                    "target": "10.255.0.1",
                    "parsed": {
                        "healthy": false,
                        "latency_avg_ms": 80.0,
                        "packet_loss_ratio": 0.05
                    }
                }))
                .unwrap(),
                exit_code: Some(0),
                done: true,
            },
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "network_speed_test",
                    "role": "client",
                    "plan": "edge-a-edge-b",
                    "interface": "tunab",
                    "peer_client_id": "right-b",
                    "server_address": "10.255.0.1",
                    "port": 5201,
                    "success": true,
                    "bytes": 1048576,
                    "throughput_mbps": 40.0
                }))
                .unwrap(),
                exit_code: Some(0),
                done: true,
            },
        ],
    )
    .await
    .unwrap();

    let recommendations = repo.list_network_ospf_recommendations(10).await.unwrap();
    let recommendation = recommendations
        .iter()
        .find(|item| item.plan_name == "edge-a-edge-b")
        .unwrap();

    assert_eq!(recommendation.confidence, "measured");
    assert_eq!(recommendation.configured_bandwidth, "100m");
    assert_eq!(recommendation.effective_bandwidth, "10m");
    assert_eq!(recommendation.latency_avg_ms, Some(80.0));
    assert_eq!(recommendation.packet_loss_avg_ratio, Some(0.05));
    assert_eq!(recommendation.throughput_avg_mbps, Some(40.0));
    let (expected_cost, _) = observed_ospf_cost(
        OspfCostPolicy::default(),
        BandwidthTier::M100,
        80.0,
        0.05,
        0.5,
        Some(40.0),
    );
    assert_eq!(recommendation.recommended_ospf_cost, expected_cost as i32);
    assert!(recommendation.recommended_ospf_cost > recommendation.plan_ospf_cost);
    assert!(recommendation.cost_delta > 0);
}

fn test_plan() -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "tunab".to_string(),
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
        latency_ms: 18.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .unwrap()
}

fn test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: Some(Arc::new(SigningKey::from_bytes(&[7_u8; 32]))),
        enrollment: EnrollmentSettings::default(),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    }
}
