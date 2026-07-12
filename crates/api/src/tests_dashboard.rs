use super::*;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use vpsman_common::{AgentCapabilitySnapshot, AgentPrivilegeMode};

#[tokio::test]
async fn dashboard_overview_rejects_invalid_window() {
    let state = dashboard_test_state(Repository::Memory(MemoryState::default()));
    let headers = crate::test_auth_headers(&state).await;
    let error = routes_dashboard::dashboard_overview(
        State(state),
        headers,
        Query(routes_dashboard::DashboardOverviewQuery {
            chart_points: None,
            window: Some("90d".to_string()),
            ..dashboard_query_default()
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "invalid_dashboard_window");
}

#[tokio::test]
async fn dashboard_overview_counts_disconnected_and_never_connected_vpss_as_offline() {
    let repo = Repository::Memory(MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    let agent = |id: &str, status: &str| AgentView {
        id: id.to_string(),
        display_name: id.to_string(),
        status: status.to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: Default::default(),
    };
    memory.agents.write().await.extend([
        agent("disconnected", "disconnected"),
        agent("never", "never"),
    ]);
    let state = dashboard_test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let Json(view) = routes_dashboard::dashboard_overview(
        State(state),
        headers,
        Query(routes_dashboard::DashboardOverviewQuery {
            group_by: Some("status".to_string()),
            ..dashboard_query_default()
        }),
    )
    .await
    .unwrap();

    assert_eq!(view.summary.total, 2);
    assert_eq!(view.summary.online, 0);
    assert_eq!(view.summary.offline, 2);
    assert_eq!(view.summary.stale, 0);
    let disconnected_groups = view
        .label_clusters
        .iter()
        .filter(|cluster| matches!(cluster.label.as_str(), "disconnected" | "never"))
        .collect::<Vec<_>>();
    assert_eq!(disconnected_groups.len(), 2);
    assert!(disconnected_groups
        .iter()
        .all(|cluster| cluster.total == 1 && cluster.offline == 1));
}

#[tokio::test]
async fn system_dashboard_counts_agent_lost_lifecycle_failures() {
    let repo = Repository::Memory(MemoryState::default());
    let now = unix_now().to_string();
    if let Repository::Memory(memory) = &repo {
        memory.job_targets.write().await.push(JobTargetView {
            job_id: Uuid::new_v4(),
            client_id: "edge-a".to_string(),
            status: "agent_lost".to_string(),
            message: Some("agent process lost".to_string()),
            exit_code: None,
            started_at: Some(now.clone()),
            deadline_at: None,
            completed_at: Some(now),
            process_incarnation_id: None,
        });
    }
    let state = dashboard_test_state(repo);
    let headers = crate::test_auth_headers(&state).await;

    routes_system::record_system_dashboard_sample(&state)
        .await
        .unwrap();
    let Json(view) = routes_system::system_dashboard(
        State(state),
        headers,
        Query(routes_system::SystemDashboardQuery {
            window: Some("24h".to_string()),
            chart_points: Some(120),
        }),
    )
    .await
    .unwrap();

    assert_eq!(view.current.targets.agent_lost_last_24h, 1);
    assert!(view.series.iter().any(|series| {
        series.metric == "targets.agent_lost_last_24h"
            && series.label == "Agent lost"
            && series.points.iter().any(|point| point.latest_value == 1.0)
    }));
}

#[tokio::test]
async fn dashboard_overview_aggregates_memory_state() {
    let repo = Repository::Memory(MemoryState::default());
    let now_unix = unix_now();
    let removed_interface_previous = now_unix.saturating_sub(2 * 60 * 60).to_string();
    let previous = now_unix.saturating_sub(60 * 60).to_string();
    let now = now_unix.to_string();
    let job_id = Uuid::new_v4();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "edge-a".to_string(),
                display_name: "Edge A".to_string(),
                status: "online".to_string(),
                tags: vec!["provider:alpha".to_string(), "country:US".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: AgentCapabilitySnapshot::default(),
            },
            AgentView {
                id: "edge-b".to_string(),
                display_name: "Edge B".to_string(),
                status: "stale".to_string(),
                tags: vec!["country:US".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: AgentCapabilitySnapshot {
                    privilege_mode: AgentPrivilegeMode::Unprivileged,
                    effective_uid: Some(1000),
                    max_job_timeout_secs: 3600,
                    can_attempt_privileged_ops: true,
                    can_manage_runtime_tunnels: false,
                    can_apply_process_limits: false,
                    unprivileged_hint: Some("agent is running without root".to_string()),
                },
            },
        ]);
        memory.telemetry_rollups.write().await.extend([
            dashboard_test_rollup("edge-a", &now, 0.7, 1.1, 400, 1500),
            dashboard_test_rollup("edge-b", &now, 0.9, 1.6, 250, 1000),
        ]);
        memory.telemetry_network_rates.write().await.extend([
            dashboard_test_rate("edge-a", "eth0", &previous, 1_000, 1_000),
            dashboard_test_rate("edge-a", "eth0", &now, 9_000, 5_000),
            dashboard_test_rate("edge-b", "eth0", &previous, 2_000, 1_000),
            dashboard_test_rate("edge-b", "eth0", &now, 4_000, 10_000),
            dashboard_test_rate(
                "edge-a",
                "removed0",
                &removed_interface_previous,
                1_000,
                1_000,
            ),
            dashboard_test_rate(
                "edge-a",
                "removed0",
                &previous,
                1_000_000_000,
                1_000_000_000,
            ),
        ]);
        memory.jobs.write().await.push(JobHistoryView {
            id: job_id,
            actor_id: None,
            command_type: "shell".to_string(),
            source_schedule_id: None,
            privileged: true,
            status: "running".to_string(),
            target_count: 1,
            payload_hash: "aa".repeat(32),
            max_timeout_secs: 30,
            created_at: now.clone(),
            completed_at: None,
        });
        memory.job_targets.write().await.push(JobTargetView {
            job_id,
            client_id: "edge-a".to_string(),
            status: "running".to_string(),
            message: None,
            exit_code: None,
            started_at: Some(now.clone()),
            deadline_at: None,
            completed_at: None,
            process_incarnation_id: None,
        });
        memory.backup_requests.write().await.extend([
            dashboard_test_backup("edge-a", &now, BackupRequestStatus::RequestedMetadataOnly),
            dashboard_test_backup(
                "edge-b",
                &now,
                BackupRequestStatus::ArtifactMetadataRecorded,
            ),
        ]);
    }

    let state = dashboard_test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let Json(view) = routes_dashboard::dashboard_overview(
        State(state),
        headers,
        Query(routes_dashboard::DashboardOverviewQuery {
            window: Some("7d".to_string()),
            ..dashboard_query_default()
        }),
    )
    .await
    .unwrap();

    assert_eq!(view.window, "7d");
    assert_eq!(view.group_by, "labels");
    assert_eq!(view.scope.kind, "all");
    assert_eq!(view.summary.total, 2);
    assert_eq!(view.summary.online, 1);
    assert_eq!(view.summary.stale, 1);
    assert_eq!(view.operations.stale_agents, 1);
    assert_eq!(view.operations.running_jobs, 1);
    assert_eq!(view.operations.backup_pending, 1);
    assert_eq!(view.operations.backup_completed, 1);
    assert_eq!(view.resources.sampled_clients, 2);
    assert!(view.resources.cpu_load_avg.unwrap() > 0.7);
    assert_eq!(view.resource_curve.metric, "cpu_load");
    assert_eq!(view.resource_curve.sampled_clients, 2);
    assert_eq!(
        view.resource_curve.latest_sample_at.as_deref(),
        Some(now.as_str())
    );
    assert!(view
        .resource_curve
        .series
        .iter()
        .any(|series| series.client_id == "edge-b" && series.critical_threshold.is_some()));
    assert_eq!(view.network.top_clients.len(), 2);
    assert!(view.network.rx_bps < 1_000.0);
    assert!(view.network.tx_bps < 1_000.0);
    assert!(!view.network.traffic_points.is_empty());
    assert_eq!(view.network.traffic_top_clients.len(), 2);
    assert_eq!(view.network.traffic_series.len(), 2);
    assert!(view
        .network
        .traffic_top_clients
        .iter()
        .any(|client| client.client_id == "edge-b" && client.tx_bytes == 9_000));
    assert!(view
        .network
        .traffic_series
        .iter()
        .any(|client| client.client_id == "edge-b"
            && client.points.iter().any(|point| point.tx_bytes == 9_000)));
    assert!(view
        .operations
        .recent_alerts
        .iter()
        .any(|alert| alert.client_label.as_deref() == Some("Edge B")));
    assert!(view
        .label_clusters
        .iter()
        .any(|cluster| cluster.label == "provider:alpha" && cluster.total == 1));
    assert!(view
        .label_clusters
        .iter()
        .any(|cluster| cluster.label == "country:US" && cluster.total == 2));
    assert_eq!(view.label_clusters.last().unwrap().label, "All VPS");
}

#[tokio::test]
async fn dashboard_overview_supports_scope_and_group_by() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.extend([
            AgentView {
                id: "edge-a".to_string(),
                display_name: "Edge A".to_string(),
                status: "online".to_string(),
                tags: vec![
                    "provider:alpha".to_string(),
                    "country:US".to_string(),
                    "edge".to_string(),
                ],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: AgentCapabilitySnapshot::default(),
            },
            AgentView {
                id: "edge-b".to_string(),
                display_name: "Edge B".to_string(),
                status: "online".to_string(),
                tags: vec![
                    "provider:alpha".to_string(),
                    "country:DE".to_string(),
                    "edge".to_string(),
                ],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: AgentCapabilitySnapshot::default(),
            },
            AgentView {
                id: "core-c".to_string(),
                display_name: "Core C".to_string(),
                status: "online".to_string(),
                tags: vec!["provider:beta".to_string(), "country:US".to_string()],
                registration_ip: None,
                last_ip: None,
                last_seen_at: None,
                arch: None,
                internal_build_number: 1,
                process_incarnation_id: None,
                stale_since: None,
                stale_reason: None,
                capabilities: AgentCapabilitySnapshot::default(),
            },
        ]);
    }

    let state = dashboard_test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let Json(view) = routes_dashboard::dashboard_overview(
        State(state),
        headers,
        Query(routes_dashboard::DashboardOverviewQuery {
            group_by: Some("countries".to_string()),
            scope_kind: Some("provider".to_string()),
            scope_value: Some("alpha".to_string()),
            window: Some("24h".to_string()),
            ..dashboard_query_default()
        }),
    )
    .await
    .unwrap();

    assert_eq!(view.group_by, "countries");
    assert_eq!(view.scope.kind, "provider");
    assert_eq!(view.scope.label, "provider:alpha");
    assert_eq!(view.scope.matched_clients, 2);
    assert_eq!(view.summary.total, 2);
    assert!(view
        .label_clusters
        .iter()
        .any(|cluster| cluster.label == "country:US" && cluster.total == 1));
    assert!(view
        .label_clusters
        .iter()
        .any(|cluster| cluster.label == "country:DE" && cluster.total == 1));
    assert!(view
        .available_filters
        .group_by_options
        .iter()
        .any(|option| option.value == "date"));
    assert!(view
        .available_filters
        .windows
        .iter()
        .any(|option| option.value == "all" && option.seconds == 0));
}

