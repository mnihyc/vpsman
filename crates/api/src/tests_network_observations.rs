use super::*;

use axum::{
    extract::{Query, State},
    Json,
};
use tokio::sync::broadcast;
use vpsman_common::{
    observed_ospf_cost, plan_tunnel, CommandOutput, OspfCostPolicy, OutputStream, TunnelKind,
    TunnelPlan, TunnelPlanInput,
};

use crate::gateway_client::GatewayDispatchClient;

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
async fn memory_network_observation_ordering_handles_mixed_timestamp_formats() {
    let repo = Repository::Memory(MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    let plan_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let observation = |id: Uuid, observed_at: &str| NetworkObservationView {
        id,
        job_id,
        client_id: "left-a".to_string(),
        seq: 0,
        kind: "network_probe".to_string(),
        role: None,
        plan_id: Some(plan_id),
        topology_identity_hash: Some("mixed-time".to_string()),
        plan_name: Some("mixed-time".to_string()),
        interface_name: Some("tun-mixed".to_string()),
        peer_client_id: Some("right-b".to_string()),
        target: Some("192.0.2.1".to_string()),
        healthy: Some(true),
        latency_avg_ms: Some(1.0),
        packet_loss_ratio: Some(0.0),
        throughput_mbps: None,
        bytes: None,
        metadata: serde_json::json!({}),
        observed_at: observed_at.to_string(),
    };
    memory.network_observations.write().await.extend([
        observation(Uuid::new_v4(), "999"),
        observation(Uuid::new_v4(), "1970-01-01T00:16:40Z"),
    ]);

    let latest = repo.list_network_observations(1).await.unwrap();
    assert_eq!(latest[0].observed_at, "1970-01-01T00:16:40Z");

    let capped = repo
        .list_network_observations_for_plans_since(&[plan_id], 0, 1, 1)
        .await
        .unwrap();
    assert_eq!(capped.len(), 1);
    assert_eq!(capped[0].observed_at, "1970-01-01T00:16:40Z");

    let trends = repo.list_network_observation_trends(10).await.unwrap();
    assert_eq!(trends.len(), 1);
    assert_eq!(trends[0].latest_observed_at, "1970-01-01T00:16:40Z");
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
                status: "online".to_string(),
                tags: vec!["bgp".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: Default::default(),
            },
            AgentView {
                id: "right-b".to_string(),
                display_name: "right-b".to_string(),
                status: "stale".to_string(),
                tags: vec!["bgp".to_string(), "provider:test".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: Default::default(),
            },
            topology_test_agent("unrelated-c", "online"),
        ]);
    }
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
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
        left_remote_underlay: plan.left_remote_underlay.clone(),
        right_remote_underlay: plan.right_remote_underlay.clone(),
        left_local_underlay: None,
        right_local_underlay: None,
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
        bandwidth_mbps: 100,
        ospf: Some(test_ospf(1.0)),
    };
    repo.record_tunnel_plan(&input, &plan, true, &operator)
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

    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let Json(graph) = crate::routes_network::get_topology_graph(
        State(state),
        headers,
        Query(HistoryQuery { limit: Some(10) }),
    )
    .await
    .unwrap();

    assert_eq!(graph.nodes.len(), 2);
    assert!(graph
        .nodes
        .iter()
        .all(|node| node.client_id == "left-a" || node.client_id == "right-b"));
    assert!(chrono::DateTime::parse_from_rfc3339(&graph.generated_at).is_ok());
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].plan_name, "edge-a-edge-b");
    assert_eq!(graph.edges[0].health, "degraded");
    assert_eq!(graph.edges[0].left_runtime_state, "unknown");
    assert_eq!(graph.edges[0].right_runtime_state, "stale");
    assert_eq!(
        graph.edges[0].unavailable_client_ids,
        vec!["right-b".to_string()]
    );
    assert_eq!(
        graph.edges[0].availability_reasons,
        vec!["endpoint_not_online:right-b:stale".to_string()]
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
async fn topology_graph_keeps_quiet_plan_evidence_beyond_noisy_global_caps() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            topology_test_agent("left-a", "online"),
            topology_test_agent("right-b", "online"),
            topology_test_agent("quiet-left", "online"),
            topology_test_agent("quiet-right", "online"),
        ]);
    }
    let operator = topology_test_operator();
    let noisy_input = test_plan_input("right-b");
    let noisy_plan = repo
        .record_tunnel_plan(
            &noisy_input,
            &plan_tunnel(&noisy_input).unwrap(),
            true,
            &operator,
        )
        .await
        .unwrap();
    let mut quiet_input = test_plan_input("quiet-right");
    quiet_input.name = "quiet-edge".to_string();
    quiet_input.interface_name = "tunquiet".to_string();
    quiet_input.left_client_id = "quiet-left".to_string();
    let quiet_plan = repo
        .record_tunnel_plan(
            &quiet_input,
            &plan_tunnel(&quiet_input).unwrap(),
            true,
            &operator,
        )
        .await
        .unwrap();
    let noisy_identity =
        crate::repository_network_observations::topology_identity_hash_for_plan(&noisy_plan);
    let quiet_identity =
        crate::repository_network_observations::topology_identity_hash_for_plan(&quiet_plan);
    let probe = |plan: &TunnelPlanView,
                 topology_identity_hash: &str,
                 client_id: String,
                 peer_client_id: &str,
                 healthy: bool,
                 latency_avg_ms: f64,
                 observed_at: &str| NetworkObservationView {
        id: Uuid::new_v4(),
        job_id: Uuid::new_v4(),
        client_id,
        seq: 0,
        kind: "network_probe".to_string(),
        role: None,
        plan_id: Some(plan.id),
        topology_identity_hash: Some(topology_identity_hash.to_string()),
        plan_name: Some(plan.name.clone()),
        interface_name: Some(plan.plan.interface_name.clone()),
        peer_client_id: Some(peer_client_id.to_string()),
        target: None,
        healthy: Some(healthy),
        latency_avg_ms: Some(latency_avg_ms),
        packet_loss_ratio: Some(if healthy { 0.0 } else { 0.1 }),
        throughput_mbps: None,
        bytes: None,
        metadata: serde_json::json!({}),
        observed_at: observed_at.to_string(),
    };
    if let Repository::Memory(memory) = &repo {
        memory
            .network_observations
            .write()
            .await
            .extend((0..1_001).map(|index| {
                probe(
                    &noisy_plan,
                    &noisy_identity,
                    format!("noisy-source-{index:04}"),
                    "right-b",
                    true,
                    5.0,
                    "2000",
                )
            }));
        memory.network_observations.write().await.push(probe(
            &quiet_plan,
            &quiet_identity,
            "quiet-left".to_string(),
            "quiet-right",
            false,
            42.0,
            "1000",
        ));

        let mut noisy_telemetry = topology_test_tunnel(noisy_plan.id, "left-a", "left", "ok", None);
        noisy_telemetry.observed_at = "2000".to_string();
        memory
            .telemetry_tunnels
            .write()
            .await
            .extend((0..1_001).map(|_| noisy_telemetry.clone()));
        let mut quiet_telemetry = noisy_telemetry;
        quiet_telemetry.client_id = "quiet-left".to_string();
        quiet_telemetry.observed_at = "1000".to_string();
        quiet_telemetry.interface = quiet_plan.plan.interface_name.clone();
        quiet_telemetry.plan_id = Some(quiet_plan.id);
        quiet_telemetry.plan_name = Some(quiet_plan.name.clone());
        quiet_telemetry.peer_client_id = Some("quiet-right".to_string());
        quiet_telemetry.traffic_status = Some("degraded".to_string());
        quiet_telemetry.traffic_reason = Some("quiet_link_degraded".to_string());
        memory.telemetry_tunnels.write().await.push(quiet_telemetry);
    }

    let graph = repo.topology_graph(24).await.unwrap();
    let quiet_edge = graph
        .edges
        .iter()
        .find(|edge| edge.plan_id == quiet_plan.id)
        .unwrap();

    assert_eq!(quiet_edge.sample_count, 1);
    assert_eq!(quiet_edge.degraded_count, 1);
    assert_eq!(quiet_edge.latency_avg_ms, Some(42.0));
    assert_eq!(quiet_edge.latency_series_ms, vec![42.0]);
    assert_eq!(quiet_edge.probe_state, "degraded");
    assert_eq!(quiet_edge.left_runtime_state, "degraded");
    assert_eq!(
        quiet_edge.left_runtime_reason.as_deref(),
        Some("quiet_link_degraded")
    );
}

