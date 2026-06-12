use super::*;

use axum::{extract::State, http::StatusCode, Json};
use tokio::sync::broadcast;
use vpsman_common::{
    AgentMetrics, BandwidthTier, GatewayTelemetryIngest, RuntimeTunnelManager, RuntimeTunnelStat,
    TelemetryEnvelope, TunnelEndpointSide, TunnelKind, MANAGED_BIRD2_FILE,
};

use crate::{gateway_client::GatewayDispatchClient, model::PromoteTelemetryTunnelRequest};

#[tokio::test]
async fn promote_telemetry_tunnel_creates_external_observed_plan_and_audit() {
    let repo = Repository::Memory(MemoryState::default());
    seed_tunnel_telemetry(
        &repo,
        RuntimeTunnelStat {
            interface: "wg42".to_string(),
            kind: "wireguard".to_string(),
            ownership_mode: "runtime_observed".to_string(),
            mutation_policy: "observe_only_import_candidate".to_string(),
            promotion_required: true,
            source: "sysfs_proc_net_dev".to_string(),
            operstate: Some("up".to_string()),
            mtu: Some(1420),
            link_type: None,
            address: None,
            rx_bytes: 100,
            tx_bytes: 200,
            ..RuntimeTunnelStat::default()
        },
    )
    .await;
    let state = test_state(repo.clone());
    let headers = crate::test_auth_headers(&state).await;

    let (status, Json(view)) = crate::routes_network::promote_telemetry_tunnel_plan(
        State(state),
        headers,
        Json(PromoteTelemetryTunnelRequest {
            client_id: "left-a".to_string(),
            interface: "wg42".to_string(),
            peer_client_id: "right-b".to_string(),
            local_underlay: "198.51.100.10".to_string(),
            peer_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            side: Some(TunnelEndpointSide::Left),
            name: Some("wg42-import".to_string()),
            topology_version: Some("telemetry-import:wg42".to_string()),
            bandwidth: Some(BandwidthTier::M1000),
            latency_ms: Some(8.0),
            packet_loss_ratio: Some(0.0),
            preference: Some(2.0),
        }),
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(view.name, "wg42-import");
    assert_eq!(view.kind, TunnelKind::Wireguard);
    assert_eq!(view.left_client_id, "left-a");
    assert_eq!(view.right_client_id, "right-b");
    assert_eq!(
        view.plan.runtime_control.manager,
        RuntimeTunnelManager::ExternalObserved
    );
    assert_eq!(
        view.plan.runtime_topology.desired_interfaces,
        vec!["wg42".to_string()]
    );
    assert!(!view.plan.mutates_host);
    assert_eq!(
        view.plan.touched_files,
        vec![MANAGED_BIRD2_FILE.to_string()]
    );

    let audits = repo.list_audit_logs(10).await.unwrap();
    assert!(audits
        .iter()
        .any(|audit| audit.action == "network.tunnel_plan_created"));
    let promotion_audit = audits
        .iter()
        .find(|audit| audit.action == "network.tunnel_plan_promoted_from_telemetry")
        .unwrap();
    assert_eq!(promotion_audit.metadata["interface"], "wg42");
    assert_eq!(
        promotion_audit.metadata["mutation_policy"],
        "observe_only_import_candidate"
    );
}

#[tokio::test]
async fn promote_telemetry_tunnel_rejects_non_import_candidate() {
    let repo = Repository::Memory(MemoryState::default());
    seed_tunnel_telemetry(
        &repo,
        RuntimeTunnelStat {
            interface: "wg42".to_string(),
            kind: "wireguard".to_string(),
            ownership_mode: "managed_desired".to_string(),
            mutation_policy: "managed_desired".to_string(),
            promotion_required: false,
            source: "sysfs_proc_net_dev".to_string(),
            operstate: Some("up".to_string()),
            mtu: Some(1420),
            link_type: None,
            address: None,
            rx_bytes: 100,
            tx_bytes: 200,
            ..RuntimeTunnelStat::default()
        },
    )
    .await;
    let state = test_state(repo);
    let headers = crate::test_auth_headers(&state).await;

    let error = crate::routes_network::promote_telemetry_tunnel_plan(
        State(state),
        headers,
        Json(PromoteTelemetryTunnelRequest {
            client_id: "left-a".to_string(),
            interface: "wg42".to_string(),
            peer_client_id: "right-b".to_string(),
            local_underlay: "198.51.100.10".to_string(),
            peer_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            side: None,
            name: None,
            topology_version: None,
            bandwidth: None,
            latency_ms: None,
            packet_loss_ratio: None,
            preference: None,
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "telemetry_tunnel_not_import_candidate");
}

async fn seed_tunnel_telemetry(repo: &Repository, tunnel: RuntimeTunnelStat) {
    repo.record_telemetry(&GatewayTelemetryIngest {
        gateway_id: "gateway-a".to_string(),
        remote_ip: None,
        telemetry: TelemetryEnvelope {
            client_id: "left-a".to_string(),
            metrics: AgentMetrics {
                observed_unix: 1_800_000_000,
                hostname: "left-a".to_string(),
                tunnels: vec![tunnel],
                ..AgentMetrics::default()
            },
        },
    })
    .await
    .unwrap();
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
    }
}