#[tokio::test]
async fn dashboard_overview_supports_all_window_with_available_data_start() {
    let repo = Repository::Memory(MemoryState::default());
    let now = unix_now();
    let older = (now.saturating_sub(10 * 24 * 60 * 60)).to_string();
    let newer = (now.saturating_sub(60 * 60)).to_string();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "edge-a".to_string(),
            display_name: "Edge A".to_string(),
            status: "online".to_string(),
            tags: vec!["provider:alpha".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            arch: None,
            internal_build_number: 1,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        });
        memory.telemetry_rollups.write().await.extend([
            dashboard_test_rollup("edge-a", &older, 0.4, 0.8, 600, 1600),
            dashboard_test_rollup("edge-a", &newer, 0.7, 1.2, 500, 1500),
        ]);
        memory.telemetry_network_rates.write().await.extend([
            dashboard_test_rate("edge-a", "eth0", &older, 1000, 2000),
            dashboard_test_rate("edge-a", "eth0", &newer, 3000, 4000),
        ]);
    }

    let state = dashboard_test_state(repo);
    let headers = crate::test_auth_headers(&state).await;
    let Json(view) = routes_dashboard::dashboard_overview(
        State(state),
        headers,
        Query(routes_dashboard::DashboardOverviewQuery {
            window: Some("all".to_string()),
            ..dashboard_query_default()
        }),
    )
    .await
    .unwrap();

    assert_eq!(view.window, "all");
    assert_eq!(view.time_range.mode, "all");
    assert_eq!(view.time_range.window.as_deref(), Some("all"));
    assert!(view.time_range.start_unix >= now.saturating_sub(11 * 24 * 60 * 60));
    assert!(view.resource_curve.series[0].points.len() >= 2);
    assert_eq!(view.network.traffic_series[0].rx_bytes, 2000);
    assert_eq!(view.network.traffic_series[0].tx_bytes, 2000);
}

