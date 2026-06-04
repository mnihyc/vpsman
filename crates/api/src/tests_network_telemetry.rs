use super::*;
use uuid::Uuid;
use vpsman_common::{
    plan_tunnel, AgentMetrics, BandwidthTier, GatewayTelemetryIngest, OspfCostPolicy,
    RuntimeTunnelAdapterHealthStat, RuntimeTunnelControl, RuntimeTunnelManager, RuntimeTunnelStat,
    TelemetryEnvelope, TunnelKind, TunnelPlanInput,
};

#[tokio::test]
async fn telemetry_tunnel_readback_records_unmatched_import_candidate() {
    let repo = Repository::Memory(MemoryState::default());
    seed_tunnel_telemetry(
        &repo,
        "edge-a",
        RuntimeTunnelStat {
            interface: "tun0".to_string(),
            kind: "tun_tap".to_string(),
            ownership_mode: "runtime_observed".to_string(),
            mutation_policy: "observe_only_import_candidate".to_string(),
            promotion_required: true,
            source: "sysfs_proc_net_dev".to_string(),
            operstate: Some("up".to_string()),
            mtu: Some(1500),
            link_type: Some(65534),
            address: Some("00:00:00:00:00:00".to_string()),
            rx_bytes: 123,
            tx_bytes: 456,
            traffic_source: Some("interface_counters".to_string()),
            traffic_status: Some("ok".to_string()),
            ..RuntimeTunnelStat::default()
        },
    )
    .await;

    let tunnels = repo
        .list_telemetry_tunnels(10, Some("edge-a"), Some("tun0"))
        .await
        .unwrap();
    assert_eq!(tunnels.len(), 1);
    assert_eq!(tunnels[0].client_id, "edge-a");
    assert_eq!(tunnels[0].interface, "tun0");
    assert_eq!(tunnels[0].kind, "tun_tap");
    assert_eq!(tunnels[0].ownership_mode, "runtime_observed");
    assert_eq!(tunnels[0].mutation_policy, "observe_only_import_candidate");
    assert!(tunnels[0].promotion_required);
    assert_eq!(tunnels[0].plan_correlation, "unmatched_import_candidate");
    assert!(tunnels[0].plan_id.is_none());
    assert_eq!(tunnels[0].rx_bytes, 123);
    assert_eq!(tunnels[0].tx_bytes, 456);
    assert_eq!(
        tunnels[0].traffic_source.as_deref(),
        Some("interface_counters")
    );
    assert_eq!(tunnels[0].traffic_status.as_deref(), Some("ok"));
}