#[tokio::test]
async fn topology_graph_ignores_observations_from_reused_plan_name_with_different_identity() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "left-a".to_string(),
                display_name: "left-a".to_string(),
                status: "online".to_string(),
                tags: vec!["bgp".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: Default::default(),
            },
            AgentView {
                id: "right-b".to_string(),
                display_name: "right-b".to_string(),
                status: "online".to_string(),
                tags: vec!["bgp".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: Default::default(),
            },
            AgentView {
                id: "right-c".to_string(),
                display_name: "right-c".to_string(),
                status: "online".to_string(),
                tags: vec!["bgp".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: Default::default(),
            },
        ]);
    }
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Uuid::nil(),
    };
    let original = test_plan();
    let saved = repo
        .record_tunnel_plan(&test_plan_input("right-b"), &original, true, &operator)
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
                "type": "network_probe",
                "plan": "edge-a-edge-b",
                "interface": "tunab",
                "peer_client_id": "right-b",
                "target": "10.255.0.1",
                "parsed": {
                    "healthy": false,
                    "latency_avg_ms": 75.0,
                    "packet_loss_ratio": 0.05
                }
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        }],
    )
    .await
    .unwrap();
    let old_observation = repo.list_network_observations(10).await.unwrap().remove(0);
    assert!(old_observation.plan_id.is_some());
    assert!(old_observation.topology_identity_hash.is_some());

    let replacement = plan_tunnel(&test_plan_input("right-c")).unwrap();
    repo.update_tunnel_plan(
        saved.id,
        saved.revision,
        &test_plan_input("right-c"),
        &replacement,
        true,
        &operator,
    )
    .await
    .unwrap();

    let graph = repo.topology_graph(10).await.unwrap();
    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].right_client_id, "right-c");
    assert_eq!(graph.edges[0].sample_count, 0);
    assert_eq!(graph.edges[0].degraded_count, 0);
    assert_eq!(graph.edges[0].latency_avg_ms, None);
    assert_ne!(
        old_observation.topology_identity_hash.as_deref(),
        Some(graph.edges[0].topology_identity_hash.as_str())
    );
}

