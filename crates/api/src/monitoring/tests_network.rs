use super::*;

use axum::{extract::State, Json};
use tokio::sync::broadcast;
use vpsman_common::{
    plan_tunnel, JobCommand, OspfControlMode, OspfCostPolicy, RoutingCostAdapterCommands,
    RoutingCostCommandSource, RuntimeTunnelAdapterCommands, RuntimeTunnelCommand,
    RuntimeTunnelControl, RuntimeTunnelManager, TunnelAddressFamily, TunnelAddressPair,
    TunnelEndpointSide, TunnelKind, TunnelOspfConfig, TunnelPlanInput,
};

use crate::{
    gateway_client::GatewayDispatchClient,
    job_request::validate_job_command,
    model::{ConfigurationPresetOverrideRecord, CreateConfigurationPresetRequest},
};

const LEFT_RUNTIME_ADAPTER: &str = "11111111-1111-4111-8111-111111111111";
const RIGHT_RUNTIME_ADAPTER: &str = "22222222-2222-4222-8222-222222222222";
const LEFT_ROUTING_ADAPTER: &str = "33333333-3333-4333-8333-333333333333";
const RIGHT_ROUTING_ADAPTER: &str = "44444444-4444-4444-8444-444444444444";

#[tokio::test]
async fn ospf_updater_uses_endpoint_preset_unless_that_plan_overrides_it() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let preset = assign_test_ospf_preset(&repo, "client-a", "global-left").await;
    let preset_id = preset.id.to_string();
    let mut input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, true);
    input.ospf.as_mut().unwrap().left_adapter_definition_id = None;
    seed_test_plan_adapter_definitions(&repo, &input).await;
    let plan = plan_tunnel(&input).unwrap();
    let saved = repo
        .record_tunnel_plan(&input, &plan, true, &network_test_operator())
        .await
        .unwrap();

    let state = test_state(repo);
    let (left, right) = crate::routes_network::resolve_plan_routing_adapters(&state, &saved)
        .await
        .unwrap();

    assert_eq!(left.source, RoutingCostCommandSource::ConfigurationPreset);
    assert_eq!(left.definition_id, preset_id);
    assert_eq!(left.update.argv[0], "/usr/bin/global-left-update");
    assert_eq!(right.source, RoutingCostCommandSource::PlanOverride);
    assert_eq!(right.definition_id, RIGHT_ROUTING_ADAPTER);

    let left_job_id = Uuid::new_v4();
    let right_job_id = Uuid::new_v4();
    let (jobs, _) = crate::routes_network::dispatch_routing_jobs(
        &state,
        &network_test_operator(),
        &saved,
        left_job_id,
        right_job_id,
        left,
        right,
        None,
    )
    .await;
    assert_eq!(jobs.len(), 2);
    let Repository::Memory(memory) = &state.repo else {
        unreachable!()
    };
    let operations = memory.job_operations.read().await;
    let JobCommand::NetworkRoutingStatus { plan, adapter, .. } =
        operations.get(&left_job_id).unwrap()
    else {
        panic!("left endpoint must receive a routing status job");
    };
    let ospf = plan.ospf.as_ref().unwrap();
    assert_eq!(
        ospf.left_adapter_definition_id.as_deref(),
        Some(preset_id.as_str())
    );
    assert_eq!(
        ospf.right_adapter_definition_id.as_deref(),
        Some(RIGHT_ROUTING_ADAPTER)
    );
    assert_eq!(
        adapter.source,
        RoutingCostCommandSource::ConfigurationPreset
    );
    let wire = serde_json::to_value(operations.get(&left_job_id).unwrap()).unwrap();
    assert_eq!(wire["plan"]["ospf"]["left_adapter_template_id"], preset_id);
    assert_eq!(wire["adapter"]["template_id"], preset_id);
    assert!(wire["plan"]["ospf"]
        .get("left_adapter_definition_id")
        .is_none());
}

