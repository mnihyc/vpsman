use super::*;
use sha2::{Digest, Sha256};
use vpsman_common::{
    plan_tunnel, AgentCapabilitySnapshot, AgentHello, AgentMetrics, AgentPrivilegeMode, DiskStat,
    GatewayTelemetryIngest, MemoryStat, PortForwardRuntimeSnapshot, PortForwardRuntimeStatus,
    RuntimeTunnelAdapterHealthStat, RuntimeTunnelControl, RuntimeTunnelManager, RuntimeTunnelStat,
    TelemetryEnvelope, TunnelAddressPair, TunnelKind, TunnelPlanInput,
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
    let plan_id = seed_declared_plan(&repo, RuntimeTunnelManager::CustomAdapter).await;
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
            ownership_mode: "custom_adapter".to_string(),
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
    let port_forward_runtime = memory.port_forward_runtime.clone();
    let repo = Repository::Memory(memory.clone());
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
                    utilization_ratio: None,
                },
                memory: MemoryStat {
                    total_bytes: 200,
                    available_bytes: 50,
                    swap_total_bytes: Some(200),
                    swap_available_bytes: Some(50),
                },
                disks: vec![DiskStat {
                    mountpoint: "/".to_string(),
                    total_bytes: 200,
                    available_bytes: 50,
                }],
                port_forwarding: Some(PortForwardRuntimeSnapshot {
                    status: PortForwardRuntimeStatus::Absent,
                    observed_unix: 1_800_000_000,
                    ..PortForwardRuntimeSnapshot::default()
                }),
                ..AgentMetrics::default()
            },
        },
    };
    seed_memory_telemetry_source(
        &memory,
        &event.telemetry.client_id,
        &event.gateway_id,
        event.gateway_session_id,
        event.process_incarnation_id,
    )
    .await;

    assert!(repo.record_telemetry(&event).await.unwrap());
    port_forward_runtime.write().await.clear();
    assert!(!repo.record_telemetry(&event).await.unwrap());
    assert!(port_forward_runtime.read().await.contains_key("edge-a"));
    event.telemetry_seq = 1;
    event.telemetry.metrics.cpu.load.one = 99.0;
    event.telemetry.metrics.cpu.cores = 64;
    event.telemetry.metrics.memory = MemoryStat {
        total_bytes: 10_000,
        available_bytes: 0,
        swap_total_bytes: Some(10_000),
        swap_available_bytes: Some(0),
    };
    event.telemetry.metrics.disks[0].total_bytes = 10_000;
    event.telemetry.metrics.disks[0].available_bytes = 0;
    assert!(!repo.record_telemetry(&event).await.unwrap());
    event.telemetry_seq = 3;
    event.telemetry.metrics.cpu.load.one = 3.0;
    event.telemetry.metrics.cpu.cores = 4;
    event.telemetry.metrics.memory = MemoryStat {
        total_bytes: 100,
        available_bytes: 75,
        swap_total_bytes: Some(100),
        swap_available_bytes: Some(75),
    };
    event.telemetry.metrics.disks[0].total_bytes = 100;
    event.telemetry.metrics.disks[0].available_bytes = 75;
    assert!(repo.record_telemetry(&event).await.unwrap());

    let rollups = repo
        .list_telemetry_rollups(10, Some("edge-a"), Some(60), false)
        .await
        .unwrap();
    assert_eq!(rollups.len(), 1);
    assert_eq!(rollups[0].sample_count, 2);
    assert_eq!(rollups[0].cpu_load_1_avg, 2.0);
    assert_eq!(rollups[0].cpu_cores_max, 4);
    assert_eq!(rollups[0].memory_total_bytes_max, 200);
    assert_eq!(rollups[0].memory_available_bytes_avg, 63);
    assert_eq!(rollups[0].memory_available_bytes_min, 50);
    assert!((rollups[0].memory_used_ratio_avg - 0.5).abs() < f64::EPSILON);
    assert!((rollups[0].memory_used_ratio_max - 0.75).abs() < f64::EPSILON);
    assert_eq!(rollups[0].swap_sample_count, 2);
    assert_eq!(rollups[0].swap_total_bytes_max, Some(200));
    assert_eq!(rollups[0].swap_available_bytes_avg, Some(63));
    assert_eq!(rollups[0].swap_available_bytes_min, Some(50));
    assert!((rollups[0].swap_used_ratio_avg.unwrap() - 0.5).abs() < f64::EPSILON);
    assert!((rollups[0].swap_used_ratio_max.unwrap() - 0.75).abs() < f64::EPSILON);
    assert_eq!(rollups[0].disk_total_bytes_max, 200);
    assert_eq!(rollups[0].disk_available_bytes_avg, 63);
    assert_eq!(rollups[0].disk_available_bytes_min, 50);
    assert!((rollups[0].disk_used_ratio_avg - 0.5).abs() < f64::EPSILON);
    assert!((rollups[0].disk_used_ratio_max - 0.75).abs() < f64::EPSILON);

    event.gateway_session_id = uuid::Uuid::new_v4();
    event.telemetry_seq = 1;
    seed_active_memory_gateway_session(
        &memory,
        &event.telemetry.client_id,
        &event.gateway_id,
        event.gateway_session_id,
    )
    .await;
    assert!(repo.record_telemetry(&event).await.unwrap());

    event.process_incarnation_id = uuid::Uuid::new_v4();
    event.telemetry_seq = 1;
    seed_visible_memory_agent(
        &memory,
        &event.telemetry.client_id,
        event.process_incarnation_id,
    )
    .await;
    assert!(repo.record_telemetry(&event).await.unwrap());
    assert_eq!(
        repo.list_telemetry_rollups(10, Some("edge-a"), Some(60), false)
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

    let before_swap_removal = repo
        .list_telemetry_rollups(10, Some("edge-a"), Some(60), false)
        .await
        .unwrap()
        .remove(0);
    event.telemetry_seq = 2;
    event.telemetry.metrics.memory.swap_total_bytes = Some(0);
    event.telemetry.metrics.memory.swap_available_bytes = Some(0);
    assert!(repo.record_telemetry(&event).await.unwrap());
    let after_swap_removal = repo
        .list_telemetry_rollups(10, Some("edge-a"), Some(60), false)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(after_swap_removal.sample_count, 5);
    assert_eq!(
        after_swap_removal.swap_sample_count,
        before_swap_removal.swap_sample_count
    );
    assert_eq!(
        after_swap_removal.swap_total_bytes_max,
        before_swap_removal.swap_total_bytes_max
    );
    assert_eq!(
        after_swap_removal.swap_available_bytes_avg,
        before_swap_removal.swap_available_bytes_avg
    );
    assert_eq!(
        after_swap_removal.swap_available_bytes_min,
        before_swap_removal.swap_available_bytes_min
    );
    assert_eq!(
        after_swap_removal.swap_used_ratio_avg,
        before_swap_removal.swap_used_ratio_avg
    );
    assert_eq!(
        after_swap_removal.swap_used_ratio_max,
        before_swap_removal.swap_used_ratio_max
    );
}

