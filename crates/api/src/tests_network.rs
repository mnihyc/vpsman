use super::*;

use axum::{extract::State, http::StatusCode, Json};
use tokio::sync::broadcast;
use vpsman_common::{
    backend_config_signature_payload, payload_hash, plan_tunnel,
    render_tunnel_endpoint_backend_config, render_tunnel_endpoint_config, AgentCapabilitySnapshot,
    AgentHello, AgentPrivilegeMode, BandwidthTier, JobCommand, OspfCostPolicy,
    RuntimeTunnelCommand, RuntimeTunnelControl, RuntimeTunnelManager, RuntimeTunnelRoute,
    RuntimeTunnelTopologyIntent, TunnelConfigBackend, TunnelEndpointSide, TunnelKind, TunnelPlan,
    TunnelPlanInput, CURRENT_COMMAND_PROTOCOL_VERSION, MANAGED_BIRD2_FILE,
    MIN_COMMAND_PROTOCOL_VERSION,
};

use crate::{
    gateway_client::GatewayDispatchClient,
    job_request::{
        job_command_min_supported_protocol_version, job_command_protocol_version,
        validate_job_command,
    },
    routes_jobs::create_job,
};

#[tokio::test]
async fn tunnel_plan_records_non_mutating_plan_and_audit() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let input = TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "tun-ab".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "client-a".to_string(),
        right_client_id: "client-b".to_string(),
        left_underlay: "203.0.113.1".to_string(),
        right_underlay: "203.0.113.2".to_string(),
        address_pool_cidr: "10.10.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.10.0.0".to_string(),
            right: "10.10.0.1".to_string(),
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
    };
    let plan = plan_tunnel(&input).unwrap();
    let view = repo
        .record_tunnel_plan(&input, &plan, &operator)
        .await
        .unwrap();
    let plans = repo.list_tunnel_plans().await.unwrap();
    let audits = repo.list_audit_logs(10).await.unwrap();

    assert_eq!(view.name, "edge-a-edge-b");
    assert_eq!(view.kind, TunnelKind::Gre);
    assert!(!view.plan.mutates_host);
    assert_eq!(plans.len(), 1);
    assert_eq!(audits[0].action, "network.tunnel_plan_created");
    assert_eq!(audits[0].metadata["mutates_host"], false);
}

#[tokio::test]
async fn allocate_tunnel_endpoints_skips_existing_plan_addresses() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let mut input = test_plan_input();
    input.address_pool_cidr = "10.10.0.0/29".to_string();
    input.ipv4_tunnel = Some(vpsman_common::TunnelAddressPair {
        left: "10.10.0.0".to_string(),
        right: "10.10.0.1".to_string(),
        prefix_len: 31,
    });
    let plan = plan_tunnel(&input).unwrap();
    repo.record_tunnel_plan(&input, &plan, &operator)
        .await
        .unwrap();

    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let Json(allocation) = crate::routes_network::allocate_tunnel_endpoints(
        State(state),
        headers,
        Json(AllocateTunnelEndpointsRequest {
            ipv4_pool_cidr: Some("10.10.0.0/29".to_string()),
            ipv6_pool_cidr: Some("fd00:10::/126".to_string()),
            reserved_addresses: Vec::new(),
            include_ipv4: true,
            include_ipv6: true,
        }),
    )
    .await
    .unwrap();

    let ipv4 = allocation.ipv4_tunnel.expect("ipv4");
    let ipv6 = allocation.ipv6_tunnel.expect("ipv6");
    assert_eq!(ipv4.left, "10.10.0.2");
    assert_eq!(ipv4.right, "10.10.0.3");
    assert_eq!(ipv4.prefix_len, 31);
    assert_eq!(ipv6.left, "fd00:10::");
    assert_eq!(ipv6.right, "fd00:10::1");
    assert_eq!(ipv6.prefix_len, 127);
}