#[tokio::test]
async fn invalid_plan_ospf_override_never_falls_back_to_a_vps_preset() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    assign_test_ospf_preset(&repo, "client-a", "must-not-run").await;
    let mut input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, true);
    input.ospf.as_mut().unwrap().left_adapter_definition_id =
        Some("55555555-5555-4555-8555-555555555555".to_string());
    seed_test_plan_adapter_definitions(&repo, &input).await;
    let plan = plan_tunnel(&input).unwrap();
    let saved = repo
        .record_tunnel_plan(&input, &plan, true, &network_test_operator())
        .await
        .unwrap();
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    memory
        .network_adapter_definitions
        .write()
        .await
        .retain(|definition| definition.id.to_string() != "55555555-5555-4555-8555-555555555555");

    let error = crate::routes_network::resolve_plan_routing_adapters(&test_state(repo), &saved)
        .await
        .unwrap_err();

    assert_eq!(error.code, "routing_cost_adapter_definition_not_found");
}

#[tokio::test]
async fn unconfigured_endpoint_ospf_preset_is_an_explicit_error() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let mut input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, true);
    let ospf = input.ospf.as_mut().unwrap();
    ospf.left_adapter_definition_id = None;
    ospf.right_adapter_definition_id = None;
    let plan = plan_tunnel(&input).unwrap();
    let saved = repo
        .record_tunnel_plan(&input, &plan, true, &network_test_operator())
        .await
        .unwrap();

    let error = crate::routes_network::resolve_plan_routing_adapters(&test_state(repo), &saved)
        .await
        .unwrap_err();

    assert_eq!(error.code, "ospf_update_command_unconfigured");
}

#[tokio::test]
async fn saved_plan_is_explicit_and_has_no_ospf_state_when_ospf_is_off() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
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
async fn memory_management_list_preserves_typed_tunnel_identity() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
    let plan = plan_tunnel(&input).unwrap();
    let created = repo
        .record_tunnel_plan(&input, &plan, false, &network_test_operator())
        .await
        .unwrap();
    let items = repo.list_tunnel_plan_items().await.unwrap();
    assert!(matches!(
        items.as_slice(),
        [crate::model::TunnelPlanListItem::Plan(plan)] if plan.id == created.id
    ));
}

#[tokio::test]
async fn connection_assessment_is_audited_revision_bound_and_cleared_by_plan_changes() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, true);
    seed_test_plan_adapter_definitions(&repo, &input).await;
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
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, true);
    seed_test_plan_adapter_definitions(&repo, &input).await;
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
async fn ospf_dispatch_reports_each_endpoint_when_one_target_disappears() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, true);
    seed_test_plan_adapter_definitions(&repo, &input).await;
    let plan = plan_tunnel(&input).unwrap();
    let saved = repo
        .record_tunnel_plan(&input, &plan, true, &network_test_operator())
        .await
        .unwrap();
    let left_job_id = Uuid::new_v4();
    let right_job_id = Uuid::new_v4();
    let staged = repo
        .stage_tunnel_plan_ospf_jobs(
            saved.id,
            saved.revision,
            None,
            None,
            None,
            left_job_id,
            right_job_id,
            &network_test_operator(),
        )
        .await
        .unwrap();
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    memory
        .agents
        .write()
        .await
        .retain(|agent| agent.id != "client-b");
    let state = test_state(repo.clone());

    let (jobs, dispatch) = crate::routes_network::dispatch_routing_jobs(
        &state,
        &network_test_operator(),
        &staged,
        left_job_id,
        right_job_id,
        routing_adapter(LEFT_ROUTING_ADAPTER),
        routing_adapter(RIGHT_ROUTING_ADAPTER),
        None,
    )
    .await;

    assert_eq!(jobs.len(), 1);
    assert_eq!(dispatch.len(), 2);
    assert_eq!(dispatch[0].client_id, "client-a");
    assert_eq!(dispatch[0].status, "queued");
    assert_eq!(dispatch[1].client_id, "client-b");
    assert_eq!(dispatch[1].status, "queue_failed");
    assert!(dispatch[1]
        .error
        .as_deref()
        .is_some_and(|message| message.contains("server rejected it")));
    let refreshed = repo.get_tunnel_plan(saved.id).await.unwrap().unwrap();
    assert_eq!(refreshed.left_ospf_status, "pending");
    assert_eq!(refreshed.right_ospf_status, "failed");
}