#[tokio::test]
async fn topology_graph_marks_offline_runtime_endpoint_without_agent_observation() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "left-a".to_string(),
                display_name: "left-a".to_string(),
                status: "online".to_string(),
                tags: vec!["bgp".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: Default::default(),
            },
            AgentView {
                id: "right-b".to_string(),
                display_name: "right-b".to_string(),
                status: "offline".to_string(),
                tags: vec!["bgp".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: Default::default(),
            },
        ]);
    }
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
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
        left_remote_underlay: plan.left_remote_underlay.clone(),
        right_remote_underlay: plan.right_remote_underlay.clone(),
        left_local_underlay: None,
        right_local_underlay: None,
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
        bandwidth_mbps: 100,
        ospf: Some(test_ospf(1.0)),
    };
    repo.record_tunnel_plan(&input, &plan, true, &operator)
        .await
        .unwrap();

    let graph = repo.topology_graph(10).await.unwrap();

    assert_eq!(graph.edges.len(), 1);
    assert_eq!(graph.edges[0].health, "degraded");
    assert_eq!(graph.edges[0].right_runtime_state, "degraded");
    assert_eq!(graph.edges[0].sample_count, 0);
    assert_eq!(graph.edges[0].degraded_count, 0);
    assert_eq!(
        graph.edges[0].unavailable_client_ids,
        vec!["right-b".to_string()]
    );
    assert_eq!(
        graph.edges[0].availability_reasons,
        vec!["endpoint_not_online:right-b:offline".to_string()]
    );
    let right = graph
        .nodes
        .iter()
        .find(|node| node.client_id == "right-b")
        .unwrap();
    assert_eq!(right.degraded_tunnel_count, 1);
}

