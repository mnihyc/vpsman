use super::*;
use uuid::Uuid;
use vpsman_common::{
    plan_tunnel, AgentMetrics, DiskStat, GatewayTelemetryIngest, NetworkStat, OspfCostPolicy,
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
async fn telemetry_ingest_averages_multiple_events_into_one_minute_summary() {
    let repo = Repository::Memory(MemoryState::default());
    seed_resource_telemetry(
        &repo,
        "edge-a",
        AgentMetrics {
            observed_unix: 1_800_000_010,
            hostname: "edge-a".to_string(),
            cpu: vpsman_common::CpuStat {
                load: vpsman_common::LoadAverage {
                    one: 0.5,
                    ..Default::default()
                },
                ..Default::default()
            },
            memory: vpsman_common::MemoryStat {
                total_bytes: 1_000,
                available_bytes: 800,
            },
            disks: vec![DiskStat {
                mountpoint: "/".to_string(),
                total_bytes: 100,
                available_bytes: 70,
            }],
            networks: vec![NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes: 1_000,
                tx_bytes: 2_000,
            }],
            ..AgentMetrics::default()
        },
    )
    .await;
    seed_resource_telemetry(
        &repo,
        "edge-a",
        AgentMetrics {
            observed_unix: 1_800_000_020,
            hostname: "edge-a".to_string(),
            cpu: vpsman_common::CpuStat {
                load: vpsman_common::LoadAverage {
                    one: 1.5,
                    ..Default::default()
                },
                ..Default::default()
            },
            memory: vpsman_common::MemoryStat {
                total_bytes: 1_200,
                available_bytes: 600,
            },
            disks: vec![DiskStat {
                mountpoint: "/".to_string(),
                total_bytes: 100,
                available_bytes: 50,
            }],
            networks: vec![NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes: 4_000,
                tx_bytes: 8_000,
            }],
            ..AgentMetrics::default()
        },
    )
    .await;

    let rollups = repo
        .list_telemetry_rollups(10, Some("edge-a"), Some(60))
        .await
        .unwrap();
    assert_eq!(rollups.len(), 1);
    assert_eq!(rollups[0].bucket_secs, 60);
    assert_eq!(rollups[0].sample_count, 2);
    assert_eq!(rollups[0].cpu_load_1_avg, 1.0);
    assert_eq!(rollups[0].memory_total_bytes_max, 1_200);
    assert_eq!(rollups[0].memory_available_bytes_avg, 700);
    assert_eq!(rollups[0].disk_available_bytes_avg, 60);

    let rates = repo
        .list_telemetry_network_rates(10, Some("edge-a"), Some("eth0"), Some(60))
        .await
        .unwrap();
    assert_eq!(rates.len(), 1);
    assert_eq!(rates[0].bucket_secs, 60);
    assert_eq!(rates[0].sample_count, 2);
    assert_eq!(rates[0].rx_bytes_avg, 2_500);
    assert_eq!(rates[0].tx_bytes_avg, 5_000);
    assert_eq!(rates[0].rx_bytes_delta, 0);
    assert_eq!(rates[0].tx_bytes_delta, 0);
    assert_eq!(rates[0].rx_bps_avg, 0.0);
    assert_eq!(rates[0].tx_bps_avg, 0.0);
}

#[tokio::test]
async fn telemetry_network_rates_are_derived_between_minute_summaries() {
    let repo = Repository::Memory(MemoryState::default());
    seed_resource_telemetry(
        &repo,
        "edge-a",
        AgentMetrics {
            observed_unix: 1_800_000_010,
            hostname: "edge-a".to_string(),
            networks: vec![NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes: 1_000,
                tx_bytes: 2_000,
            }],
            ..AgentMetrics::default()
        },
    )
    .await;
    seed_resource_telemetry(
        &repo,
        "edge-a",
        AgentMetrics {
            observed_unix: 1_800_000_070,
            hostname: "edge-a".to_string(),
            networks: vec![NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes: 4_000,
                tx_bytes: 8_000,
            }],
            ..AgentMetrics::default()
        },
    )
    .await;

    let rates = repo
        .list_telemetry_network_rates(10, Some("edge-a"), Some("eth0"), Some(60))
        .await
        .unwrap();
    let latest = rates
        .iter()
        .find(|rate| rate.bucket_start == "1800000060")
        .unwrap();
    assert_eq!(latest.rx_bytes_avg, 4_000);
    assert_eq!(latest.tx_bytes_avg, 8_000);
    assert_eq!(latest.rx_bytes_delta, 3_000);
    assert_eq!(latest.tx_bytes_delta, 6_000);
    assert_eq!(latest.rx_bps_avg, 400.0);
    assert_eq!(latest.tx_bps_avg, 800.0);
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
    repo.record_tunnel_plan(&input, &plan, true, &operator_context())
        .await
        .unwrap()
}

fn operator_context() -> AuthContext {
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
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.42.0.0".to_string(),
            right: "10.42.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
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
        gateway_session_id: uuid::Uuid::new_v4(),
        process_incarnation_id: uuid::Uuid::new_v4(),
        remote_ip: None,
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

async fn seed_resource_telemetry(repo: &Repository, client_id: &str, metrics: AgentMetrics) {
    repo.record_telemetry(&GatewayTelemetryIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id: uuid::Uuid::new_v4(),
        process_incarnation_id: uuid::Uuid::new_v4(),
        remote_ip: None,
        telemetry: TelemetryEnvelope {
            client_id: client_id.to_string(),
            metrics,
        },
    })
    .await
    .unwrap();
}