#[tokio::test]
async fn dashboard_repository_scopes_before_limit_and_keeps_pre_window_network_baseline() {
    let repo = Repository::Memory(MemoryState::default());
    let current = unix_now() / 60 * 60;
    let previous = current.saturating_sub(60);
    if let Repository::Memory(memory) = &repo {
        memory.telemetry_rollups.write().await.extend([
            dashboard_test_rollup("unrelated", &current.to_string(), 9.0, 9.0, 100, 100),
            dashboard_test_rollup("selected", &current.to_string(), 0.5, 0.8, 600, 1600),
        ]);
        memory.telemetry_network_rates.write().await.extend([
            dashboard_test_rate("unrelated", "eth0", &current.to_string(), 99_000, 99_000),
            dashboard_test_rate("selected", "eth0", &previous.to_string(), 1_000, 2_000),
            dashboard_test_rate("selected", "eth0", &current.to_string(), 4_000, 8_000),
        ]);
    }
    let scope = vec!["selected".to_string()];
    let rollups = repo
        .list_dashboard_telemetry_rollups(
            1,
            Some(current),
            Some(current + 59),
            Some(60),
            60,
            &scope,
        )
        .await
        .unwrap();
    assert_eq!(rollups.len(), 1);
    assert_eq!(rollups[0].client_id, "selected");

    let rates = repo
        .list_dashboard_telemetry_network_rates(
            10,
            Some(current),
            Some(current + 59),
            Some(60),
            60,
            &scope,
        )
        .await
        .unwrap();
    assert_eq!(rates.len(), 1);
    assert_eq!(rates[0].client_id, "selected");
    assert_eq!(rates[0].rx_bytes_delta, 3_000);
    assert_eq!(rates[0].tx_bytes_delta, 6_000);
    assert!(rates[0].rx_bps_avg > 0.0);
    assert!(rates[0].tx_bps_avg > 0.0);
}