#[tokio::test]
async fn deleting_agent_soft_deletes_tunnel_plans_for_either_endpoint() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "network-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
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

    let endpoint_as_right = test_plan_input();
    let endpoint_as_right_plan = plan_tunnel(&endpoint_as_right).unwrap();
    repo.record_tunnel_plan(&endpoint_as_right, &endpoint_as_right_plan, &operator)
        .await
        .unwrap();

    let mut endpoint_as_left = test_plan_input();
    endpoint_as_left.name = "edge-b-edge-c".to_string();
    endpoint_as_left.interface_name = "tunbc".to_string();
    endpoint_as_left.left_client_id = "right-b".to_string();
    endpoint_as_left.right_client_id = "edge-c".to_string();
    endpoint_as_left.left_underlay = "203.0.113.20".to_string();
    endpoint_as_left.right_underlay = "192.0.2.30".to_string();
    endpoint_as_left.address_pool_cidr = "10.255.0.4/31".to_string();
    endpoint_as_left.ipv4_tunnel = Some(vpsman_common::TunnelAddressPair {
        left: "10.255.0.4".to_string(),
        right: "10.255.0.5".to_string(),
        prefix_len: 31,
    });
    let endpoint_as_left_plan = plan_tunnel(&endpoint_as_left).unwrap();
    repo.record_tunnel_plan(&endpoint_as_left, &endpoint_as_left_plan, &operator)
        .await
        .unwrap();

    let mut survivor = test_plan_input();
    survivor.name = "edge-c-edge-d".to_string();
    survivor.interface_name = "tuncd".to_string();
    survivor.left_client_id = "edge-c".to_string();
    survivor.right_client_id = "edge-d".to_string();
    survivor.left_underlay = "192.0.2.30".to_string();
    survivor.right_underlay = "192.0.2.40".to_string();
    survivor.address_pool_cidr = "10.255.0.8/31".to_string();
    survivor.ipv4_tunnel = Some(vpsman_common::TunnelAddressPair {
        left: "10.255.0.8".to_string(),
        right: "10.255.0.9".to_string(),
        prefix_len: 31,
    });
    let survivor_plan = plan_tunnel(&survivor).unwrap();
    repo.record_tunnel_plan(&survivor, &survivor_plan, &operator)
        .await
        .unwrap();

    repo.delete_agent(
        "right-b",
        &DeleteAgentRequest {
            confirmed: true,
            reason: Some("decommissioned peer".to_string()),
        },
        &operator,
    )
    .await
    .unwrap();

    let active_names = repo
        .list_tunnel_plans()
        .await
        .unwrap()
        .into_iter()
        .map(|plan| plan.name)
        .collect::<Vec<_>>();
    assert_eq!(active_names, vec!["edge-c-edge-d".to_string()]);

    if let Repository::Memory(memory) = &repo {
        let plans = memory.tunnel_plans.read().await;
        let deleted = plans
            .iter()
            .filter(|plan| plan.deleted_at.is_some())
            .map(|plan| plan.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(deleted, vec!["edge-a-edge-b", "edge-b-edge-c"]);
        for plan in plans.iter().filter(|plan| plan.deleted_at.is_some()) {
            assert_eq!(plan.deleted_by, Some(operator.operator.id));
            assert!(!plan.enabled);
            assert!(plan
                .deleted_reason
                .as_deref()
                .unwrap_or_default()
                .contains("endpoint_vps_deleted:right-b"));
        }
        let survivor = plans
            .iter()
            .find(|plan| plan.name == "edge-c-edge-d")
            .unwrap();
        assert!(survivor.deleted_at.is_none());
        let audits = memory.audits.read().await;
        let deleted_audit = audits
            .iter()
            .find(|audit| audit.action == "agent.deleted")
            .unwrap();
        assert_eq!(
            deleted_audit.metadata["soft_deleted_tunnel_plan_count"].as_u64(),
            Some(2)
        );
    }
}

#[tokio::test]
async fn tunnel_plan_enabled_state_is_explicit_and_controls_ospf_recommendations() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "network-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let input = test_plan_input();
    let plan = plan_tunnel(&input).unwrap();
    let created = repo
        .record_tunnel_plan(&input, &plan, &operator)
        .await
        .unwrap();
    assert!(created.enabled);
    assert_eq!(
        repo.list_network_ospf_recommendations(10)
            .await
            .unwrap()
            .len(),
        1
    );

    let disabled = repo
        .set_tunnel_plan_enabled(created.id, false, &operator)
        .await
        .unwrap();
    assert!(!disabled.enabled);
    let visible = repo.list_tunnel_plans().await.unwrap();
    assert_eq!(visible.len(), 1);
    assert!(!visible[0].enabled);
    assert!(repo
        .list_network_ospf_recommendations(10)
        .await
        .unwrap()
        .is_empty());

    let edited_plan = plan_tunnel(&input).unwrap();
    let edited = repo
        .record_tunnel_plan(&input, &edited_plan, &operator)
        .await
        .unwrap();
    assert!(!edited.enabled);

    let enabled = repo
        .set_tunnel_plan_enabled(created.id, true, &operator)
        .await
        .unwrap();
    assert!(enabled.enabled);
    assert_eq!(
        repo.list_network_ospf_recommendations(10)
            .await
            .unwrap()
            .len(),
        1
    );

    if let Repository::Memory(memory) = &repo {
        let actions = memory
            .audits
            .read()
            .await
            .iter()
            .map(|audit| audit.action.clone())
            .collect::<Vec<_>>();
        assert!(actions.contains(&"network.tunnel_plan_disabled".to_string()));
        assert!(actions.contains(&"network.tunnel_plan_enabled".to_string()));
    }
}

