use super::*;

#[tokio::test]
async fn raw_source_is_used_only_when_it_covers_retained_minute_history() {
    let repo = Repository::Memory(crate::repository::MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    memory
        .telemetry_rollups
        .write()
        .await
        .extend([rollup("v-1", 100), rollup("v-2", 300)]);
    memory.telemetry_samples.write().await.extend([
        raw_sample("v-1", 200),
        raw_sample("v-2", 300),
        raw_sample("v-3", 200),
    ]);
    memory.traffic_counter_samples.write().await.push(
        crate::model_alert_policies::TrafficCounterSampleRecord {
            client_id: "v-3".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            observed_at: "100".to_string(),
            observed_unix: 100,
            rx_bytes: 0,
            tx_bytes: 0,
            rx_counter_epoch: 0,
            tx_counter_epoch: 0,
            sample_source: "test".to_string(),
        },
    );

    assert!(!repo
        .raw_telemetry_covers_range_start(&["v-1".to_string()], 150)
        .await
        .unwrap());
    assert!(repo
        .raw_telemetry_covers_range_start(&["v-1".to_string()], 200)
        .await
        .unwrap());
    assert!(!repo
        .raw_telemetry_covers_range_start(&["v-3".to_string()], 150)
        .await
        .unwrap());
    assert!(repo
        .raw_telemetry_covers_range_start(&["v-3".to_string()], 200)
        .await
        .unwrap());
    assert!(repo
        .raw_telemetry_covers_range_start(&["v-2".to_string()], 100)
        .await
        .unwrap());
    assert!(!repo
        .raw_telemetry_covers_range_start(&["v-1".to_string(), "v-2".to_string()], 150,)
        .await
        .unwrap());
}

#[test]
fn coarse_resource_overlap_returns_one_whole_physical_bucket() {
    let mut row = rollup("a", 120);
    row.bucket_secs = 300;
    row.sample_count = 5;

    let selected = fragment_telemetry_rollup(row, Some(419), Some(420), 60);

    assert_eq!(
        selected
            .iter()
            .map(|row| (row.bucket_start.as_str(), row.bucket_secs, row.sample_count))
            .collect::<Vec<_>>(),
        vec![("0", 300, 5)]
    );
}

#[test]
fn conflicting_resource_tiers_use_the_coarsest_whole_bucket_once() {
    let mut coarse = rollup("a", 0);
    coarse.bucket_secs = 300;
    coarse.sample_count = 5;
    let fine = rollup("a", 240);

    let selected = retain_authoritative_telemetry_rows(vec![coarse, fine]);

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].bucket_secs, 300);
    assert_eq!(selected[0].sample_count, 5);
}

#[test]
fn coarse_resource_selection_preserves_authoritative_values_and_timestamps() {
    let mut compact = rollup("a", 120);
    compact.bucket_secs = 300;
    compact.sample_count = 5;
    compact.cpu_load_1_avg = 0.5;
    compact.cpu_load_1_max = 0.5;
    compact.latest_observed_at = "377".to_string();
    compact.connections_sample_count = 5;
    compact.tcp_sockets_latest = Some(12);
    compact.udp_sockets_latest = Some(4);
    compact.connections_observed_at = Some("391".to_string());
    compact.swap_sample_count = 5;
    compact.swap_total_bytes_max = Some(1_000);
    compact.swap_available_bytes_avg = Some(400);
    compact.swap_available_bytes_min = Some(400);
    compact.swap_used_ratio_avg = Some(0.6);
    compact.swap_used_ratio_max = Some(0.6);

    let selected = fragment_telemetry_rollup(compact, Some(419), Some(420), 120);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].sample_count, 5);
    assert_eq!(selected[0].bucket_secs, 300);
    assert_eq!(selected[0].latest_observed_at, "377");
    assert_eq!(selected[0].connections_observed_at.as_deref(), Some("391"));
    assert_eq!(selected[0].swap_sample_count, 5);
}