#[tokio::test]
async fn allocation_skips_addresses_already_owned_by_saved_plans() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
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
    let created = created.plan;
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
    let unchanged = unchanged.plan;
    assert_eq!(unchanged.revision, created.revision);
    assert_eq!(
        repo.list_audit_logs(100).await.unwrap().len(),
        audit_count_before_noops
    );
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
    assert_eq!(still_disabled.plan.revision, created.revision);
    assert_eq!(still_disabled.sync.len(), 2);
    assert!(still_disabled
        .sync
        .iter()
        .all(|outcome| outcome.status == "queued"));

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
    let updated = updated.plan;
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
async fn tunnel_plan_repository_write_boundary_rechecks_resource_conflicts() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    seed_online_agent(&repo, "client-c").await;
    let operator = network_test_operator();
    let first_input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
    let first_plan = plan_tunnel(&first_input).unwrap();
    repo.record_tunnel_plan(&first_input, &first_plan, false, &operator)
        .await
        .unwrap();

    let mut second_input = first_input.clone();
    second_input.name = "edge-a-edge-c".to_string();
    second_input.interface_name = "tunac".to_string();
    second_input.right_client_id = "client-c".to_string();
    second_input.address_pool_cidr = "10.11.0.0/29".to_string();
    second_input.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.11.0.0".to_string(),
        right: "10.11.0.1".to_string(),
        prefix_len: 31,
    });
    let second_plan = plan_tunnel(&second_input).unwrap();
    let second = repo
        .record_tunnel_plan(&second_input, &second_plan, false, &operator)
        .await
        .unwrap();

    let mut create_interface_conflict = second_input.clone();
    create_interface_conflict.name = "create-interface-conflict".to_string();
    create_interface_conflict.interface_name = first_input.interface_name.clone();
    create_interface_conflict.address_pool_cidr = "10.12.0.0/29".to_string();
    create_interface_conflict.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.12.0.0".to_string(),
        right: "10.12.0.1".to_string(),
        prefix_len: 31,
    });
    let error = repo
        .record_tunnel_plan(
            &create_interface_conflict,
            &plan_tunnel(&create_interface_conflict).unwrap(),
            false,
            &operator,
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "tunnel_plan_interface_conflict");

    let mut create_address_conflict = second_input.clone();
    create_address_conflict.name = "create-address-conflict".to_string();
    create_address_conflict.interface_name = "tun-address".to_string();
    create_address_conflict.address_pool_cidr = first_input.address_pool_cidr.clone();
    create_address_conflict.ipv4_tunnel = first_input.ipv4_tunnel.clone();
    let error = repo
        .record_tunnel_plan(
            &create_address_conflict,
            &plan_tunnel(&create_address_conflict).unwrap(),
            false,
            &operator,
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "tunnel_plan_address_conflict");

    let mut update_interface_conflict = second_input.clone();
    update_interface_conflict.interface_name = first_input.interface_name.clone();
    let error = repo
        .update_tunnel_plan(
            second.id,
            second.revision,
            &update_interface_conflict,
            &plan_tunnel(&update_interface_conflict).unwrap(),
            false,
            &operator,
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "tunnel_plan_interface_conflict");

    let mut update_address_conflict = second_input.clone();
    update_address_conflict.address_pool_cidr = first_input.address_pool_cidr;
    update_address_conflict.ipv4_tunnel = first_input.ipv4_tunnel;
    let error = repo
        .update_tunnel_plan(
            second.id,
            second.revision,
            &update_address_conflict,
            &plan_tunnel(&update_address_conflict).unwrap(),
            false,
            &operator,
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "tunnel_plan_address_conflict");

    let unchanged = repo
        .update_tunnel_plan(
            second.id,
            second.revision,
            &second_input,
            &second_plan,
            false,
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(unchanged.revision, second.revision + 1);
}