#[tokio::test]
async fn create_tunnel_plan_accepts_external_observed_import() {
    let repo = Repository::Memory(MemoryState::default());
    let mut input = test_plan_input();
    input.name = "wg-import".to_string();
    input.interface_name = "wg42".to_string();
    input.kind = TunnelKind::Wireguard;
    input.runtime_control = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalObserved,
        ..RuntimeTunnelControl::default()
    };
    input.runtime_topology = RuntimeTunnelTopologyIntent {
        version: Some("provider-a:42".to_string()),
        desired_interfaces: vec!["wg42".to_string()],
        ..RuntimeTunnelTopologyIntent::default()
    };

    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (status, Json(view)) = crate::routes_network::create_tunnel_plan(
        State(state),
        headers,
        Json(CreateTunnelPlanRequest { input }),
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(view.kind, TunnelKind::Wireguard);
    assert_eq!(
        view.plan.runtime_control.manager,
        RuntimeTunnelManager::ExternalObserved
    );
    assert_eq!(
        view.plan.touched_files,
        vec![MANAGED_BIRD2_FILE.to_string()]
    );
    assert!(view.plan.ifupdown_snippet.contains("external observed"));

    let audits = repo.list_audit_logs(10).await.unwrap();
    assert_eq!(audits[0].metadata["runtime_manager"], "external_observed");
}

#[tokio::test]
async fn create_tunnel_plan_rejects_custom_kind_without_external_runtime_manager() {
    let repo = Repository::Memory(MemoryState::default());
    let mut input = test_plan_input();
    input.name = "custom-bad".to_string();
    input.interface_name = "cust42".to_string();
    input.kind = TunnelKind::Custom;

    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let error = crate::routes_network::create_tunnel_plan(
        State(state),
        headers,
        Json(CreateTunnelPlanRequest { input }),
    )
    .await
    .unwrap_err();

    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "unsupported_tunnel_kind_for_runtime_manager");
}

#[tokio::test]
async fn completed_network_jobs_update_tunnel_plan_endpoint_state() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "network-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    };
    let input = test_plan_input();
    let plan = plan_tunnel(&input).unwrap();
    repo.record_tunnel_plan(&input, &plan, &operator)
        .await
        .unwrap();
    let left = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let left_job = Uuid::new_v4();
    repo.record_tunnel_plan_execution(
        left_job,
        &JobCommand::NetworkApply {
            plan: Box::new(plan.clone()),
            side: TunnelEndpointSide::Left,
            config_backend: TunnelConfigBackend::Ifupdown,
            config_sha256_hex: None,
            ifupdown_sha256_hex: payload_hash(left.ifupdown_snippet.as_bytes()),
            bird2_sha256_hex: payload_hash(left.bird2_interface_snippet.as_bytes()),
        },
        "completed",
    )
    .await
    .unwrap();
    let plans = repo.list_tunnel_plans().await.unwrap();
    assert_eq!(plans[0].left_status, "applied");
    assert_eq!(plans[0].right_status, "planned");
    assert_eq!(plans[0].status, "partially_applied");
    assert_eq!(plans[0].last_apply_job_id, Some(left_job));

    let right = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Right).unwrap();
    repo.record_tunnel_plan_execution(
        Uuid::new_v4(),
        &JobCommand::NetworkApply {
            plan: Box::new(plan.clone()),
            side: TunnelEndpointSide::Right,
            config_backend: TunnelConfigBackend::Ifupdown,
            config_sha256_hex: None,
            ifupdown_sha256_hex: payload_hash(right.ifupdown_snippet.as_bytes()),
            bird2_sha256_hex: payload_hash(right.bird2_interface_snippet.as_bytes()),
        },
        "completed",
    )
    .await
    .unwrap();
    let plans = repo.list_tunnel_plans().await.unwrap();
    assert_eq!(plans[0].left_status, "applied");
    assert_eq!(plans[0].right_status, "applied");
    assert_eq!(plans[0].status, "applied");

    let rollback_job = Uuid::new_v4();
    repo.record_tunnel_plan_execution(
        rollback_job,
        &JobCommand::NetworkRollback {
            plan: Box::new(plan),
            side: TunnelEndpointSide::Left,
        },
        "completed",
    )
    .await
    .unwrap();
    let plans = repo.list_tunnel_plans().await.unwrap();
    assert_eq!(plans[0].left_status, "rolled_back");
    assert_eq!(plans[0].right_status, "applied");
    assert_eq!(plans[0].status, "partially_rolled_back");
    assert_eq!(plans[0].last_rollback_job_id, Some(rollback_job));
    let audits = repo.list_audit_logs(10).await.unwrap();
    assert!(audits
        .iter()
        .any(|audit| audit.action == "network.tunnel_plan_applied"));
    assert!(audits
        .iter()
        .any(|audit| audit.action == "network.tunnel_plan_rolled_back"));
}

