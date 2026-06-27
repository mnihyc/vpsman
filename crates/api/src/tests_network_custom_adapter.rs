use super::*;

use axum::{extract::State, http::StatusCode, Json};
use tokio::sync::broadcast;
use vpsman_common::{
    AgentHello, OspfCostPolicy, RuntimeTunnelCommand, RuntimeTunnelControl, RuntimeTunnelManager,
    RuntimeTunnelTopologyIntent, TunnelKind, TunnelPlanInput,
};

use crate::gateway_client::GatewayDispatchClient;

#[tokio::test]
async fn promote_observed_tunnel_plan_to_external_adapter_preserves_plan_id() {
    let repo = Repository::Memory(MemoryState::default());
    let observed = create_observed_plan(&repo, "observed-wg", "wg42", TunnelKind::Wireguard).await;
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;

    let Json(promoted) = crate::routes_network::promote_tunnel_plan_to_custom_adapter(
        State(state),
        headers,
        Json(PromoteTunnelPlanToCustomAdapterRequest {
            plan_id: observed.id,
            runtime_control: RuntimeTunnelControl {
                manager: RuntimeTunnelManager::ExternalManagedAdapter,
                startup: Some(RuntimeTunnelCommand {
                    argv: vec![
                        "/usr/local/libexec/wg-adapter".to_string(),
                        "start".to_string(),
                    ],
                    ..RuntimeTunnelCommand::default()
                }),
                cleanup: Some(RuntimeTunnelCommand {
                    argv: vec![
                        "/usr/local/libexec/wg-adapter".to_string(),
                        "cleanup".to_string(),
                    ],
                    ..RuntimeTunnelCommand::default()
                }),
                status: Some(RuntimeTunnelCommand {
                    argv: vec![
                        "/usr/local/libexec/wg-adapter".to_string(),
                        "status".to_string(),
                    ],
                    ..RuntimeTunnelCommand::default()
                }),
                ..RuntimeTunnelControl::default()
            },
            runtime_topology: None,
            name: Some("managed-wg".to_string()),
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    assert_eq!(promoted.id, observed.id);
    assert_eq!(promoted.created_at, observed.created_at);
    assert_eq!(promoted.name, "managed-wg");
    assert_eq!(
        promoted.plan.runtime_control.manager,
        RuntimeTunnelManager::ExternalManagedAdapter
    );
    assert!(promoted.plan.runtime_control.status.is_some());
    assert!(promoted.plan.runtime_control.cleanup.is_some());
    assert_eq!(
        promoted.plan.runtime_topology.desired_interfaces,
        vec!["wg42".to_string()]
    );
    assert!(promoted.plan.ifupdown_snippet.contains("custom adapter"));
    let audits = repo.list_audit_logs(10).await.unwrap();
    let audit = audits
        .iter()
        .find(|audit| audit.action == "network.tunnel_plan_promoted_to_custom_adapter")
        .expect("custom adapter audit");
    assert_eq!(audit.metadata["custom_adapter_cleanup_configured"], true);
}

#[tokio::test]
async fn promote_disabled_tunnel_plan_to_custom_adapter_does_not_sync_runtime_config() {
    let repo = Repository::Memory(MemoryState::default());
    let observed = create_observed_plan_with_enabled(
        &repo,
        "disabled-observed-wg",
        "wg52",
        TunnelKind::Wireguard,
        false,
    )
    .await;
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;

    let Json(promoted) = crate::routes_network::promote_tunnel_plan_to_custom_adapter(
        State(state),
        headers,
        Json(PromoteTunnelPlanToCustomAdapterRequest {
            plan_id: observed.id,
            runtime_control: full_adapter_control("/usr/local/libexec/wg-adapter"),
            runtime_topology: None,
            name: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap();

    assert!(!promoted.enabled);
    assert_eq!(
        promoted.plan.runtime_control.manager,
        RuntimeTunnelManager::ExternalManagedAdapter
    );
    assert!(repo.list_jobs(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn promote_tunnel_plan_to_custom_adapter_requires_confirmation_and_status_command() {
    let repo = Repository::Memory(MemoryState::default());
    let observed =
        create_observed_plan(&repo, "observed-openvpn", "ovpn42", TunnelKind::Openvpn).await;
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;

    let error = crate::routes_network::promote_tunnel_plan_to_custom_adapter(
        State(state),
        headers,
        Json(PromoteTunnelPlanToCustomAdapterRequest {
            plan_id: observed.id,
            runtime_control: startup_only_adapter_control(),
            runtime_topology: None,
            name: None,
            confirmed: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "custom_adapter_requires_confirmation");

    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let error = crate::routes_network::promote_tunnel_plan_to_custom_adapter(
        State(state),
        headers,
        Json(PromoteTunnelPlanToCustomAdapterRequest {
            plan_id: observed.id,
            runtime_control: startup_only_adapter_control(),
            runtime_topology: None,
            name: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "custom_adapter_status_command_required");
}

async fn create_observed_plan(
    repo: &Repository,
    name: &str,
    interface_name: &str,
    kind: TunnelKind,
) -> TunnelPlanView {
    create_observed_plan_with_enabled(repo, name, interface_name, kind, true).await
}

async fn create_observed_plan_with_enabled(
    repo: &Repository,
    name: &str,
    interface_name: &str,
    kind: TunnelKind,
    enabled: bool,
) -> TunnelPlanView {
    if let Repository::Memory(memory) = repo {
        seed_test_agent(memory, "client-a").await;
        seed_test_agent(memory, "client-b").await;
    }
    let mut input = test_plan_input(name, interface_name, kind);
    input.runtime_control = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalObserved,
        ..RuntimeTunnelControl::default()
    };
    input.runtime_topology = RuntimeTunnelTopologyIntent {
        version: Some(format!("observed:{interface_name}")),
        desired_interfaces: vec![interface_name.to_string()],
        ..RuntimeTunnelTopologyIntent::default()
    };
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;
    let (status, Json(observed)) = crate::routes_network::create_tunnel_plan(
        State(state),
        headers,
        Json(CreateTunnelPlanRequest {
            input,
            enabled,
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::CREATED);
    observed
}

async fn seed_test_agent(memory: &MemoryState, client_id: &str) {
    upsert_memory_agent(
        &memory.agents,
        &AgentHello {
            client_id: client_id.to_string(),
            process_incarnation_id: uuid::Uuid::new_v4(),
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

fn startup_only_adapter_control() -> RuntimeTunnelControl {
    RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalManagedAdapter,
        startup: Some(RuntimeTunnelCommand {
            argv: vec!["/usr/local/libexec/openvpn-adapter".to_string()],
            ..RuntimeTunnelCommand::default()
        }),
        ..RuntimeTunnelControl::default()
    }
}

fn full_adapter_control(binary: &str) -> RuntimeTunnelControl {
    RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalManagedAdapter,
        startup: Some(RuntimeTunnelCommand {
            argv: vec![binary.to_string(), "start".to_string()],
            ..RuntimeTunnelCommand::default()
        }),
        cleanup: Some(RuntimeTunnelCommand {
            argv: vec![binary.to_string(), "cleanup".to_string()],
            ..RuntimeTunnelCommand::default()
        }),
        status: Some(RuntimeTunnelCommand {
            argv: vec![binary.to_string(), "status".to_string()],
            ..RuntimeTunnelCommand::default()
        }),
        ..RuntimeTunnelControl::default()
    }
}

fn test_plan_input(name: &str, interface_name: &str, kind: TunnelKind) -> TunnelPlanInput {
    TunnelPlanInput {
        name: name.to_string(),
        interface_name: interface_name.to_string(),
        kind,
        runtime_control: RuntimeTunnelControl::default(),
        runtime_topology: RuntimeTunnelTopologyIntent::default(),
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
        bandwidth_mbps: 100,
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
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}