#[test]
fn coarse_resource_selection_preserves_explicit_no_swap() {
    let mut compact = rollup("a", 120);
    compact.bucket_secs = 300;
    compact.sample_count = 5;
    compact.swap_sample_count = 0;
    compact.swap_total_bytes_max = Some(0);
    compact.swap_available_bytes_avg = Some(0);
    compact.swap_available_bytes_min = Some(0);
    compact.swap_used_ratio_avg = None;
    compact.swap_used_ratio_max = None;

    let selected = fragment_telemetry_rollup(compact, Some(419), Some(420), 120);

    assert_eq!(selected.len(), 1);
    assert!(selected.iter().all(|row| {
        row.swap_sample_count == 0
            && row.swap_total_bytes_max == Some(0)
            && row.swap_available_bytes_avg == Some(0)
            && row.swap_available_bytes_min == Some(0)
            && row.swap_used_ratio_avg.is_none()
            && row.swap_used_ratio_max.is_none()
    }));
}

#[test]
fn coarse_network_overlap_returns_one_whole_physical_bucket() {
    let mut compacted = vec![network_rate_with_counter(
        "a", "eth0", 120, 60, 1_000, 2_000,
    )];
    let mut wide = network_rate_with_counter("a", "eth0", 180, 300, 1_600, 2_900);
    wide.sample_count = 5;
    wide.latest_observed_at = "420".to_string();
    compacted.push(wide);
    compacted.push(network_rate_with_counter(
        "a", "eth0", 240, 60, 1_500, 2_700,
    ));

    let rows = dashboard_network_rows_for_test(compacted, Some(419), Some(421), 60);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].bucket_secs, 300);
    assert_eq!(rows[0].sample_count, 5);
    assert_eq!(rows[0].rx_bytes_delta, 600);
}

#[tokio::test]
async fn aggregate_rate_selection_filters_interfaces_and_keeps_both_directions() {
    let repo = Repository::Memory(crate::repository::MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    memory.telemetry_network_rates.write().await.extend([
        network_rate_with_counter("v-1", "eth0", 0, 60, 1_000, 2_000),
        network_rate_with_counter("v-1", "eth0", 60, 60, 1_060, 2_120),
        network_rate_with_counter("v-1", "lo", 0, 60, 3_000, 4_000),
        network_rate_with_counter("v-1", "lo", 60, 60, 3_600, 4_700),
    ]);

    let mut selection = NetworkRateInterfaceSelection::default();
    selection.select_exact(
        "v-1".to_string(),
        std::collections::BTreeSet::from(["eth0".to_string()]),
    );

    let selected = repo
        .list_dashboard_telemetry_network_rates_selected(
            10,
            Some(60),
            Some(61),
            Some(60),
            60,
            &selection,
        )
        .await
        .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].interface, "eth0");
    assert_eq!(selected[0].rx_bytes_delta, 60);
    assert!(selected[0].rx_bps_avg > 0.0);
    assert_eq!(selected[0].tx_bytes_delta, 120);
    assert!(selected[0].tx_bps_avg > 0.0);

    let raw = repo
        .list_telemetry_network_rates(10, Some("v-1"), None, Some(60), false)
        .await
        .unwrap();
    assert_eq!(
        raw.iter()
            .map(|rate| rate.interface.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["eth0", "lo"])
    );
}

#[tokio::test]
async fn network_counter_resets_are_gaps_and_advance_the_memory_baseline() {
    let repo = Repository::Memory(crate::repository::MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    memory.telemetry_network_rates.write().await.extend([
        network_rate_with_counter("v-1", "eth0", 0, 60, 1_000, 2_000),
        network_rate_with_counter("v-1", "eth0", 60, 60, 100, 2_100),
    ]);
    memory.telemetry_samples.write().await.extend([
        raw_network_sample("v-1", 0, 1_000, 2_000),
        raw_network_sample("v-1", 60, 100, 2_100),
    ]);

    assert!(repo
        .list_dashboard_telemetry_network_rates(
            10,
            Some(60),
            Some(61),
            Some(60),
            60,
            &["v-1".to_string()],
        )
        .await
        .unwrap()
        .is_empty());
    assert!(repo
        .list_dashboard_raw_telemetry_network_rates(10, 60, 60, 60, &["v-1".to_string()],)
        .await
        .unwrap()
        .is_empty());
    let listed = repo
        .list_telemetry_network_rates(10, Some("v-1"), Some("eth0"), Some(60), false)
        .await
        .unwrap();
    assert!(listed.is_empty());
    assert!(repo
        .list_latest_telemetry_network_rates(10, Some("v-1"), Some("eth0"), Some(60))
        .await
        .unwrap()
        .is_empty());

    memory
        .telemetry_network_rates
        .write()
        .await
        .push(network_rate_with_counter(
            "v-1", "eth0", 120, 60, 160, 2_200,
        ));
    memory
        .telemetry_samples
        .write()
        .await
        .push(raw_network_sample("v-1", 120, 160, 2_200));

    for rows in [
        repo.list_dashboard_telemetry_network_rates(
            10,
            Some(120),
            Some(121),
            Some(60),
            60,
            &["v-1".to_string()],
        )
        .await
        .unwrap(),
        repo.list_dashboard_raw_telemetry_network_rates(10, 120, 120, 60, &["v-1".to_string()])
            .await
            .unwrap(),
        repo.list_latest_telemetry_network_rates(10, Some("v-1"), Some("eth0"), Some(60))
            .await
            .unwrap(),
    ] {
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].bucket_start, "120");
        assert_eq!(rows[0].rx_bytes_delta, 60);
        assert_eq!(rows[0].tx_bytes_delta, 100);
        assert!(rows[0].rx_bps_avg > 0.0);
        assert!(rows[0].tx_bps_avg > 0.0);
    }
}