#[tokio::test]
async fn topology_graph_uses_exact_plan_bound_endpoint_telemetry() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            topology_test_agent("left-a", "online"),
            topology_test_agent("right-b", "online"),
        ]);
    }
    let operator = topology_test_operator();
    let plan = test_plan();
    let input = test_plan_input("right-b");
    let saved = repo
        .record_tunnel_plan(&input, &plan, true, &operator)
        .await
        .unwrap();
    if let Repository::Memory(memory) = &repo {
        memory.telemetry_tunnels.write().await.extend([
            topology_test_tunnel(saved.id, "left-a", "left", "ok", None),
            topology_test_tunnel(
                saved.id,
                "right-b",
                "right",
                "missing",
                Some("tunab_not_found"),
            ),
        ]);
    }

    let graph = repo.topology_graph(10).await.unwrap();
    let edge = &graph.edges[0];
    assert_eq!(edge.left_runtime_state, "healthy");
    assert_eq!(edge.right_runtime_state, "degraded");
    assert_eq!(
        edge.right_runtime_reason.as_deref(),
        Some("tunab_not_found")
    );
    assert_eq!(edge.health, "degraded");

    repo.set_tunnel_plan_enabled(saved.id, saved.revision, false, &operator)
        .await
        .unwrap();
    let disabled = repo.topology_graph(10).await.unwrap();
    assert_eq!(disabled.edges[0].health, "disabled");
    assert_eq!(disabled.edges[0].left_runtime_state, "disabled");
    assert_eq!(disabled.edges[0].right_runtime_state, "disabled");
}

#[tokio::test]
async fn failed_latency_probe_does_not_reclassify_converged_runtime_as_failed() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            topology_test_agent("left-a", "online"),
            topology_test_agent("right-b", "online"),
        ]);
    }
    let operator = topology_test_operator();
    let plan = test_plan();
    let input = test_plan_input("right-b");
    let saved = repo
        .record_tunnel_plan(&input, &plan, true, &operator)
        .await
        .unwrap();
    let mut left = topology_test_tunnel(saved.id, "left-a", "left", "ok", None);
    left.latency_monitoring_enabled = Some(true);
    left.latency_status = Some("healthy".to_string());
    let mut right = topology_test_tunnel(saved.id, "right-b", "right", "ok", None);
    right.latency_monitoring_enabled = Some(true);
    right.latency_status = Some("failed".to_string());
    right.latency_reason = Some("icmp_blocked_or_unreachable".to_string());
    if let Repository::Memory(memory) = &repo {
        memory.telemetry_tunnels.write().await.extend([left, right]);
    }

    let graph = repo.topology_graph(10).await.unwrap();
    let edge = &graph.edges[0];
    assert_eq!(edge.left_runtime_state, "healthy");
    assert_eq!(edge.right_runtime_state, "healthy");
    assert_eq!(edge.left_reachability_state, "reachable");
    assert_eq!(edge.right_reachability_state, "probe_failed");
    assert_eq!(
        edge.right_reachability_reason.as_deref(),
        Some("icmp_blocked_or_unreachable")
    );
}

#[tokio::test]
async fn topology_graph_exposes_explicit_runtime_status_coverage() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "left-a".to_string(),
                display_name: "left-a".to_string(),
                status: "online".to_string(),
                tags: vec!["bgp".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: Default::default(),
            },
            AgentView {
                id: "right-b".to_string(),
                display_name: "right-b".to_string(),
                status: "online".to_string(),
                tags: vec!["bgp".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: Default::default(),
            },
        ]);
    }
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
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
            left_remote_underlay: plan.left_remote_underlay.clone(),
            right_remote_underlay: plan.right_remote_underlay.clone(),
            left_local_underlay: None,
            right_local_underlay: None,
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
            bandwidth_mbps: 100,
            ospf: Some(test_ospf(1.0)),
        },
        &plan,
        true,
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
                        "kernel_link_probe_state": "success",
                        "neighbor_probe_state": "failed",
                        "route_probe_state": "skipped",
                        "real_kernel_namespace_covered": true,
                        "desired_missing_count": 1,
                        "stale_present_count": 1
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
    assert_eq!(edge.runtime_state, "drift");
    assert_eq!(
        edge.runtime_reasons,
        vec![
            "desired_interface_missing".to_string(),
            "stale_interface_present".to_string()
        ]
    );
    assert_eq!(edge.adapter_state, "not_applicable");
    assert_eq!(edge.routing_state, "unknown");
    assert_eq!(edge.kernel_link_probe_state, "success");
    assert_eq!(edge.kernel_neighbor_probe_state, "failed");
    assert_eq!(edge.kernel_route_probe_state, "skipped");
    assert!(edge.kernel_namespace_covered);
    assert_eq!(edge.desired_missing_count, 1);
    assert_eq!(edge.stale_present_count, 1);
}

