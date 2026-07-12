use super::*;

use axum::{extract::State, Json};
use tokio::sync::broadcast;
use vpsman_common::{
    plan_tunnel, JobCommand, OspfControlMode, OspfCostPolicy, RuntimeTunnelAdapterCommands,
    RuntimeTunnelCommand, RuntimeTunnelControl, RuntimeTunnelManager, TunnelAddressFamily,
    TunnelAddressPair, TunnelEndpointSide, TunnelKind, TunnelOspfConfig, TunnelPlanInput,
};

use crate::{gateway_client::GatewayDispatchClient, job_request::validate_job_command};

const LEFT_RUNTIME_ADAPTER: &str = "11111111-1111-4111-8111-111111111111";
const RIGHT_RUNTIME_ADAPTER: &str = "22222222-2222-4222-8222-222222222222";
const LEFT_ROUTING_ADAPTER: &str = "33333333-3333-4333-8333-333333333333";
const RIGHT_ROUTING_ADAPTER: &str = "44444444-4444-4444-8444-444444444444";

#[tokio::test]
async fn saved_plan_is_explicit_and_has_no_ospf_state_when_ospf_is_off() {
    let repo = Repository::Memory(MemoryState::default());
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
    let plan = plan_tunnel(&input).unwrap();
    let view = repo
        .record_tunnel_plan(&input, &plan, true, &network_test_operator())
        .await
        .unwrap();

    assert!(view.enabled);
    assert_eq!(view.ospf_status, "disabled");
    assert_eq!(view.recommended_ospf_cost, None);
    assert_eq!(repo.list_tunnel_plans().await.unwrap().len(), 1);
    assert_eq!(
        repo.list_audit_logs(10).await.unwrap()[0].action,
        "network.tunnel_plan_created"
    );
}