#[tokio::test]
async fn latest_telemetry_snapshots_return_one_current_row_per_resource_key() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        let mut coarse_rollup = dashboard_test_rollup("edge-a", "250", 2.5, 2.7, 400, 1400);
        coarse_rollup.bucket_secs = 300;
        memory.telemetry_rollups.write().await.extend([
            dashboard_test_rollup("edge-a", "100", 0.5, 0.7, 600, 1600),
            dashboard_test_rollup("edge-a", "200", 1.5, 1.7, 500, 1500),
            dashboard_test_rollup("edge-b", "150", 0.8, 1.0, 550, 1400),
            coarse_rollup,
        ]);
        let mut coarse_previous = dashboard_test_rate("edge-a", "eth0", "150", 2_000, 3_000);
        coarse_previous.bucket_secs = 300;
        let mut coarse_current = dashboard_test_rate("edge-a", "eth0", "250", 12_000, 13_000);
        coarse_current.bucket_secs = 300;
        memory.telemetry_network_rates.write().await.extend([
            dashboard_test_rate("edge-a", "eth0", "100", 1_000, 2_000),
            dashboard_test_rate("edge-a", "eth0", "200", 7_000, 8_000),
            dashboard_test_rate("edge-a", "wg0", "180", 3_000, 4_000),
            coarse_previous,
            coarse_current,
        ]);
    }

    let rollups = repo
        .list_latest_telemetry_rollups(10, None, Some(60))
        .await
        .unwrap();
    assert_eq!(rollups.len(), 2);
    assert_eq!(
        rollups
            .iter()
            .find(|rollup| rollup.client_id == "edge-a")
            .unwrap()
            .latest_observed_at,
        "200"
    );

    let rates = repo
        .list_latest_telemetry_network_rates(10, Some("edge-a"), None, Some(60))
        .await
        .unwrap();
    assert_eq!(rates.len(), 2);
    let eth0 = rates.iter().find(|rate| rate.interface == "eth0").unwrap();
    assert_eq!(eth0.bucket_start, "200");
    assert_eq!(eth0.rx_bytes_delta, 6_000);

    let mixed_rollups = repo
        .list_latest_telemetry_rollups(10, None, None)
        .await
        .unwrap();
    assert_eq!(mixed_rollups.len(), 2);
    let edge_a_rollup = mixed_rollups
        .iter()
        .find(|rollup| rollup.client_id == "edge-a")
        .unwrap();
    assert_eq!(edge_a_rollup.bucket_start, "250");
    assert_eq!(edge_a_rollup.bucket_secs, 300);

    let mixed_rates = repo
        .list_latest_telemetry_network_rates(10, Some("edge-a"), None, None)
        .await
        .unwrap();
    assert_eq!(mixed_rates.len(), 2);
    let mixed_eth0 = mixed_rates
        .iter()
        .find(|rate| rate.interface == "eth0")
        .unwrap();
    assert_eq!(mixed_eth0.bucket_start, "250");
    assert_eq!(mixed_eth0.bucket_secs, 300);
    assert_eq!(mixed_eth0.rx_bytes_delta, 10_000);
}