#[tokio::test]
async fn memory_telemetry_touch_preserves_agent_identity_and_capabilities() {
    let memory = MemoryState::default();
    let process_incarnation_id = uuid::Uuid::new_v4();
    let gateway_session_id = uuid::Uuid::new_v4();
    let capabilities = AgentCapabilitySnapshot {
        privilege_mode: AgentPrivilegeMode::Root,
        effective_uid: Some(0),
        can_attempt_privileged_ops: true,
        can_manage_runtime_tunnels: true,
        can_apply_process_limits: true,
        ..AgentCapabilitySnapshot::default()
    };
    memory.agents.write().await.push(crate::model::AgentView {
        id: "edge-a".to_string(),
        display_name: "Edge A".to_string(),
        status: "offline".to_string(),
        tags: vec!["edge".to_string()],
        registration_ip: Some("198.51.100.1".to_string()),
        last_ip: Some("198.51.100.1".to_string()),
        last_seen_at: None,
        arch: Some("aarch64".to_string()),
        internal_build_number: 42,
        process_incarnation_id: Some(process_incarnation_id),
        stale_since: None,
        stale_reason: None,
        capabilities: capabilities.clone(),
    });
    seed_active_memory_gateway_session(&memory, "edge-a", "gateway-a", gateway_session_id).await;
    let repo = Repository::Memory(memory.clone());

    assert!(repo
        .record_telemetry(&GatewayTelemetryIngest {
            gateway_id: "gateway-a".to_string(),
            gateway_session_id,
            process_incarnation_id,
            telemetry_seq: 1,
            remote_ip: Some("2001:db8::20".to_string()),
            telemetry: TelemetryEnvelope {
                client_id: "edge-a".to_string(),
                metrics: AgentMetrics::default(),
            },
        })
        .await
        .unwrap());

    let agents = memory.agents.read().await;
    let agent = &agents[0];
    assert_eq!(agent.status, "online");
    assert_eq!(agent.registration_ip.as_deref(), Some("198.51.100.1"));
    assert_eq!(agent.last_ip.as_deref(), Some("2001:db8::20"));
    assert_eq!(agent.arch.as_deref(), Some("aarch64"));
    assert_eq!(agent.internal_build_number, 42);
    assert_eq!(agent.process_incarnation_id, Some(process_incarnation_id));
    assert_eq!(agent.capabilities, capabilities);
}