#[tokio::test]
async fn connection_assessment_is_audited_revision_bound_and_cleared_by_plan_changes() {
    let repo = Repository::Memory(MemoryState::default());
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, true);
    let plan = plan_tunnel(&input).unwrap();
    let saved = repo
        .record_tunnel_plan(&input, &plan, true, &network_test_operator())
        .await
        .unwrap();
    let original_updated_at = saved.updated_at.clone();
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;

    let Json(assessed) = crate::routes_network::update_tunnel_connection_assessment(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(saved.id),
        Json(UpdateTunnelConnectionAssessmentRequest {
            assessment: "connected".to_string(),
            expected_revision: saved.revision,
            note: Some("Application traffic verified; ICMP is blocked".to_string()),
        }),
    )
    .await
    .unwrap();
    assert_eq!(assessed.connection_assessment, "connected");
    assert_eq!(assessed.revision, saved.revision + 1);
    assert_eq!(assessed.updated_at, original_updated_at);
    assert!(assessed.connection_assessed_at.is_some());
    assert_eq!(assessed.ospf_status, saved.ospf_status);

    let stale = crate::routes_network::update_tunnel_connection_assessment(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(saved.id),
        Json(UpdateTunnelConnectionAssessmentRequest {
            assessment: "disconnected".to_string(),
            expected_revision: saved.revision,
            note: Some("Console test failed".to_string()),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.code, "tunnel_plan_snapshot_stale");

    let missing_note = crate::routes_network::update_tunnel_connection_assessment(
        State(state),
        headers,
        axum::extract::Path(saved.id),
        Json(UpdateTunnelConnectionAssessmentRequest {
            assessment: "disconnected".to_string(),
            expected_revision: assessed.revision,
            note: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        missing_note.code,
        "tunnel_connection_assessment_note_required"
    );

    let mut changed_input = input.clone();
    changed_input.bandwidth_mbps = 1500;
    let changed_plan = plan_tunnel(&changed_input).unwrap();
    let updated = repo
        .update_tunnel_plan(
            saved.id,
            assessed.revision,
            &changed_input,
            &changed_plan,
            true,
            &network_test_operator(),
        )
        .await
        .unwrap();
    assert_eq!(updated.connection_assessment, "automatic");
    assert!(updated.connection_assessment_note.is_none());
    assert!(repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|event| event.action == "network.tunnel_connection_assessed"));
}

#[tokio::test]
async fn enabled_ospf_plan_starts_unverified_and_stages_exact_endpoint_jobs() {
    let repo = Repository::Memory(MemoryState::default());
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, true);
    let plan = plan_tunnel(&input).unwrap();
    let saved = repo
        .record_tunnel_plan(&input, &plan, true, &network_test_operator())
        .await
        .unwrap();
    assert_eq!(saved.ospf_status, "unverified");
    assert!(saved.recommended_ospf_cost.is_some());

    let left_job = Uuid::new_v4();
    let right_job = Uuid::new_v4();
    let stale_error = repo
        .stage_tunnel_plan_ospf_jobs(
            saved.id,
            saved.revision + 1,
            None,
            None,
            None,
            left_job,
            right_job,
            &network_test_operator(),
        )
        .await
        .unwrap_err();
    assert!(stale_error
        .to_string()
        .contains("tunnel_plan_ospf_snapshot_stale"));
    let staged = repo
        .stage_tunnel_plan_ospf_jobs(
            saved.id,
            saved.revision,
            None,
            None,
            None,
            left_job,
            right_job,
            &network_test_operator(),
        )
        .await
        .unwrap();
    assert_eq!(staged.ospf_status, "pending");
    assert_eq!(staged.left_ospf_job_id, Some(left_job));
    assert_eq!(staged.right_ospf_job_id, Some(right_job));

    repo.record_tunnel_plan_ospf_job_result(
        saved.id,
        TunnelEndpointSide::Left,
        left_job,
        Some(20),
        true,
    )
    .await
    .unwrap();
    let verified = repo
        .record_tunnel_plan_ospf_job_result(
            saved.id,
            TunnelEndpointSide::Right,
            right_job,
            Some(20),
            true,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(verified.ospf_status, "verified");
    assert_eq!(verified.left_current_ospf_cost, Some(20));
    assert_eq!(verified.right_current_ospf_cost, Some(20));

    assert!(repo
        .record_tunnel_plan_ospf_job_result(
            saved.id,
            TunnelEndpointSide::Left,
            left_job,
            None,
            false,
        )
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn routing_template_update_marks_only_bound_endpoint_state_stale() {
    let repo = Repository::Memory(MemoryState::default());
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, true);
    let plan = plan_tunnel(&input).unwrap();
    let saved = repo
        .record_tunnel_plan(&input, &plan, true, &network_test_operator())
        .await
        .unwrap();
    let left_job = Uuid::new_v4();
    let right_job = Uuid::new_v4();
    repo.stage_tunnel_plan_ospf_jobs(
        saved.id,
        saved.revision,
        None,
        None,
        None,
        left_job,
        right_job,
        &network_test_operator(),
    )
    .await
    .unwrap();

    repo.mark_routing_adapter_template_stale(Uuid::parse_str(LEFT_ROUTING_ADAPTER).unwrap())
        .await
        .unwrap();
    let stale = repo.get_tunnel_plan(saved.id).await.unwrap().unwrap();
    assert_eq!(stale.left_ospf_status, "stale");
    assert_eq!(stale.right_ospf_status, "pending");
    assert_eq!(stale.left_ospf_job_id, None);
    assert_eq!(stale.ospf_status, "pending");
}

#[tokio::test]
async fn routing_template_update_leaves_disabled_plan_state_disabled() {
    let repo = Repository::Memory(MemoryState::default());
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, true);
    let plan = plan_tunnel(&input).unwrap();
    let saved = repo
        .record_tunnel_plan(&input, &plan, false, &network_test_operator())
        .await
        .unwrap();

    repo.mark_routing_adapter_template_stale(Uuid::parse_str(LEFT_ROUTING_ADAPTER).unwrap())
        .await
        .unwrap();
    let unchanged = repo.get_tunnel_plan(saved.id).await.unwrap().unwrap();
    assert_eq!(unchanged.ospf_status, "disabled");
    assert_eq!(unchanged.left_ospf_status, "disabled");
    assert_eq!(unchanged.right_ospf_status, "disabled");
}

#[tokio::test]
async fn allocation_skips_addresses_already_owned_by_saved_plans() {
    let repo = Repository::Memory(MemoryState::default());
    let mut input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
    input.address_pool_cidr = "10.10.0.0/29".to_string();
    input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.10.0.0".to_string(),
        right: "10.10.0.1".to_string(),
        prefix_len: 31,
    });
    let plan = plan_tunnel(&input).unwrap();
    repo.record_tunnel_plan(&input, &plan, false, &network_test_operator())
        .await
        .unwrap();

    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let Json(allocation) = crate::routes_network::allocate_tunnel_endpoints(
        State(state),
        headers,
        Json(AllocateTunnelEndpointsRequest {
            ipv4_pool_cidr: Some("10.10.0.0/29".to_string()),
            ipv6_pool_cidr: None,
            reserved_addresses: Vec::new(),
            include_ipv4: Some(true),
            include_ipv6: Some(false),
        }),
    )
    .await
    .unwrap();
    assert_eq!(allocation.ipv4_tunnel.unwrap().left, "10.10.0.2");
}