#[tokio::test]
async fn tunnel_plan_repository_write_boundary_rejects_invalid_addresses() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let input = test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false);
    let mut plan = plan_tunnel(&input).unwrap();
    plan.ipv4_tunnel.as_mut().unwrap().left = "not-an-ip".to_string();

    let error = repo
        .record_tunnel_plan(&input, &plan, false, &network_test_operator())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "tunnel_plan_address_invalid");
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
async fn tunnel_plan_can_be_revision_bound_retired_and_recreated() {
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
    let created = created.plan;

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
    let deleted = deleted.plan;
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
    let recreated = recreated.plan;
    let Json(deleted_enabled) = crate::routes_network::delete_tunnel_plan(
        State(state),
        headers,
        axum::extract::Path(recreated.id),
        Json(crate::routes_network::TunnelPlanMutationRequest {
            confirmed: true,
            expected_revision: recreated.revision,
        }),
    )
    .await
    .unwrap();
    assert!(!deleted_enabled.plan.enabled);
    assert!(deleted_enabled.plan.deleted_at.is_some());
    assert_eq!(deleted_enabled.sync.len(), 2);
    assert!(deleted_enabled
        .sync
        .iter()
        .all(|outcome| outcome.status == "queued"));
}

#[tokio::test]
async fn tunnel_plan_delete_commits_immediately_and_queues_absent_desired_state() {
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
            input,
            enabled: true,
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    let Json(deleted) = crate::routes_network::delete_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(created.plan.id),
        Json(crate::routes_network::TunnelPlanMutationRequest {
            confirmed: true,
            expected_revision: created.plan.revision,
        }),
    )
    .await
    .unwrap();
    assert!(!deleted.plan.enabled);
    assert!(deleted.plan.deleted_at.is_some());
    assert_eq!(deleted.sync.len(), 2);
    assert!(deleted
        .sync
        .iter()
        .all(|outcome| outcome.status == "queued"));
    assert!(repo.list_tunnel_plans().await.unwrap().is_empty());
    let deleted_plan_id = deleted.plan.id.to_string();

    for client_id in ["client-a", "client-b"] {
        let pending = repo
            .list_runtime_config_apply_records(Some(client_id))
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(
            pending.pending_reason.as_deref(),
            Some("tunnel_plan_deleted")
        );
        assert!(pending
            .pending_config
            .unwrap()
            .network
            .runtime_status_telemetry_plans
            .iter()
            .all(|plan| plan.plan_id.as_deref() != Some(deleted_plan_id.as_str())));
    }
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
    let created = created.plan;
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
    let updated = updated.plan;

    assert!(updated.enabled);
    assert_eq!(updated.revision, 2);
    assert_eq!(updated.plan.bandwidth_mbps, 2500);
}