#[tokio::test]
async fn intra_minute_counter_reset_remains_a_gap_after_recovery_above_the_old_value() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let gateway_session_id = uuid::Uuid::new_v4();
    let process_incarnation_id = uuid::Uuid::new_v4();
    let minute = crate::unix_now() / 60 * 60;
    seed_memory_telemetry_source(
        &memory,
        "v-1",
        "gateway-a",
        gateway_session_id,
        process_incarnation_id,
    )
    .await;
    memory.traffic_counter_samples.write().await.push(
        crate::model_alert_policies::TrafficCounterSampleRecord {
            client_id: "v-1".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            observed_at: (minute - 60).to_string(),
            observed_unix: (minute - 60) as i64,
            rx_bytes: 1_000,
            tx_bytes: 2_000,
            rx_counter_epoch: 0,
            tx_counter_epoch: 0,
            sample_source: "test".to_string(),
        },
    );
    memory
        .telemetry_network_rates
        .write()
        .await
        .push(crate::model::TelemetryNetworkRateView {
            client_id: "v-1".to_string(),
            interface: "eth0".to_string(),
            bucket_start: (minute - 60).to_string(),
            bucket_secs: 60,
            sample_count: 1,
            rx_bytes_avg: 1_000,
            tx_bytes_avg: 2_000,
            rx_bytes_last: 1_000,
            tx_bytes_last: 2_000,
            rx_counter_epoch: 0,
            tx_counter_epoch: 0,
            rx_bytes_delta: 0,
            tx_bytes_delta: 0,
            rx_bps_avg: 0.0,
            tx_bps_avg: 0.0,
            updated_at: (minute - 60).to_string(),
        });
    for (sequence, rx_bytes, tx_bytes) in [(1, 100, 2_100), (2, 1_200, 2_200)] {
        assert!(repo
            .record_telemetry(&GatewayTelemetryIngest {
                gateway_id: "gateway-a".to_string(),
                gateway_session_id,
                process_incarnation_id,
                telemetry_seq: sequence,
                remote_ip: None,
                telemetry: TelemetryEnvelope {
                    client_id: "v-1".to_string(),
                    metrics: AgentMetrics {
                        observed_unix: minute,
                        hostname: "v-1".to_string(),
                        networks: vec![vpsman_common::NetworkStat {
                            interface: "eth0".to_string(),
                            rx_bytes,
                            tx_bytes,
                        }],
                        ..AgentMetrics::default()
                    },
                },
            })
            .await
            .unwrap());
    }

    let retained_reset = repo
        .list_dashboard_telemetry_network_rates(
            10,
            Some(minute),
            Some(minute + 59),
            Some(60),
            60,
            &["v-1".to_string()],
        )
        .await
        .unwrap();
    assert!(retained_reset.is_empty());

    let minute_row = memory
        .telemetry_network_rates
        .read()
        .await
        .iter()
        .find(|row| row.bucket_start == minute.to_string())
        .cloned()
        .unwrap();
    assert_eq!(minute_row.rx_bytes_last, 1_200);
    assert_eq!(minute_row.rx_counter_epoch, 1);
    assert_eq!(minute_row.tx_counter_epoch, 0);
    memory
        .telemetry_network_rates
        .write()
        .await
        .push(crate::model::TelemetryNetworkRateView {
            client_id: "v-1".to_string(),
            interface: "eth0".to_string(),
            bucket_start: (minute + 60).to_string(),
            bucket_secs: 60,
            sample_count: 1,
            rx_bytes_avg: 1_300,
            tx_bytes_avg: 2_300,
            rx_bytes_last: 1_300,
            tx_bytes_last: 2_300,
            rx_counter_epoch: 1,
            tx_counter_epoch: 0,
            rx_bytes_delta: 0,
            tx_bytes_delta: 0,
            rx_bps_avg: 0.0,
            tx_bps_avg: 0.0,
            updated_at: (minute + 60).to_string(),
        });

    let retained_recovery = repo
        .list_dashboard_telemetry_network_rates(
            10,
            Some(minute + 60),
            Some(minute + 119),
            Some(60),
            60,
            &["v-1".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(
        retained_recovery.len(),
        1,
        "retained: {retained_recovery:?}"
    );
    assert_eq!(retained_recovery[0].rx_bytes_delta, 100);
    assert_eq!(retained_recovery[0].tx_bytes_delta, 100);
}

async fn seed_declared_plan(repo: &Repository, manager: RuntimeTunnelManager) -> uuid::Uuid {
    if let Repository::Memory(memory) = repo {
        for client_id in ["edge-a", "edge-b"] {
            seed_visible_memory_agent(memory, client_id, uuid::Uuid::new_v4()).await;
        }
    }
    let runtime_control = RuntimeTunnelControl {
        manager,
        left_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
            .then(|| "11111111-1111-4111-8111-111111111111".to_string()),
        right_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
            .then(|| "22222222-2222-4222-8222-222222222222".to_string()),
        ..RuntimeTunnelControl::default()
    };
    let endpoint_mtu = if manager == RuntimeTunnelManager::AgentBuiltin {
        vpsman_common::default_tunnel_mtu(TunnelKind::Wireguard)
    } else {
        None
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
        left_mtu: endpoint_mtu,
        right_mtu: endpoint_mtu,
        ospf: None,
    };
    crate::tests_network::seed_test_plan_adapter_definitions(repo, &input).await;
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
        session_id: None,
    }
}