#[tokio::test]
async fn create_plan_route_requires_confirmation_before_any_write() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let error = crate::routes_network::create_tunnel_plan(
        State(state),
        headers,
        Json(CreateTunnelPlanRequest {
            input: test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false),
            enabled: false,
            confirmed: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "tunnel_plan_mutation_requires_confirmation");
    assert!(repo.list_tunnel_plans().await.unwrap().is_empty());
}

#[tokio::test]
async fn tunnel_plan_create_rejects_duplicates_and_update_rejects_stale_revisions() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
    let (_, Json(created)) = crate::routes_network::create_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        Json(CreateTunnelPlanRequest {
            input: input.clone(),
            enabled: false,
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(created.revision, 1);

    let audit_count_before_noops = repo.list_audit_logs(100).await.unwrap().len();
    let Json(unchanged) = crate::routes_network::update_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(created.id),
        Json(UpdateTunnelPlanRequest {
            input: input.clone(),
            expected_revision: created.revision,
            enabled: Some(false),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(unchanged.revision, created.revision);
    let Json(still_disabled) = crate::routes_network::disable_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(created.id),
        Json(crate::routes_network::TunnelPlanMutationRequest {
            confirmed: true,
            expected_revision: created.revision,
        }),
    )
    .await
    .unwrap();
    assert_eq!(still_disabled.revision, created.revision);
    assert_eq!(
        repo.list_audit_logs(100).await.unwrap().len(),
        audit_count_before_noops
    );

    let duplicate = crate::routes_network::create_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        Json(CreateTunnelPlanRequest {
            input: input.clone(),
            enabled: false,
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(duplicate.code, "tunnel_plan_name_conflict");

    let mut replacement = input.clone();
    replacement.bandwidth_mbps = 2500;
    let Json(updated) = crate::routes_network::update_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(created.id),
        Json(UpdateTunnelPlanRequest {
            input: replacement.clone(),
            expected_revision: created.revision,
            enabled: Some(false),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(updated.id, created.id);
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.plan.bandwidth_mbps, 2500);

    let stale = crate::routes_network::update_tunnel_plan(
        State(state),
        headers,
        axum::extract::Path(created.id),
        Json(UpdateTunnelPlanRequest {
            input: replacement,
            expected_revision: created.revision,
            enabled: Some(false),
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.code, "tunnel_plan_snapshot_stale");
    let lifecycle_state = test_state(repo.clone());
    let lifecycle_headers = crate::test_auth_headers(&lifecycle_state).await;
    let stale_lifecycle = crate::routes_network::disable_tunnel_plan(
        State(lifecycle_state),
        lifecycle_headers,
        axum::extract::Path(created.id),
        Json(crate::routes_network::TunnelPlanMutationRequest {
            confirmed: true,
            expected_revision: created.revision,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(stale_lifecycle.code, "tunnel_plan_snapshot_stale");
    assert_eq!(repo.list_tunnel_plans().await.unwrap().len(), 1);
}

#[tokio::test]
async fn tunnel_plan_create_rejects_endpoint_interface_and_address_collisions() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
    let _ = crate::routes_network::create_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        Json(CreateTunnelPlanRequest {
            input: input.clone(),
            enabled: false,
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    let mut interface_collision = input.clone();
    interface_collision.name = "same-interface".to_string();
    interface_collision.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.10.0.2".to_string(),
        right: "10.10.0.3".to_string(),
        prefix_len: 31,
    });
    let error = crate::routes_network::create_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        Json(CreateTunnelPlanRequest {
            input: interface_collision,
            enabled: false,
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "tunnel_plan_interface_conflict");

    let mut address_collision = input;
    address_collision.name = "same-addresses".to_string();
    address_collision.interface_name = "tun-other".to_string();
    let error = crate::routes_network::create_tunnel_plan(
        State(state),
        headers,
        Json(CreateTunnelPlanRequest {
            input: address_collision,
            enabled: false,
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "tunnel_plan_address_conflict");
}

#[tokio::test]
async fn disabled_tunnel_plan_can_be_revision_bound_retired_and_recreated() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
    let (_, Json(created)) = crate::routes_network::create_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        Json(CreateTunnelPlanRequest {
            input: input.clone(),
            enabled: false,
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    let stale = crate::routes_network::delete_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(created.id),
        Json(crate::routes_network::TunnelPlanMutationRequest {
            confirmed: true,
            expected_revision: created.revision + 1,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.code, "tunnel_plan_snapshot_stale");

    let Json(deleted) = crate::routes_network::delete_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(created.id),
        Json(crate::routes_network::TunnelPlanMutationRequest {
            confirmed: true,
            expected_revision: created.revision,
        }),
    )
    .await
    .unwrap();
    assert_eq!(deleted.deleted_reason.as_deref(), Some("operator_retired"));
    assert!(deleted.deleted_at.is_some());
    assert!(repo.list_tunnel_plans().await.unwrap().is_empty());

    let (_, Json(recreated)) = crate::routes_network::create_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        Json(CreateTunnelPlanRequest {
            input,
            enabled: true,
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    let enabled = crate::routes_network::delete_tunnel_plan(
        State(state),
        headers,
        axum::extract::Path(recreated.id),
        Json(crate::routes_network::TunnelPlanMutationRequest {
            confirmed: true,
            expected_revision: recreated.revision,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(enabled.code, "tunnel_plan_disable_before_delete");
}

#[tokio::test]
async fn tunnel_plan_update_preserves_enabled_state_when_omitted() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
    let (_, Json(created)) = crate::routes_network::create_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        Json(CreateTunnelPlanRequest {
            input: input.clone(),
            enabled: true,
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    let mut replacement = input;
    replacement.bandwidth_mbps = 2500;

    let Json(updated) = crate::routes_network::update_tunnel_plan(
        State(state),
        headers,
        axum::extract::Path(created.id),
        Json(UpdateTunnelPlanRequest {
            input: replacement,
            expected_revision: created.revision,
            enabled: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    assert!(updated.enabled);
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.plan.bandwidth_mbps, 2500);
}

#[tokio::test]
async fn external_observed_plan_enables_evidence_without_enabling_mutation() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let input = test_plan_input(RuntimeTunnelManager::ExternalObserved, false);
    let plan = plan_tunnel(&input).unwrap();
    repo.record_tunnel_plan(&input, &plan, true, &network_test_operator())
        .await
        .unwrap();
    let state = test_state(repo);

    let runtime = crate::runtime_config::compose_runtime_config(&state, "client-a", 1)
        .await
        .unwrap();
    assert!(runtime.network.runtime_reconcile_enabled);
    assert!(runtime.network.runtime_status_telemetry_enabled);
    assert!(!runtime.network.apply_enabled);
    assert_eq!(runtime.network.runtime_status_telemetry_plans.len(), 1);
    assert_eq!(
        runtime.network.runtime_status_telemetry_plans[0]
            .plan
            .runtime_control
            .manager,
        RuntimeTunnelManager::ExternalObserved
    );
}

#[test]
fn network_status_requires_a_server_bound_runtime_adapter_snapshot() {
    let plan = plan_tunnel(&test_plan_input(
        RuntimeTunnelManager::ExternalManagedAdapter,
        false,
    ))
    .unwrap();
    let missing = JobCommand::NetworkStatus {
        plan_id: Uuid::new_v4().to_string(),
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        runtime_adapter: None,
    };
    assert_eq!(
        validate_job_command(&missing).unwrap_err().code,
        "network_status_adapter_snapshot_required"
    );

    let bound = JobCommand::NetworkStatus {
        plan_id: Uuid::new_v4().to_string(),
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
        runtime_adapter: Some(runtime_adapter(LEFT_RUNTIME_ADAPTER)),
    };
    validate_job_command(&bound).unwrap();
}

#[test]
fn network_status_side_must_match_the_only_dispatch_target() {
    let plan = plan_tunnel(&test_plan_input(
        RuntimeTunnelManager::AgentIproute2Managed,
        false,
    ))
    .unwrap();
    let command = JobCommand::NetworkStatus {
        plan_id: Uuid::new_v4().to_string(),
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
        runtime_adapter: None,
    };
    assert!(vpsman_server_core::validate_network_command_targets(
        &command,
        &["client-a".to_string()]
    )
    .is_ok());
    assert!(vpsman_server_core::validate_network_command_targets(
        &command,
        &["client-b".to_string()]
    )
    .is_err());
}

#[tokio::test]
async fn network_diagnostics_require_an_exact_declared_plan_and_limit_disabled_plans_to_status() {
    let repo = Repository::Memory(MemoryState::default());
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
    let plan = plan_tunnel(&input).unwrap();
    let saved = repo
        .record_tunnel_plan(&input, &plan, true, &network_test_operator())
        .await
        .unwrap();
    let state = test_state(repo.clone());

    let request_for = |plan_id: Uuid, plan: vpsman_common::TunnelPlan| CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: String::new(),
        target_client_ids: vec![plan.left_client_id.clone()],
        destructive: false,
        confirmed: false,
        command: "network_status".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::NetworkStatus {
            plan_id: plan_id.to_string(),
            plan: Box::new(plan),
            side: TunnelEndpointSide::Left,
            runtime_adapter: None,
        }),
        max_timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: false,
        privilege_assertion: None,
    };

    let mut exact = request_for(saved.id, saved.plan.clone());
    crate::routes_jobs::bind_declared_network_plan(&state, &mut exact)
        .await
        .unwrap();

    let mut stale_plan = saved.plan.clone();
    stale_plan.bandwidth_mbps = 250;
    let mut stale = request_for(saved.id, stale_plan);
    assert_eq!(
        crate::routes_jobs::bind_declared_network_plan(&state, &mut stale)
            .await
            .unwrap_err()
            .code,
        "network_diagnostic_plan_snapshot_stale"
    );

    let mut missing = request_for(Uuid::new_v4(), saved.plan.clone());
    assert_eq!(
        crate::routes_jobs::bind_declared_network_plan(&state, &mut missing)
            .await
            .unwrap_err()
            .code,
        "network_diagnostic_plan_not_found"
    );

    repo.set_tunnel_plan_enabled(saved.id, saved.revision, false, &network_test_operator())
        .await
        .unwrap();
    let mut disabled_status = request_for(saved.id, saved.plan.clone());
    crate::routes_jobs::bind_declared_network_plan(&state, &mut disabled_status)
        .await
        .unwrap();

    let mut disabled_probe = request_for(saved.id, saved.plan.clone());
    disabled_probe.command = "network_probe".to_string();
    disabled_probe.operation = Some(JobCommand::NetworkProbe {
        plan_id: saved.id.to_string(),
        plan: Box::new(saved.plan),
        side: TunnelEndpointSide::Left,
        count: 3,
        interval_ms: 500,
    });
    assert_eq!(
        crate::routes_jobs::bind_declared_network_plan(&state, &mut disabled_probe)
            .await
            .unwrap_err()
            .code,
        "network_diagnostic_plan_disabled"
    );
}

pub(super) fn test_plan_input(manager: RuntimeTunnelManager, ospf: bool) -> TunnelPlanInput {
    TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "tunab".to_string(),
        kind: if manager == RuntimeTunnelManager::AgentIproute2Managed {
            TunnelKind::Gre
        } else {
            TunnelKind::Wireguard
        },
        runtime_control: RuntimeTunnelControl {
            manager,
            left_adapter_template_id: (manager == RuntimeTunnelManager::ExternalManagedAdapter)
                .then(|| LEFT_RUNTIME_ADAPTER.to_string()),
            right_adapter_template_id: (manager == RuntimeTunnelManager::ExternalManagedAdapter)
                .then(|| RIGHT_RUNTIME_ADAPTER.to_string()),
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "client-a".to_string(),
        right_client_id: "client-b".to_string(),
        left_remote_underlay: "203.0.113.1".to_string(),
        right_remote_underlay: "203.0.113.2".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.10.0.0/29".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.10.0.0".to_string(),
            right: "10.10.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: TunnelAddressFamily::Ipv4,
        bandwidth_mbps: 1234,
        ospf: ospf.then(|| TunnelOspfConfig {
            mode: OspfControlMode::Reviewed,
            planned_latency_ms: 18.0,
            planned_packet_loss_ratio: 0.0,
            preference: 1.0,
            policy: OspfCostPolicy::default(),
            min_cost_delta: 5,
            healthy_windows: 2,
            left_adapter_template_id: LEFT_ROUTING_ADAPTER.to_string(),
            right_adapter_template_id: RIGHT_ROUTING_ADAPTER.to_string(),
        }),
    }
}

fn runtime_adapter(template_id: &str) -> RuntimeTunnelAdapterCommands {
    let command = |verb: &str| RuntimeTunnelCommand {
        argv: vec!["/opt/vpsman-adapters/runtime".to_string(), verb.to_string()],
        max_timeout_secs: 10,
        max_output_bytes: 16 * 1024,
    };
    RuntimeTunnelAdapterCommands {
        template_id: template_id.to_string(),
        template_name: "runtime-adapter".to_string(),
        definition_hash: "ab".repeat(32),
        startup: Some(command("start")),
        stop: Some(command("stop")),
        cleanup: None,
        restart: None,
        status: command("status"),
        traffic_limit_apply: None,
    }
}

async fn seed_online_agent(repo: &Repository, client_id: &str) {
    let Repository::Memory(memory) = repo else {
        panic!("memory repository required");
    };
    memory.agents.write().await.push(AgentView {
        id: client_id.to_string(),
        display_name: client_id.to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: Some(crate::unix_now().to_string()),
        arch: Some("x86_64".to_string()),
        internal_build_number: 1,
        process_incarnation_id: Some(Uuid::new_v4()),
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    });
}

fn test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(32);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::new(None, None),
        backup_object_store: None,
        update_release_policy: crate::state::UpdateReleasePolicy::default(),
        fleet_alert_policy: crate::fleet_alerts::FleetAlertPolicy::default(),
        job_output_artifact_min_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("/tmp/vpsman-test-suite-config-missing.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}

fn network_test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "network-test".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Uuid::new_v4(),
    }
}
