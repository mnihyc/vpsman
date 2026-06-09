use super::*;
use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use ed25519_dalek::SigningKey;
use tokio::sync::broadcast;
use vpsman_common::{
    BandwidthTier, OspfCostPolicy, RuntimeTunnelCommand, RuntimeTunnelControl,
    RuntimeTunnelManager, RuntimeTunnelTopologyIntent, TunnelKind, TunnelPlanInput,
};

use crate::{gateway_client::GatewayDispatchClient};

#[tokio::test]
async fn promote_observed_tunnel_plan_to_external_adapter_preserves_plan_id() {
    let repo = Repository::Memory(MemoryState::default());
    let observed = create_observed_plan(&repo, "observed-wg", "wg42", TunnelKind::Wireguard).await;

    let Json(promoted) = crate::routes_network::promote_tunnel_plan_to_adapter(
        State(test_state_with_signing_key(repo.clone())),
        HeaderMap::new(),
        Json(PromoteTunnelPlanToAdapterRequest {
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
    assert!(promoted
        .plan
        .ifupdown_snippet
        .contains("external managed adapter"));
    let audits = repo.list_audit_logs(10).await.unwrap();
    let audit = audits
        .iter()
        .find(|audit| audit.action == "network.tunnel_plan_promoted_to_adapter")
        .expect("adapter promotion audit");
    assert_eq!(audit.metadata["adapter_cleanup_configured"], true);
}

#[tokio::test]
async fn promote_tunnel_plan_to_adapter_requires_confirmation_and_status_command() {
    let repo = Repository::Memory(MemoryState::default());
    let observed =
        create_observed_plan(&repo, "observed-openvpn", "ovpn42", TunnelKind::Openvpn).await;

    let error = crate::routes_network::promote_tunnel_plan_to_adapter(
        State(test_state_with_signing_key(repo.clone())),
        HeaderMap::new(),
        Json(PromoteTunnelPlanToAdapterRequest {
            plan_id: observed.id,
            runtime_control: startup_only_adapter_control(),
            runtime_topology: None,
            name: None,
            confirmed: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "adapter_promotion_requires_confirmation");

    let error = crate::routes_network::promote_tunnel_plan_to_adapter(
        State(test_state_with_signing_key(repo)),
        HeaderMap::new(),
        Json(PromoteTunnelPlanToAdapterRequest {
            plan_id: observed.id,
            runtime_control: startup_only_adapter_control(),
            runtime_topology: None,
            name: None,
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "adapter_promotion_status_command_required");
}

async fn create_observed_plan(
    repo: &Repository,
    name: &str,
    interface_name: &str,
    kind: TunnelKind,
) -> TunnelPlanView {
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
    let (status, Json(observed)) = crate::routes_network::create_tunnel_plan(
        State(test_state_with_signing_key(repo.clone())),
        HeaderMap::new(),
        Json(CreateTunnelPlanRequest { input }),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::CREATED);
    observed
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
        bandwidth: BandwidthTier::M100,
        latency_ms: 18.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    }
}

fn test_state_with_signing_key(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        server_signing_key: Some(Arc::new(SigningKey::from_bytes(&[7_u8; 32]))),
        backup_object_store: None,
        update_object_store: None,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    }
}