#[tokio::test]
async fn enabling_an_enabled_tunnel_plan_requeues_its_current_desired_state() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let (_, Json(created)) = crate::routes_network::create_tunnel_plan(
        State(state.clone()),
        headers.clone(),
        Json(CreateTunnelPlanRequest {
            input: test_plan_input(RuntimeTunnelManager::AgentIproute2Managed, false),
            enabled: true,
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    let Json(reapplied) = crate::routes_network::enable_tunnel_plan(
        State(state),
        headers,
        axum::extract::Path(created.plan.id),
        Json(crate::routes_network::TunnelPlanMutationRequest {
            confirmed: true,
            expected_revision: created.plan.revision,
        }),
    )
    .await
    .unwrap();

    assert!(reapplied.plan.enabled);
    assert_eq!(reapplied.plan.revision, created.plan.revision);
    assert_eq!(reapplied.sync.len(), 2);
    assert!(reapplied.sync.iter().all(|entry| entry.status == "queued"));
}

#[tokio::test]
async fn external_observed_plan_enables_evidence_without_enabling_mutation() {
    let repo = Repository::Memory(MemoryState::default());
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
    let mut input = test_plan_input(RuntimeTunnelManager::ExternalObserved, false);
    input.name = "Operator observed link".to_string();
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
    seed_online_agent(&repo, "client-a").await;
    seed_online_agent(&repo, "client-b").await;
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
        rollout: None,
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
    let kind = if manager == RuntimeTunnelManager::AgentIproute2Managed {
        TunnelKind::Gre
    } else {
        TunnelKind::Wireguard
    };
    TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "tunab".to_string(),
        kind,
        runtime_control: RuntimeTunnelControl {
            manager,
            left_adapter_definition_id: (manager == RuntimeTunnelManager::ExternalManagedAdapter)
                .then(|| LEFT_RUNTIME_ADAPTER.to_string()),
            right_adapter_definition_id: (manager == RuntimeTunnelManager::ExternalManagedAdapter)
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
        left_mtu: vpsman_common::default_tunnel_mtu(kind),
        right_mtu: vpsman_common::default_tunnel_mtu(kind),
        ospf: ospf.then(|| TunnelOspfConfig {
            mode: OspfControlMode::Reviewed,
            planned_latency_ms: 18.0,
            planned_packet_loss_ratio: 0.0,
            preference: 1.0,
            policy: OspfCostPolicy::default(),
            min_cost_delta: 5,
            healthy_windows: 2,
            left_adapter_definition_id: Some(LEFT_ROUTING_ADAPTER.to_string()),
            right_adapter_definition_id: Some(RIGHT_ROUTING_ADAPTER.to_string()),
        }),
    }
}

pub(super) async fn seed_test_plan_adapter_definitions(repo: &Repository, input: &TunnelPlanInput) {
    let mut references = Vec::<(Uuid, &'static str)>::new();
    let mut add_reference = |raw_id: &str, adapter_kind: &'static str| {
        let id = Uuid::parse_str(raw_id).unwrap();
        if !references.contains(&(id, adapter_kind)) {
            references.push((id, adapter_kind));
        }
    };
    if input.runtime_control.manager == RuntimeTunnelManager::ExternalManagedAdapter {
        add_reference(
            input
                .runtime_control
                .left_adapter_definition_id
                .as_deref()
                .unwrap(),
            "runtime_tunnel",
        );
        add_reference(
            input
                .runtime_control
                .right_adapter_definition_id
                .as_deref()
                .unwrap(),
            "runtime_tunnel",
        );
    }
    if let Some(ospf) = &input.ospf {
        if let Some(id) = ospf.left_adapter_definition_id.as_deref() {
            add_reference(id, "routing_cost");
        }
        if let Some(id) = ospf.right_adapter_definition_id.as_deref() {
            add_reference(id, "routing_cost");
        }
    }

    for (id, adapter_kind) in references {
        let command = |verb: &str| {
            serde_json::json!({
                "argv": [format!("/usr/bin/test-{verb}")],
                "max_timeout_secs": 10,
                "max_output_bytes": 16384
            })
        };
        let definition = if adapter_kind == "runtime_tunnel" {
            serde_json::json!({
                "manager": "external_managed_adapter",
                "contract_version": 1,
                "startup_command": command("start"),
                "cleanup_command": command("cleanup"),
                "status_command": command("status")
            })
        } else {
            serde_json::json!({
                "contract_version": vpsman_common::ROUTING_COST_ADAPTER_CONTRACT_VERSION,
                "status_command": command("status"),
                "update_command": command("update")
            })
        };
        let name = format!("test-{adapter_kind}-{}", id.simple());
        match repo {
            Repository::Memory(memory) => {
                let mut definitions = memory.network_adapter_definitions.write().await;
                if definitions.iter().any(|definition| definition.id == id) {
                    continue;
                }
                let now = crate::unix_now().to_string();
                definitions.push(
                    crate::model_configuration_presets::NetworkAdapterDefinitionView {
                        id,
                        adapter_kind: adapter_kind.to_string(),
                        name,
                        description: None,
                        definition,
                        created_at: now.clone(),
                        updated_at: now,
                    },
                );
            }
            Repository::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO network_adapter_definitions (
                        id, adapter_kind, name, definition
                    )
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (id) DO NOTHING
                    "#,
                )
                .bind(id)
                .bind(adapter_kind)
                .bind(name)
                .bind(sqlx::types::Json(definition))
                .execute(pool)
                .await
                .unwrap();
            }
        }
    }
}

