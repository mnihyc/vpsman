use super::*;

use axum::{extract::State, http::StatusCode, Json};
use tokio::sync::broadcast;
use vpsman_common::{
    payload_hash, plan_tunnel, render_tunnel_endpoint_config, AgentHello, BandwidthTier,
    JobCommand, OspfCostPolicy, TunnelEndpointSide, TunnelKind, TunnelPlan, TunnelPlanInput,
};

use crate::{
    gateway_client::GatewayDispatchClient, job_request::validate_job_command,
    routes_jobs::create_job,
};

#[test]
fn network_ospf_cost_update_validation_rejects_noop_or_bad_hash() {
    let mut plan = test_plan();
    let current_ospf_cost = plan.recommended_ospf_cost;
    let recommended_ospf_cost = current_ospf_cost.saturating_add(10);
    plan.recommended_ospf_cost = recommended_ospf_cost;
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let command = JobCommand::NetworkOspfCostUpdate {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        current_ospf_cost,
        recommended_ospf_cost,
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    validate_job_command(&command).unwrap();

    let noop = JobCommand::NetworkOspfCostUpdate {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        current_ospf_cost: recommended_ospf_cost,
        recommended_ospf_cost,
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    let error = validate_job_command(&noop).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_ospf_cost_update_noop");

    let bad_hash = JobCommand::NetworkOspfCostUpdate {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        current_ospf_cost,
        recommended_ospf_cost,
        bird2_sha256_hex: "00".repeat(32),
    };
    let error = validate_job_command(&bad_hash).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_ospf_cost_update_bird2_hash_mismatch");

    let mut stale_plan = plan;
    stale_plan.recommended_ospf_cost = recommended_ospf_cost.saturating_add(1);
    let stale = JobCommand::NetworkOspfCostUpdate {
        plan: Box::new(stale_plan),
        side: TunnelEndpointSide::Left,
        current_ospf_cost,
        recommended_ospf_cost,
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    let error = validate_job_command(&stale).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_ospf_cost_update_plan_cost_mismatch");
}

#[tokio::test]
async fn network_ospf_cost_update_create_job_rejects_wrong_side_target() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "right-b".to_string(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: Default::default(),
            },
        )
        .await;
    }

    let mut plan = test_plan();
    let current_ospf_cost = plan.recommended_ospf_cost;
    let recommended_ospf_cost = current_ospf_cost.saturating_add(10);
    plan.recommended_ospf_cost = recommended_ospf_cost;
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:right-b".to_string(),
        target_client_ids: vec!["right-b".to_string()],
        destructive: true,
        confirmed: true,
        command: "network_ospf_cost_update".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::NetworkOspfCostUpdate {
            plan: Box::new(plan),
            side: TunnelEndpointSide::Left,
            current_ospf_cost,
            recommended_ospf_cost,
            bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        }),
        timeout_secs: Some(60),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };
    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let error = create_job(State(state), headers, Json(request))
        .await
        .unwrap_err();

    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_apply_target_mismatch");
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
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
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
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}