#[tokio::test]
async fn raw_network_reset_inside_one_display_bucket_cannot_be_hidden_by_recovery() {
    let repo = Repository::Memory(crate::repository::MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    memory.telemetry_samples.write().await.extend([
        raw_network_sample("v-1", 585, 1_000, 2_000),
        raw_network_sample("v-1", 605, 100, 2_100),
        raw_network_sample("v-1", 620, 1_200, 2_200),
        raw_network_sample("v-1", 665, 1_300, 2_300),
    ]);

    assert!(repo
        .list_dashboard_raw_telemetry_network_rates(10, 600, 659, 60, &["v-1".to_string()])
        .await
        .unwrap()
        .is_empty());
    let recovered = repo
        .list_dashboard_raw_telemetry_network_rates(10, 660, 719, 60, &["v-1".to_string()])
        .await
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].rx_bytes_delta, 100);
    assert_eq!(recovered[0].tx_bytes_delta, 100);
}

#[test]
fn rollup_budget_is_globally_bounded_and_rank_fair() {
    let mut rows = vec![
        rollup("a", 100),
        rollup("a", 300),
        rollup("b", 200),
        rollup("b", 250),
        rollup("c", 275),
    ];

    retain_fair_rollup_points(&mut rows, 2, 4);

    assert_eq!(rollup_keys(&rows), vec!["b:200", "b:250", "c:275", "a:300"]);
    assert_eq!(rows.len(), 4);
    assert!(["a", "b", "c"]
        .into_iter()
        .all(|client_id| rows.iter().any(|row| row.client_id == client_id)));
}

#[test]
fn socket_rollups_keep_missing_distinct_from_zero_and_choose_latest_known_value() {
    let sample = |observed_at, connections| {
        let metrics = AgentMetrics {
            observed_unix: observed_at,
            hostname: "v-1".to_string(),
            connections,
            ..AgentMetrics::default()
        };
        TelemetrySampleView {
            id: uuid::Uuid::new_v4(),
            client_id: "v-1".to_string(),
            observed_at: observed_at.to_string(),
            cpu_load_1: 0.0,
            memory_total_bytes: 0,
            memory_available_bytes: 0,
            payload: serde_json::to_value(metrics).unwrap(),
        }
    };
    let rows = vec![
        raw_sample_rollup(sample(
            60,
            Some(vpsman_common::ConnectionStat { tcp: 3, udp: 2 }),
        ))
        .unwrap(),
        raw_sample_rollup(sample(120, None)).unwrap(),
        raw_sample_rollup(sample(
            180,
            Some(vpsman_common::ConnectionStat { tcp: 5, udp: 4 }),
        ))
        .unwrap(),
    ];
    let aggregated = aggregate_memory_telemetry_rollups(rows, 300);
    assert_eq!(aggregated.len(), 1);
    assert_eq!(aggregated[0].sample_count, 3);
    assert_eq!(aggregated[0].connections_sample_count, 2);
    assert_eq!(aggregated[0].tcp_sockets_latest, Some(5));
    assert_eq!(aggregated[0].udp_sockets_latest, Some(4));
    assert_eq!(
        aggregated[0].connections_observed_at.as_deref(),
        Some("180")
    );
}