async fn seed_tunnel_telemetry(repo: &Repository, client_id: &str, tunnel: RuntimeTunnelStat) {
    let gateway_session_id = uuid::Uuid::new_v4();
    let process_incarnation_id = uuid::Uuid::new_v4();
    if let Repository::Memory(memory) = repo {
        seed_memory_telemetry_source(
            memory,
            client_id,
            "gateway-a",
            gateway_session_id,
            process_incarnation_id,
        )
        .await;
    }
    repo.record_telemetry(&GatewayTelemetryIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id,
        process_incarnation_id,
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

async fn seed_memory_telemetry_source(
    memory: &MemoryState,
    client_id: &str,
    gateway_id: &str,
    gateway_session_id: uuid::Uuid,
    process_incarnation_id: uuid::Uuid,
) {
    seed_visible_memory_agent(memory, client_id, process_incarnation_id).await;
    seed_active_memory_gateway_session(memory, client_id, gateway_id, gateway_session_id).await;
}

async fn seed_visible_memory_agent(
    memory: &MemoryState,
    client_id: &str,
    process_incarnation_id: uuid::Uuid,
) {
    crate::repository_ingest::upsert_memory_agent(
        &memory.agents,
        &AgentHello {
            client_id: client_id.to_string(),
            process_incarnation_id,
            agent_version: "test".to_string(),
            os_release: "test".to_string(),
            arch: "x86_64".to_string(),
            cpu_model: None,
            kernel_release: None,
            virtualization: None,
            update_heartbeat: None,
            internal_build_number: 1,
            capabilities: AgentCapabilitySnapshot::default(),
        },
    )
    .await;
}

async fn seed_active_memory_gateway_session(
    memory: &MemoryState,
    client_id: &str,
    gateway_id: &str,
    gateway_session_id: uuid::Uuid,
) {
    memory
        .gateway_sessions
        .write()
        .await
        .push(crate::model::GatewaySessionView {
            id: gateway_session_id,
            gateway_id: gateway_id.to_string(),
            client_id: client_id.to_string(),
            noise_public_key_hex: None,
            remote_ip: None,
            agent_version: "test".to_string(),
            status: "active".to_string(),
            started_at: "1800000000".to_string(),
            last_seen_at: "1800000000".to_string(),
            ended_at: None,
            end_reason: None,
        });
}