#[tokio::test]
async fn recommends_ospf_cost_from_probe_and_speed_trends() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
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
            left_remote_underlay: plan.left_remote_underlay.clone(),
            right_remote_underlay: plan.right_remote_underlay.clone(),
            left_local_underlay: None,
            right_local_underlay: None,
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
            bandwidth_mbps: 100,
            ospf: Some(test_ospf(0.5)),
        },
        &plan,
        true,
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
    assert_eq!(recommendation.configured_bandwidth_mbps, 100);
    assert_eq!(recommendation.effective_bandwidth_mbps, 40);
    assert_eq!(recommendation.latency_avg_ms, Some(80.0));
    assert_eq!(recommendation.packet_loss_avg_ratio, Some(0.05));
    assert_eq!(recommendation.throughput_avg_mbps, Some(40.0));
    let (expected_cost, _) =
        observed_ospf_cost(OspfCostPolicy::default(), 100, 80.0, 0.05, 0.5, Some(40.0));
    assert_eq!(recommendation.recommended_ospf_cost, expected_cost as i32);
    assert!(recommendation.recommended_ospf_cost > recommendation.plan_ospf_cost);
    assert!(recommendation.cost_delta > 0);
}

fn test_plan() -> TunnelPlan {
    plan_tunnel(&test_plan_input("right-b")).unwrap()
}

fn test_plan_input(right_client_id: &str) -> TunnelPlanInput {
    TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "tunab".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: right_client_id.to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
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
        bandwidth_mbps: 100,
        ospf: Some(test_ospf(1.0)),
    }
}

fn test_ospf(preference: f64) -> vpsman_common::TunnelOspfConfig {
    vpsman_common::TunnelOspfConfig {
        mode: vpsman_common::OspfControlMode::Reviewed,
        planned_latency_ms: 18.0,
        planned_packet_loss_ratio: 0.0,
        preference,
        policy: OspfCostPolicy::default(),
        min_cost_delta: 5,
        healthy_windows: 2,
        left_adapter_template_id: "33333333-3333-4333-8333-333333333333".to_string(),
        right_adapter_template_id: "44444444-4444-4444-8444-444444444444".to_string(),
    }
}

fn topology_test_agent(id: &str, status: &str) -> AgentView {
    AgentView {
        id: id.to_string(),
        display_name: id.to_string(),
        status: status.to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: Default::default(),
    }
}

fn topology_test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Uuid::nil(),
    }
}

fn topology_test_tunnel(
    plan_id: Uuid,
    client_id: &str,
    endpoint_side: &str,
    traffic_status: &str,
    traffic_reason: Option<&str>,
) -> crate::model::TelemetryTunnelView {
    crate::model::TelemetryTunnelView {
        client_id: client_id.to_string(),
        observed_at: crate::unix_now().to_string(),
        interface: "tunab".to_string(),
        kind: "gre".to_string(),
        ownership_mode: "agent_iproute2_managed".to_string(),
        mutation_policy: "managed_desired".to_string(),
        plan_id: Some(plan_id),
        plan_name: Some("edge-a-edge-b".to_string()),
        plan_runtime_manager: Some("agent_iproute2_managed".to_string()),
        endpoint_side: Some(endpoint_side.to_string()),
        peer_client_id: Some(
            if endpoint_side == "left" {
                "right-b"
            } else {
                "left-a"
            }
            .to_string(),
        ),
        source: "approved_runtime_status_telemetry".to_string(),
        operstate: None,
        mtu: None,
        link_type: None,
        address: None,
        rx_bytes: 1,
        tx_bytes: 1,
        traffic_source: Some("interface_counters".to_string()),
        traffic_status: Some(traffic_status.to_string()),
        traffic_reason: traffic_reason.map(str::to_string),
        traffic_checked_unix: Some(crate::unix_now() as i64),
        adapter_health: None,
        latency_monitoring_enabled: Some(false),
        latency_status: Some("disabled".to_string()),
        latency_reason: None,
        latency_primary_family: Some("ipv4".to_string()),
        latency_target: None,
        latency_checked_unix: None,
        latency_avg_ms: None,
        packet_loss_ratio: None,
        latency_healthy_windows: None,
        latency_missed_windows: None,
    }
}

fn test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}