#[test]
fn swap_rollups_keep_unavailable_distinct_from_zero_and_weight_known_samples() {
    let sample = |observed_at, swap: Option<(u64, u64)>| {
        let metrics = AgentMetrics {
            observed_unix: observed_at,
            hostname: "v-1".to_string(),
            memory: vpsman_common::MemoryStat {
                swap_total_bytes: swap.map(|(total, _)| total),
                swap_available_bytes: swap.map(|(_, available)| available),
                ..Default::default()
            },
            ..AgentMetrics::default()
        };
        TelemetrySampleView {
            id: uuid::Uuid::new_v4(),
            client_id: "v-1".to_string(),
            observed_at: observed_at.to_string(),
            cpu_load_1: 0.0,
            memory_total_bytes: 0,
            memory_available_bytes: 0,
            payload: serde_json::to_value(metrics).unwrap(),
        }
    };

    let rows = [
        raw_sample_rollup(sample(60, None)).unwrap(),
        raw_sample_rollup(sample(120, Some((0, 0)))).unwrap(),
        raw_sample_rollup(sample(180, Some((1_000, 400)))).unwrap(),
    ];
    assert_eq!(rows[0].swap_sample_count, 0);
    assert_eq!(rows[0].swap_total_bytes_max, None);
    assert_eq!(rows[1].swap_sample_count, 0);
    assert_eq!(rows[1].swap_total_bytes_max, Some(0));
    assert_eq!(rows[1].swap_available_bytes_avg, Some(0));
    assert_eq!(rows[1].swap_available_bytes_min, Some(0));
    assert_eq!(rows[1].swap_used_ratio_avg, None);
    assert_eq!(rows[1].swap_used_ratio_max, None);

    let zero_only = aggregate_memory_telemetry_rollups(vec![rows[1].clone()], 300);
    assert_eq!(zero_only.len(), 1);
    assert_eq!(zero_only[0].swap_sample_count, 0);
    assert_eq!(zero_only[0].swap_total_bytes_max, Some(0));
    assert_eq!(zero_only[0].swap_available_bytes_avg, Some(0));
    assert_eq!(zero_only[0].swap_available_bytes_min, Some(0));
    assert_eq!(zero_only[0].swap_used_ratio_avg, None);
    assert_eq!(zero_only[0].swap_used_ratio_max, None);

    let mut one_sided = sample(240, None);
    one_sided.payload["memory"]["swap_total_bytes"] = serde_json::json!(1_000);
    assert!(raw_sample_rollup(one_sided)
        .unwrap_err()
        .to_string()
        .contains("swap evidence is one-sided"));

    let invalid_available = sample(300, Some((1_000, 1_001)));
    assert!(raw_sample_rollup(invalid_available)
        .unwrap_err()
        .to_string()
        .contains("swap available exceeds total"));

    let aggregated = aggregate_memory_telemetry_rollups(rows.into(), 300);
    assert_eq!(aggregated.len(), 1);
    assert_eq!(aggregated[0].sample_count, 3);
    assert_eq!(aggregated[0].swap_sample_count, 1);
    assert_eq!(aggregated[0].swap_total_bytes_max, Some(1_000));
    assert_eq!(aggregated[0].swap_available_bytes_avg, Some(400));
    assert_eq!(aggregated[0].swap_available_bytes_min, Some(400));
    assert_eq!(aggregated[0].swap_used_ratio_avg, Some(0.6));
    assert_eq!(aggregated[0].swap_used_ratio_max, Some(0.6));
}