#[tokio::test]
async fn telemetry_tunnel_readback_correlates_saved_adapter_plan() {
    let repo = Repository::Memory(MemoryState::default());
    let saved = save_tunnel_plan(
        &repo,
        TunnelPlanInput {
            name: "adapter-a-b".to_string(),
            interface_name: "ovpn42".to_string(),
            kind: TunnelKind::Openvpn,
            runtime_control: RuntimeTunnelControl {
                manager: RuntimeTunnelManager::ExternalManagedAdapter,
                startup: Some(vpsman_common::RuntimeTunnelCommand {
                    argv: vec!["/usr/local/libexec/vpsman-openvpn-adapter".to_string()],
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..plan_input_defaults()
        },
    )
    .await;
    seed_tunnel_telemetry(
        &repo,
        "edge-a",
        runtime_observed_tunnel("ovpn42", "openvpn", 1024, 2048),
    )
    .await;

    let tunnels = repo
        .list_telemetry_tunnels(10, Some("edge-a"), Some("ovpn42"))
        .await
        .unwrap();

    assert_eq!(tunnels.len(), 1);
    assert_eq!(tunnels[0].plan_correlation, "matched_saved_plan");
    assert_eq!(tunnels[0].plan_id, Some(saved.id));
    assert_eq!(tunnels[0].plan_name.as_deref(), Some("adapter-a-b"));
    assert_eq!(
        tunnels[0].plan_runtime_manager.as_deref(),
        Some("external_managed_adapter")
    );
    assert_eq!(tunnels[0].endpoint_side.as_deref(), Some("left"));
    assert_eq!(tunnels[0].peer_client_id.as_deref(), Some("edge-b"));
    assert_eq!(tunnels[0].ownership_mode, "external_managed_adapter");
    assert_eq!(tunnels[0].mutation_policy, "managed_desired");
    assert!(!tunnels[0].promotion_required);
}

#[tokio::test]
async fn telemetry_tunnel_readback_keeps_external_observed_plan_observe_only() {
    let repo = Repository::Memory(MemoryState::default());
    let saved = save_tunnel_plan(
        &repo,
        TunnelPlanInput {
            name: "observed-wg-a-b".to_string(),
            interface_name: "wg42".to_string(),
            kind: TunnelKind::Wireguard,
            runtime_control: RuntimeTunnelControl {
                manager: RuntimeTunnelManager::ExternalObserved,
                ..Default::default()
            },
            ..plan_input_defaults()
        },
    )
    .await;
    seed_tunnel_telemetry(
        &repo,
        "edge-a",
        runtime_observed_tunnel("wg42", "wireguard", 10, 20),
    )
    .await;

    let tunnels = repo
        .list_telemetry_tunnels(10, Some("edge-a"), Some("wg42"))
        .await
        .unwrap();

    assert_eq!(tunnels.len(), 1);
    assert_eq!(tunnels[0].plan_correlation, "matched_saved_plan");
    assert_eq!(tunnels[0].plan_id, Some(saved.id));
    assert_eq!(
        tunnels[0].plan_runtime_manager.as_deref(),
        Some("external_observed")
    );
    assert_eq!(tunnels[0].ownership_mode, "external_observed");
    assert_eq!(tunnels[0].mutation_policy, "observe_only_saved_plan");
    assert!(!tunnels[0].promotion_required);
}

#[tokio::test]
async fn telemetry_tunnel_readback_exposes_redacted_adapter_health() {
    let repo = Repository::Memory(MemoryState::default());
    let mut tunnel = runtime_observed_tunnel("ovpn42", "openvpn", 10, 20);
    let command_hash = "a".repeat(64);
    let stdout_hash = "b".repeat(64);
    let stderr_hash = "c".repeat(64);
    tunnel.plan_name = Some("local-approved-adapter".to_string());
    tunnel.plan_runtime_manager = Some("external_managed_adapter".to_string());
    tunnel.endpoint_side = Some("left".to_string());
    tunnel.peer_client_id = Some("edge-b".to_string());
    tunnel.ownership_mode = "external_managed_adapter".to_string();
    tunnel.mutation_policy = "managed_desired".to_string();
    tunnel.promotion_required = false;
    tunnel.adapter_health = Some(RuntimeTunnelAdapterHealthStat {
        status: "healthy".to_string(),
        checked_unix: 1_800_000_010,
        configured: true,
        success: true,
        exit_code: Some(0),
        reason: None,
        duration_ms: 12,
        command_sha256_hex: Some(command_hash.clone()),
        timed_out: false,
        output_truncated: false,
        stdout_sha256_hex: Some(stdout_hash.clone()),
        stderr_sha256_hex: Some(stderr_hash.clone()),
    });
    seed_tunnel_telemetry(&repo, "edge-a", tunnel).await;

    let tunnels = repo
        .list_telemetry_tunnels(10, Some("edge-a"), Some("ovpn42"))
        .await
        .unwrap();

    assert_eq!(tunnels.len(), 1);
    assert_eq!(tunnels[0].plan_correlation, "telemetry_reported_plan");
    assert_eq!(
        tunnels[0].plan_name.as_deref(),
        Some("local-approved-adapter")
    );
    let health = tunnels[0].adapter_health.as_ref().unwrap();
    assert_eq!(health.status, "healthy");
    assert_eq!(health.checked_unix, 1_800_000_010);
    assert!(health.configured);
    assert!(health.success);
    assert_eq!(health.exit_code, Some(0));
    assert_eq!(health.duration_ms, 12);
    assert_eq!(
        health.command_sha256_hex.as_deref(),
        Some(command_hash.as_str())
    );
    assert_eq!(
        health.stdout_sha256_hex.as_deref(),
        Some(stdout_hash.as_str())
    );
    assert_eq!(
        health.stderr_sha256_hex.as_deref(),
        Some(stderr_hash.as_str())
    );
}

async fn save_tunnel_plan(repo: &Repository, input: TunnelPlanInput) -> TunnelPlanView {
    let plan = plan_tunnel(&input).unwrap();
    repo.record_tunnel_plan(&input, &plan, &operator_context())
        .await
        .unwrap()
}

fn operator_context() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "memory-dev".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            totp_enabled: false,
        },
        session_id: Uuid::nil(),
    }
}

fn plan_input_defaults() -> TunnelPlanInput {
    TunnelPlanInput {
        name: "edge-a-b".to_string(),
        interface_name: "tunab".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: RuntimeTunnelControl::default(),
        runtime_topology: Default::default(),
        left_client_id: "edge-a".to_string(),
        right_client_id: "edge-b".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "198.51.100.11".to_string(),
        address_pool_cidr: "10.42.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 12.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    }
}

fn runtime_observed_tunnel(
    interface: &str,
    kind: &str,
    rx_bytes: u64,
    tx_bytes: u64,
) -> RuntimeTunnelStat {
    RuntimeTunnelStat {
        interface: interface.to_string(),
        kind: kind.to_string(),
        ownership_mode: "runtime_observed".to_string(),
        mutation_policy: "observe_only_import_candidate".to_string(),
        promotion_required: true,
        source: "sysfs_proc_net_dev".to_string(),
        operstate: Some("up".to_string()),
        mtu: Some(1500),
        link_type: None,
        address: None,
        rx_bytes,
        tx_bytes,
        ..RuntimeTunnelStat::default()
    }
}

async fn seed_tunnel_telemetry(repo: &Repository, client_id: &str, tunnel: RuntimeTunnelStat) {
    repo.record_telemetry(&GatewayTelemetryIngest {
        gateway_id: "gateway-a".to_string(),
        telemetry: TelemetryEnvelope {
            client_id: client_id.to_string(),
            metrics: AgentMetrics {
                observed_unix: 1_800_000_000,
                hostname: client_id.to_string(),
                tunnels: vec![tunnel],
                ..AgentMetrics::default()
            },
        },
    })
    .await
    .unwrap();
}