fn runtime_adapter(definition_id: &str) -> RuntimeTunnelAdapterCommands {
    let command = |verb: &str| RuntimeTunnelCommand {
        argv: vec!["/opt/vpsman-adapters/runtime".to_string(), verb.to_string()],
        max_timeout_secs: 10,
        max_output_bytes: 16 * 1024,
    };
    RuntimeTunnelAdapterCommands {
        definition_id: definition_id.to_string(),
        definition_name: "runtime-adapter".to_string(),
        definition_hash: "ab".repeat(32),
        startup: Some(command("start")),
        stop: Some(command("stop")),
        cleanup: None,
        restart: None,
        status: command("status"),
        traffic_limit_apply: None,
    }
}

fn routing_adapter(definition_id: &str) -> RoutingCostAdapterCommands {
    let command = |verb: &str| RuntimeTunnelCommand {
        argv: vec!["/opt/vpsman-adapters/routing".to_string(), verb.to_string()],
        max_timeout_secs: 10,
        max_output_bytes: 16 * 1024,
    };
    RoutingCostAdapterCommands {
        source: vpsman_common::RoutingCostCommandSource::PlanOverride,
        definition_id: definition_id.to_string(),
        definition_name: "routing-adapter".to_string(),
        definition_hash: "cd".repeat(32),
        status: command("status"),
        update: command("update"),
    }
}

async fn assign_test_ospf_preset(
    repo: &Repository,
    client_id: &str,
    executable_prefix: &str,
) -> crate::model::ConfigurationPresetView {
    let command = |verb: &str| {
        serde_json::json!({
            "argv": [format!("/usr/bin/{executable_prefix}-{verb}")],
            "max_timeout_secs": 10,
            "max_output_bytes": 16384
        })
    };
    let preset = repo
        .create_configuration_preset(
            &CreateConfigurationPresetRequest {
                behavior: "ospf_update_command".to_string(),
                name: format!("{client_id} OSPF updater"),
                description: None,
                definition: serde_json::json!({
                    "contract_version": vpsman_common::ROUTING_COST_ADAPTER_CONTRACT_VERSION,
                    "status_command": command("status"),
                    "update_command": command("update")
                }),
            },
            &network_test_operator(),
        )
        .await
        .unwrap();
    let Repository::Memory(memory) = repo else {
        unreachable!()
    };
    memory
        .configuration_preset_overrides
        .write()
        .await
        .push(ConfigurationPresetOverrideRecord {
            client_id: client_id.to_string(),
            behavior: "ospf_update_command".to_string(),
            preset_id: preset.id,
            updated_at: crate::unix_now().to_string(),
        });
    preset
}

pub(crate) async fn seed_online_agent(repo: &Repository, client_id: &str) {
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
        gateway: GatewayDispatchClient::new(Some("http://127.0.0.1:1".to_string()), None),
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
        session_id: Some(Uuid::new_v4()),
    }
}