#[test]
fn dynamic_resource_capacities_keep_snapshot_ratios_when_aggregated() {
    let sample = |observed_at: u64, memory: (u64, u64), swap: (u64, u64), disk: (u64, u64)| {
        let metrics = AgentMetrics {
            observed_unix: observed_at,
            hostname: "v-1".to_string(),
            memory: vpsman_common::MemoryStat {
                total_bytes: memory.0,
                available_bytes: memory.1,
                swap_total_bytes: Some(swap.0),
                swap_available_bytes: Some(swap.1),
            },
            disks: vec![vpsman_common::DiskStat {
                mountpoint: "/".to_string(),
                total_bytes: disk.0,
                available_bytes: disk.1,
            }],
            ..AgentMetrics::default()
        };
        TelemetrySampleView {
            id: uuid::Uuid::new_v4(),
            client_id: "v-1".to_string(),
            observed_at: observed_at.to_string(),
            cpu_load_1: 0.0,
            memory_total_bytes: memory.0 as i64,
            memory_available_bytes: memory.1 as i64,
            payload: serde_json::to_value(metrics).unwrap(),
        }
    };

    let first = raw_sample_rollup(sample(60, (100, 50), (100, 20), (100, 75))).unwrap();
    let mut later = raw_sample_rollup(sample(120, (400, 300), (400, 200), (400, 40))).unwrap();

    assert_eq!(first.memory_used_ratio_avg, 0.5);
    assert_eq!(first.swap_used_ratio_avg, Some(0.8));
    assert_eq!(first.disk_used_ratio_avg, 0.25);
    assert_eq!(later.memory_used_ratio_avg, 0.25);
    assert_eq!(later.swap_used_ratio_avg, Some(0.5));
    assert_eq!(later.disk_used_ratio_avg, 0.9);

    // Model three snapshots at the later capacity to exercise weighted rollup
    // aggregation while retaining the ratios frozen at each source snapshot.
    later.sample_count = 3;
    later.swap_sample_count = 3;
    let aggregated = aggregate_memory_telemetry_rollups(vec![first, later], 300);

    assert_eq!(aggregated.len(), 1);
    let row = &aggregated[0];
    assert_eq!(row.sample_count, 4);
    assert_eq!(row.memory_total_bytes_max, 400);
    assert_eq!(row.memory_available_bytes_avg, 238);
    assert_eq!(row.memory_available_bytes_min, 50);
    assert!((row.memory_used_ratio_avg - 0.3125).abs() < f64::EPSILON);
    assert!((row.memory_used_ratio_max - 0.5).abs() < f64::EPSILON);

    assert_eq!(row.swap_sample_count, 4);
    assert_eq!(row.swap_total_bytes_max, Some(400));
    assert_eq!(row.swap_available_bytes_avg, Some(155));
    assert_eq!(row.swap_available_bytes_min, Some(20));
    assert!((row.swap_used_ratio_avg.unwrap() - 0.575).abs() < f64::EPSILON);
    assert!((row.swap_used_ratio_max.unwrap() - 0.8).abs() < f64::EPSILON);

    assert_eq!(row.disk_total_bytes_max, 400);
    assert_eq!(row.disk_available_bytes_avg, 49);
    assert_eq!(row.disk_available_bytes_min, 40);
    assert!((row.disk_used_ratio_avg - 0.7375).abs() < f64::EPSILON);
    assert!((row.disk_used_ratio_max - 0.9).abs() < f64::EPSILON);
}

#[test]
fn network_budget_is_globally_bounded_and_stably_rank_fair() {
    let mut rows = vec![
        network_rate("a", "eth0", 200),
        network_rate("a", "eth0", 300),
        network_rate("a", "eth1", 200),
        network_rate("a", "eth1", 300),
        network_rate("b", "wg0", 275),
    ];

    retain_fair_network_points(&mut rows, 2, 4);

    assert_eq!(
        network_keys(&rows),
        vec!["a/eth0:200", "b/wg0:275", "a/eth0:300", "a/eth1:300"]
    );
    assert_eq!(rows.len(), 4);
    assert!(["a/eth0", "a/eth1", "b/wg0"].into_iter().all(|series| {
        rows.iter()
            .any(|row| format!("{}/{}", row.client_id, row.interface) == series)
    }));
}