async fn wait_for_job_status(
    repo: &crate::repository::Repository,
    job_id: uuid::Uuid,
    expected: &str,
) {
    for _ in 0..50 {
        let jobs = repo.list_jobs(100).await.unwrap();
        if jobs
            .iter()
            .any(|job| job.id == job_id && job.status == expected)
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("job {job_id} did not reach status {expected}");
}

#[test]
fn network_apply_validation_rejects_mutating_plan_or_hash_mismatch() {
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let command = JobCommand::NetworkApply {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    validate_job_command(&command).unwrap();

    let bad_hash = JobCommand::NetworkApply {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    let error = validate_job_command(&bad_hash).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_apply_ifupdown_hash_mismatch");

    let mut mutating_plan = plan;
    mutating_plan.mutates_host = true;
    let mutating = JobCommand::NetworkApply {
        plan: Box::new(mutating_plan),
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    let error = validate_job_command(&mutating).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_apply_plan_must_be_observe_plan");
}

#[test]
fn network_validation_rejects_invalid_runtime_tunnel_control() {
    let mut plan = test_plan();
    plan.runtime_control = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalObserved,
        restart: Some(RuntimeTunnelCommand {
            argv: vec!["/usr/local/libexec/restart-tunnel".to_string()],
            timeout_secs: 10,
            max_output_bytes: 4096,
        }),
        ..RuntimeTunnelControl::default()
    };
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let command = JobCommand::NetworkApply {
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };

    let error = validate_job_command(&command).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_runtime_control_invalid");
}

#[test]
fn network_apply_validation_rejects_invalid_runtime_topology() {
    let mut plan = test_plan();
    plan.runtime_topology = RuntimeTunnelTopologyIntent {
        desired_interfaces: vec!["other0".to_string()],
        ..RuntimeTunnelTopologyIntent::default()
    };
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let command = JobCommand::NetworkApply {
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };

    let error = validate_job_command(&command).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_runtime_topology_invalid");
}

#[test]
fn network_apply_validation_rejects_invalid_runtime_route() {
    let mut plan = test_plan();
    plan.runtime_topology = RuntimeTunnelTopologyIntent {
        desired_interfaces: vec![plan.interface_name.clone()],
        routes: vec![RuntimeTunnelRoute {
            destination_cidr: "not-cidr".to_string(),
            ..RuntimeTunnelRoute::default()
        }],
        ..RuntimeTunnelTopologyIntent::default()
    };
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let command = JobCommand::NetworkApply {
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };

    let error = validate_job_command(&command).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_runtime_route_invalid");
}

#[test]
fn network_apply_validation_requires_backend_specific_config_hash() {
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let backend_config = render_tunnel_endpoint_backend_config(
        &plan,
        TunnelEndpointSide::Left,
        TunnelConfigBackend::Netplan,
    )
    .unwrap();
    let command = JobCommand::NetworkApply {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Netplan,
        config_sha256_hex: Some(payload_hash(&backend_config_signature_payload(
            &backend_config,
        ))),
        ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    validate_job_command(&command).unwrap();

    let missing_hash = JobCommand::NetworkApply {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Netplan,
        config_sha256_hex: None,
        ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    let error = validate_job_command(&missing_hash).unwrap_err();
    assert_eq!(error.code, "network_apply_config_hash_required");

    let bad_hash = JobCommand::NetworkApply {
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Netplan,
        config_sha256_hex: Some("00".repeat(32)),
        ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    let error = validate_job_command(&bad_hash).unwrap_err();
    assert_eq!(error.code, "network_apply_config_hash_mismatch");
}

#[test]
fn network_rollback_validation_rejects_mutating_plan() {
    let mut plan = test_plan();
    let command = JobCommand::NetworkRollback {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
    };
    validate_job_command(&command).unwrap();

    plan.mutates_host = true;
    let command = JobCommand::NetworkRollback {
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
    };
    let error = validate_job_command(&command).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_rollback_plan_must_be_observe_plan");
}

#[test]
fn network_status_validation_rejects_mutating_plan() {
    let mut plan = test_plan();
    let command = JobCommand::NetworkStatus {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
    };
    validate_job_command(&command).unwrap();

    plan.mutates_host = true;
    let command = JobCommand::NetworkStatus {
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
    };
    let error = validate_job_command(&command).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_status_plan_must_be_observe_plan");
}

#[test]
fn network_interfaces_validation_uses_current_protocol() {
    let command = JobCommand::NetworkInterfaces;

    validate_job_command(&command).unwrap();
    assert_eq!(
        job_command_protocol_version(&command),
        CURRENT_COMMAND_PROTOCOL_VERSION
    );
    assert_eq!(
        job_command_min_supported_protocol_version(&command),
        MIN_COMMAND_PROTOCOL_VERSION
    );
}

#[test]
fn network_probe_validation_rejects_mutating_plan_or_unbounded_probe() {
    let mut plan = test_plan();
    let command = JobCommand::NetworkProbe {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        count: 3,
        interval_ms: 500,
    };
    validate_job_command(&command).unwrap();

    let bad_count = JobCommand::NetworkProbe {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        count: 0,
        interval_ms: 500,
    };
    let error = validate_job_command(&bad_count).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_probe_count_out_of_range");

    let bad_interval = JobCommand::NetworkProbe {
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        count: 3,
        interval_ms: 50,
    };
    let error = validate_job_command(&bad_interval).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_probe_interval_ms_out_of_range");

    plan.mutates_host = true;
    let command = JobCommand::NetworkProbe {
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
        count: 3,
        interval_ms: 500,
    };
    let error = validate_job_command(&command).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_probe_plan_must_be_observe_plan");
}

#[test]
fn network_speed_test_validation_rejects_mutating_plan_or_unbounded_budget() {
    let mut plan = test_plan();
    let build_command = |plan: TunnelPlan,
                         duration_secs: u8,
                         max_bytes: u64,
                         rate_limit_kbps: u32,
                         port: u16,
                         connect_timeout_ms: u16| {
        JobCommand::NetworkSpeedTest {
            plan: Box::new(plan),
            server_side: TunnelEndpointSide::Left,
            duration_secs,
            max_bytes,
            rate_limit_kbps,
            port,
            connect_timeout_ms,
        }
    };
    let command = build_command(plan.clone(), 3, 16 * 1024 * 1024, 100_000, 5201, 5000);
    validate_job_command(&command).unwrap();

    let bad_duration = build_command(plan.clone(), 0, 16 * 1024 * 1024, 100_000, 5201, 5000);
    let error = validate_job_command(&bad_duration).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_speed_test_duration_secs_out_of_range");

    let bad_bytes = build_command(plan.clone(), 3, 1, 100_000, 5201, 5000);
    let error = validate_job_command(&bad_bytes).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_speed_test_max_bytes_out_of_range");

    let bad_rate = build_command(plan.clone(), 3, 16 * 1024 * 1024, 0, 5201, 5000);
    let error = validate_job_command(&bad_rate).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error.code,
        "network_speed_test_rate_limit_kbps_out_of_range"
    );

    let bad_port = build_command(plan.clone(), 3, 16 * 1024 * 1024, 100_000, 22, 5000);
    let error = validate_job_command(&bad_port).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_speed_test_port_out_of_range");

    let bad_connect_timeout = build_command(plan.clone(), 3, 16 * 1024 * 1024, 100_000, 5201, 50);
    let error = validate_job_command(&bad_connect_timeout).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error.code,
        "network_speed_test_connect_timeout_ms_out_of_range"
    );

    plan.mutates_host = true;
    let command = build_command(plan, 3, 16 * 1024 * 1024, 100_000, 5201, 5000);
    let error = validate_job_command(&command).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "network_speed_test_plan_must_be_observe_plan");
}

#[tokio::test]
async fn network_apply_create_job_rejects_wrong_side_target() {
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

    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:right-b".to_string(),
        target_client_ids: vec!["right-b".to_string()],
        destructive: true,
        confirmed: true,
        command: "network_apply".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::NetworkApply {
            plan: Box::new(plan),
            side: TunnelEndpointSide::Left,
            config_backend: TunnelConfigBackend::Ifupdown,
            config_sha256_hex: None,
            ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
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

#[tokio::test]
async fn network_apply_degrades_unprivileged_target_after_privilege_verification() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "left-a".to_string(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: AgentCapabilitySnapshot {
                    privilege_mode: AgentPrivilegeMode::Unprivileged,
                    effective_uid: Some(1000),
                    can_attempt_privileged_ops: true,
                    ..Default::default()
                },
            },
        )
        .await;
    }

    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let operation = JobCommand::NetworkApply {
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
        config_backend: TunnelConfigBackend::Ifupdown,
        config_sha256_hex: None,
        ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:left-a".to_string(),
        target_client_ids: vec!["left-a".to_string()],
        destructive: true,
        confirmed: true,
        command: "network_apply".to_string(),
        argv: Vec::new(),
        operation: Some(operation),
        timeout_secs: Some(60),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };
    let state = test_state_with_privilege_auto_approve(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (status, response) = create_job(State(state), headers, Json(request))
        .await
        .unwrap();
    wait_for_job_status(&repo, response.job_id, "partial_success").await;
    let targets = repo.list_job_targets(response.job_id).await.unwrap();

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(response.status, "partial_success");
    assert_eq!(targets[0].status, "skipped");
}

#[tokio::test]
async fn network_rollback_create_job_rejects_wrong_side_target() {
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

    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:right-b".to_string(),
        target_client_ids: vec!["right-b".to_string()],
        destructive: true,
        confirmed: true,
        command: "network_rollback".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::NetworkRollback {
            plan: Box::new(test_plan()),
            side: TunnelEndpointSide::Left,
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

#[tokio::test]
async fn network_status_create_job_rejects_wrong_side_target() {
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

    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:right-b".to_string(),
        target_client_ids: vec!["right-b".to_string()],
        destructive: false,
        confirmed: false,
        command: "network_status".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::NetworkStatus {
            plan: Box::new(test_plan()),
            side: TunnelEndpointSide::Left,
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

#[tokio::test]
async fn network_probe_create_job_rejects_wrong_side_target() {
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

    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:right-b".to_string(),
        target_client_ids: vec!["right-b".to_string()],
        destructive: false,
        confirmed: false,
        command: "network_probe".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::NetworkProbe {
            plan: Box::new(test_plan()),
            side: TunnelEndpointSide::Left,
            count: 3,
            interval_ms: 500,
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

#[tokio::test]
async fn network_speed_test_create_job_requires_both_tunnel_endpoints() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "left-a".to_string(),
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

    let request = CreateJobRequest {
        job_id: None,
        selector_expression: "id:left-a".to_string(),
        target_client_ids: vec!["left-a".to_string()],
        destructive: false,
        confirmed: false,
        command: "network_speed_test".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::NetworkSpeedTest {
            plan: Box::new(test_plan()),
            server_side: TunnelEndpointSide::Left,
            duration_secs: 3,
            max_bytes: 16 * 1024 * 1024,
            rate_limit_kbps: 100_000,
            port: 5201,
            connect_timeout_ms: 5000,
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
    assert_eq!(error.code, "network_speed_test_target_mismatch");
}

fn test_plan() -> TunnelPlan {
    plan_tunnel(&test_plan_input()).unwrap()
}

fn test_plan_input() -> TunnelPlanInput {
    TunnelPlanInput {
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
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}

fn test_state_with_privilege_auto_approve(repo: Repository) -> AppState {
    AppState {
        gateway: GatewayDispatchClient::test_privilege_auto_approve(),
        ..test_state(repo)
    }
}
