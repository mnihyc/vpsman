use super::*;
use sha2::{Digest, Sha256};
use vpsman_common::{
    plan_tunnel, AgentMetrics, GatewayTelemetryIngest, RuntimeTunnelAdapterHealthStat,
    RuntimeTunnelControl, RuntimeTunnelManager, RuntimeTunnelStat, TelemetryEnvelope,
    TunnelAddressPair, TunnelKind, TunnelPlanInput,
};

#[tokio::test]
async fn undeclared_tunnel_telemetry_is_not_exposed() {
    let repo = Repository::Memory(MemoryState::default());
    seed_tunnel_telemetry(
        &repo,
        "edge-a",
        RuntimeTunnelStat {
            interface: "wg0".to_string(),
            kind: "wireguard".to_string(),
            ownership_mode: "external_observed".to_string(),
            mutation_policy: "observe_only_saved_plan".to_string(),
            source: "approved_runtime_status_telemetry".to_string(),
            rx_bytes: 123,
            tx_bytes: 456,
            ..RuntimeTunnelStat::default()
        },
    )
    .await;

    assert!(repo
        .list_telemetry_tunnels(10, Some("edge-a"), None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn declared_tunnel_telemetry_keeps_exact_plan_and_endpoint_identity() {
    let repo = Repository::Memory(MemoryState::default());
    let plan_id = seed_declared_plan(&repo, RuntimeTunnelManager::ExternalObserved).await;
    seed_tunnel_telemetry(
        &repo,
        "edge-a",
        RuntimeTunnelStat {
            interface: "wg0".to_string(),
            kind: "wireguard".to_string(),
            ownership_mode: "external_observed".to_string(),
            mutation_policy: "observe_only_saved_plan".to_string(),
            source: "approved_runtime_status_telemetry".to_string(),
            rx_bytes: 123,
            tx_bytes: 456,
            plan_id: Some(plan_id.to_string()),
            plan_name: Some("edge-a-edge-b".to_string()),
            plan_runtime_manager: Some("external_observed".to_string()),
            endpoint_side: Some("left".to_string()),
            peer_client_id: Some("edge-b".to_string()),
            traffic_source: Some("interface_counters".to_string()),
            traffic_status: Some("ok".to_string()),
            ..RuntimeTunnelStat::default()
        },
    )
    .await;

    let tunnels = repo
        .list_telemetry_tunnels(10, Some("edge-a"), Some("wg0"))
        .await
        .unwrap();
    assert_eq!(tunnels.len(), 1);
    assert_eq!(tunnels[0].plan_id, Some(plan_id));
    assert_eq!(tunnels[0].plan_name.as_deref(), Some("edge-a-edge-b"));
    assert_eq!(tunnels[0].endpoint_side.as_deref(), Some("left"));
    assert_eq!(tunnels[0].peer_client_id.as_deref(), Some("edge-b"));
    assert_eq!(tunnels[0].rx_bytes, 123);
    assert_eq!(tunnels[0].tx_bytes, 456);
}

#[tokio::test]
async fn disabled_tunnel_plan_stops_exposing_stale_telemetry() {
    let repo = Repository::Memory(MemoryState::default());
    let plan_id = seed_declared_plan(&repo, RuntimeTunnelManager::ExternalObserved).await;
    seed_tunnel_telemetry(
        &repo,
        "edge-a",
        RuntimeTunnelStat {
            interface: "wg0".to_string(),
            kind: "wireguard".to_string(),
            ownership_mode: "external_observed".to_string(),
            mutation_policy: "observe_only_saved_plan".to_string(),
            source: "approved_runtime_status_telemetry".to_string(),
            plan_id: Some(plan_id.to_string()),
            plan_name: Some("edge-a-edge-b".to_string()),
            endpoint_side: Some("left".to_string()),
            peer_client_id: Some("edge-b".to_string()),
            ..RuntimeTunnelStat::default()
        },
    )
    .await;

    repo.set_tunnel_plan_enabled(plan_id, 1, false, &test_operator())
        .await
        .unwrap();

    assert!(repo
        .list_telemetry_tunnels(10, Some("edge-a"), Some("wg0"))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn adapter_health_output_is_redacted_to_hashes() {
    let repo = Repository::Memory(MemoryState::default());
    let plan_id = seed_declared_plan(&repo, RuntimeTunnelManager::ExternalManagedAdapter).await;
    let stdout = b"private adapter output";
    let stderr = b"private adapter error";
    let stdout_hash = hex::encode(Sha256::digest(stdout));
    let stderr_hash = hex::encode(Sha256::digest(stderr));
    seed_tunnel_telemetry(
        &repo,
        "edge-a",
        RuntimeTunnelStat {
            interface: "wg0".to_string(),
            kind: "wireguard".to_string(),
            ownership_mode: "external_managed_adapter".to_string(),
            mutation_policy: "managed_desired".to_string(),
            source: "approved_runtime_status_telemetry".to_string(),
            plan_id: Some(plan_id.to_string()),
            plan_name: Some("edge-a-edge-b".to_string()),
            endpoint_side: Some("left".to_string()),
            peer_client_id: Some("edge-b".to_string()),
            adapter_health: Some(RuntimeTunnelAdapterHealthStat {
                status: "healthy".to_string(),
                checked_unix: 1_800_000_000,
                configured: true,
                success: true,
                exit_code: Some(0),
                stdout_sha256_hex: Some(stdout_hash.clone()),
                stderr_sha256_hex: Some(stderr_hash.clone()),
                ..RuntimeTunnelAdapterHealthStat::default()
            }),
            ..RuntimeTunnelStat::default()
        },
    )
    .await;

    let health = repo
        .list_telemetry_tunnels(10, Some("edge-a"), Some("wg0"))
        .await
        .unwrap()[0]
        .adapter_health
        .clone()
        .unwrap();
    assert_eq!(
        health.stdout_sha256_hex.as_deref(),
        Some(stdout_hash.as_str())
    );
    assert_eq!(
        health.stderr_sha256_hex.as_deref(),
        Some(stderr_hash.as_str())
    );
}

#[tokio::test]
async fn telemetry_sequence_is_idempotent_per_gateway_session() {
    let memory = MemoryState::default();
    let webhook_events = memory.webhook_events.clone();
    let repo = Repository::Memory(memory);
    let process_incarnation_id = uuid::Uuid::new_v4();
    let mut event = GatewayTelemetryIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id: uuid::Uuid::new_v4(),
        process_incarnation_id,
        telemetry_seq: 2,
        remote_ip: None,
        telemetry: TelemetryEnvelope {
            client_id: "edge-a".to_string(),
            metrics: AgentMetrics {
                observed_unix: 1_800_000_000,
                hostname: "edge-a".to_string(),
                cpu: vpsman_common::CpuStat {
                    load: vpsman_common::LoadAverage {
                        one: 1.0,
                        five: 0.8,
                        fifteen: 0.5,
                    },
                    cores: 2,
                },
                ..AgentMetrics::default()
            },
        },
    };

    assert!(repo.record_telemetry(&event).await.unwrap());
    assert!(!repo.record_telemetry(&event).await.unwrap());
    event.telemetry_seq = 1;
    event.telemetry.metrics.cpu.load.one = 99.0;
    assert!(!repo.record_telemetry(&event).await.unwrap());
    event.telemetry_seq = 3;
    event.telemetry.metrics.cpu.load.one = 3.0;
    assert!(repo.record_telemetry(&event).await.unwrap());

    let rollups = repo
        .list_telemetry_rollups(10, Some("edge-a"), Some(60))
        .await
        .unwrap();
    assert_eq!(rollups.len(), 1);
    assert_eq!(rollups[0].sample_count, 2);
    assert_eq!(rollups[0].cpu_load_1_avg, 2.0);

    event.gateway_session_id = uuid::Uuid::new_v4();
    event.telemetry_seq = 1;
    assert!(repo.record_telemetry(&event).await.unwrap());

    event.process_incarnation_id = uuid::Uuid::new_v4();
    event.telemetry_seq = 1;
    assert!(repo.record_telemetry(&event).await.unwrap());
    assert_eq!(
        repo.list_telemetry_rollups(10, Some("edge-a"), Some(60))
            .await
            .unwrap()[0]
            .sample_count,
        4
    );
    let event_ids = webhook_events
        .read()
        .await
        .iter()
        .map(|event| event.event_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(event_ids.len(), 4);
    assert!(event_ids.iter().any(|event_id| event_id.ends_with(":2")));
    assert!(event_ids.iter().any(|event_id| event_id.ends_with(":3")));
}

async fn seed_declared_plan(repo: &Repository, manager: RuntimeTunnelManager) -> uuid::Uuid {
    let runtime_control = RuntimeTunnelControl {
        manager,
        left_adapter_template_id: (manager == RuntimeTunnelManager::ExternalManagedAdapter)
            .then(|| "11111111-1111-4111-8111-111111111111".to_string()),
        right_adapter_template_id: (manager == RuntimeTunnelManager::ExternalManagedAdapter)
            .then(|| "22222222-2222-4222-8222-222222222222".to_string()),
        ..RuntimeTunnelControl::default()
    };
    let input = TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "wg0".to_string(),
        kind: TunnelKind::Wireguard,
        runtime_control,
        runtime_topology: Default::default(),
        left_client_id: "edge-a".to_string(),
        right_client_id: "edge-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        ospf: None,
    };
    let plan = plan_tunnel(&input).unwrap();
    repo.record_tunnel_plan(&input, &plan, true, &test_operator())
        .await
        .unwrap()
        .id
}

fn test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: uuid::Uuid::nil(),
            username: "network-telemetry-test".to_string(),
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
        session_id: uuid::Uuid::nil(),
    }
}

async fn seed_tunnel_telemetry(repo: &Repository, client_id: &str, tunnel: RuntimeTunnelStat) {
    repo.record_telemetry(&GatewayTelemetryIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id: uuid::Uuid::new_v4(),
        process_incarnation_id: uuid::Uuid::new_v4(),
        telemetry_seq: 2,
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