fn rollup(client_id: &str, bucket_start: u64) -> TelemetryRollupView {
    let bucket_start = bucket_start.to_string();
    TelemetryRollupView {
        client_id: client_id.to_string(),
        bucket_start: bucket_start.clone(),
        bucket_secs: 60,
        sample_count: 1,
        cpu_usage_sample_count: 0,
        cpu_usage_avg: None,
        cpu_usage_max: None,
        cpu_cores_max: 0,
        cpu_load_1_avg: 0.0,
        cpu_load_1_max: 0.0,
        cpu_load_5_avg: 0.0,
        cpu_load_5_max: 0.0,
        cpu_load_15_avg: 0.0,
        cpu_load_15_max: 0.0,
        memory_total_bytes_max: 0,
        memory_available_bytes_avg: 0,
        memory_available_bytes_min: 0,
        memory_used_ratio_avg: 0.0,
        memory_used_ratio_max: 0.0,
        swap_sample_count: 0,
        swap_total_bytes_max: None,
        swap_available_bytes_avg: None,
        swap_available_bytes_min: None,
        swap_used_ratio_avg: None,
        swap_used_ratio_max: None,
        disk_total_bytes_max: 0,
        disk_available_bytes_avg: 0,
        disk_available_bytes_min: 0,
        disk_used_ratio_avg: 0.0,
        disk_used_ratio_max: 0.0,
        network_rx_bytes_max: 0,
        network_tx_bytes_max: 0,
        connections_sample_count: 0,
        tcp_sockets_latest: None,
        udp_sockets_latest: None,
        connections_observed_at: None,
        latest_observed_at: bucket_start.clone(),
        updated_at: bucket_start,
    }
}

fn raw_sample(client_id: &str, observed_at: u64) -> TelemetrySampleView {
    TelemetrySampleView {
        id: uuid::Uuid::new_v4(),
        client_id: client_id.to_string(),
        observed_at: observed_at.to_string(),
        cpu_load_1: 0.0,
        memory_total_bytes: 0,
        memory_available_bytes: 0,
        payload: serde_json::json!({}),
    }
}

fn raw_network_sample(
    client_id: &str,
    observed_at: u64,
    rx_bytes: u64,
    tx_bytes: u64,
) -> TelemetrySampleView {
    let mut sample = raw_sample(client_id, observed_at);
    sample.payload = serde_json::to_value(AgentMetrics {
        observed_unix: observed_at,
        hostname: client_id.to_string(),
        networks: vec![vpsman_common::NetworkStat {
            interface: "eth0".to_string(),
            rx_bytes,
            tx_bytes,
        }],
        ..AgentMetrics::default()
    })
    .unwrap();
    sample
}

fn network_rate(client_id: &str, interface: &str, bucket_start: u64) -> TelemetryNetworkRateView {
    network_rate_with_counter(client_id, interface, bucket_start, 60, 0, 0)
}

fn network_rate_with_counter(
    client_id: &str,
    interface: &str,
    bucket_start: u64,
    bucket_secs: i32,
    rx_bytes_avg: i64,
    tx_bytes_avg: i64,
) -> TelemetryNetworkRateView {
    TelemetryNetworkRateView {
        client_id: client_id.to_string(),
        interface: interface.to_string(),
        bucket_start: bucket_start.to_string(),
        bucket_secs,
        sample_count: 1,
        rx_bytes_avg,
        tx_bytes_avg,
        rx_bytes_last: rx_bytes_avg,
        tx_bytes_last: tx_bytes_avg,
        rx_counter_epoch: 0,
        tx_counter_epoch: 0,
        latest_observed_at: bucket_start.to_string(),
        rx_bytes_delta: 0,
        tx_bytes_delta: 0,
        rx_bps_avg: 0.0,
        tx_bps_avg: 0.0,
        updated_at: bucket_start.to_string(),
    }
}

fn rollup_keys(rows: &[TelemetryRollupView]) -> Vec<String> {
    rows.iter()
        .map(|row| format!("{}:{}", row.client_id, row.bucket_start))
        .collect()
}

fn dashboard_network_rows_for_test(
    rows: Vec<TelemetryNetworkRateView>,
    start_unix: Option<u64>,
    end_unix: Option<u64>,
    step_secs: i32,
) -> Vec<TelemetryNetworkRateView> {
    let rows = retain_authoritative_network_rows(rows);
    let mut rows = derive_network_rates(select_dashboard_network_rows(
        rows, start_unix, end_unix, step_secs,
    ));
    rows.retain(|row| row.sample_count > 0);
    rows
}

fn network_keys(rows: &[TelemetryNetworkRateView]) -> Vec<String> {
    rows.iter()
        .map(|row| format!("{}/{}:{}", row.client_id, row.interface, row.bucket_start))
        .collect()
}