fn dashboard_query_default() -> routes_dashboard::DashboardOverviewQuery {
    routes_dashboard::DashboardOverviewQuery {
        end_at: None,
        end_unix: None,
        chart_points: None,
        group_by: None,
        scope_kind: None,
        scope_value: None,
        start_at: None,
        start_unix: None,
        resource_metric: None,
        window: None,
    }
}

fn dashboard_test_state(repo: Repository) -> AppState {
    AppState {
        repo,
        events: tokio::sync::broadcast::channel(4).0,
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

fn dashboard_test_rollup(
    client_id: &str,
    observed_at: &str,
    cpu_load_1_avg: f64,
    cpu_load_1_max: f64,
    memory_available: i64,
    disk_available: i64,
) -> TelemetryRollupView {
    TelemetryRollupView {
        client_id: client_id.to_string(),
        bucket_start: observed_at.to_string(),
        bucket_secs: 60,
        sample_count: 3,
        cpu_load_1_avg,
        cpu_load_1_max,
        memory_total_bytes_max: 1000,
        memory_available_bytes_avg: memory_available,
        memory_available_bytes_min: memory_available,
        disk_total_bytes_max: 2000,
        disk_available_bytes_avg: disk_available,
        disk_available_bytes_min: disk_available,
        network_rx_bytes_max: 0,
        network_tx_bytes_max: 0,
        latest_observed_at: observed_at.to_string(),
        updated_at: observed_at.to_string(),
    }
}

fn dashboard_test_rate(
    client_id: &str,
    interface: &str,
    observed_at: &str,
    rx_bytes_avg: i64,
    tx_bytes_avg: i64,
) -> TelemetryNetworkRateView {
    TelemetryNetworkRateView {
        client_id: client_id.to_string(),
        interface: interface.to_string(),
        bucket_start: observed_at.to_string(),
        bucket_secs: 60,
        sample_count: 3,
        rx_bytes_avg,
        tx_bytes_avg,
        rx_bytes_delta: 0,
        tx_bytes_delta: 0,
        rx_bps_avg: 0.0,
        tx_bps_avg: 0.0,
        updated_at: observed_at.to_string(),
    }
}

fn dashboard_test_backup(
    client_id: &str,
    created_at: &str,
    status: BackupRequestStatus,
) -> BackupRequestView {
    BackupRequestView {
        id: Uuid::new_v4(),
        actor_id: None,
        client_id: client_id.to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
        status: status.as_str().to_string(),
        payload_hash: "bb".repeat(32),
        command_scope: format!("client:{client_id}"),
        artifact_id: None,
        source_job_id: None,
        source_schedule_id: None,
        note: None,
        created_at: created_at.to_string(),
    }
}
