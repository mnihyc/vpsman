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
fn merged_span_fragments_only_logical_minutes_inside_the_range() {
    let mut row = rollup("a", 120);
    row.bucket_secs = 300;
    row.sample_count = 5;

    let fragments = fragment_telemetry_rollup(row, Some(300), Some(360), 60);

    assert_eq!(
        fragments
            .iter()
            .map(|row| (row.bucket_start.as_str(), row.sample_count))
            .collect::<Vec<_>>(),
        vec![("300", 1), ("360", 1)]
    );
}

#[test]
fn adaptive_resource_fragmentation_matches_uncompacted_minutes() {
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

    let uncompacted = (0..5)
        .map(|minute| {
            let mut row = rollup("a", 120 + minute * 60);
            row.cpu_load_1_avg = 0.5;
            row.cpu_load_1_max = 0.5;
            row.latest_observed_at = (120 + minute * 60 + 17).to_string();
            row.connections_sample_count = 1;
            row.tcp_sockets_latest = Some(12);
            row.udp_sockets_latest = Some(4);
            row.connections_observed_at = Some((120 + minute * 60 + 31).to_string());
            row
        })
        .flat_map(|row| fragment_telemetry_rollup(row, Some(180), Some(360), 120))
        .collect::<Vec<_>>();
    let compacted = fragment_telemetry_rollup(compact, Some(180), Some(360), 120);

    let left = aggregate_memory_telemetry_rollups(uncompacted, 120);
    let right = aggregate_memory_telemetry_rollups(compacted, 120);
    assert_eq!(rollup_counts(&left), rollup_counts(&right));
    let timestamps = |rows: &[TelemetryRollupView]| {
        rows.iter()
            .map(|row| {
                (
                    row.bucket_start.clone(),
                    row.latest_observed_at.clone(),
                    row.connections_observed_at.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(timestamps(&left), timestamps(&right));
    assert_eq!(
        rollup_counts(&right),
        vec![("120", 1), ("240", 2), ("360", 1)]
    );
    assert_eq!(
        timestamps(&right),
        vec![
            (
                "120".to_string(),
                "197".to_string(),
                Some("211".to_string())
            ),
            (
                "240".to_string(),
                "317".to_string(),
                Some("331".to_string())
            ),
            (
                "360".to_string(),
                "377".to_string(),
                Some("391".to_string())
            ),
        ]
    );
}

#[test]
fn adaptive_network_fragmentation_matches_uncompacted_minutes() {
    let mut uncompacted = vec![network_rate_with_counter(
        "a", "eth0", 120, 60, 1_000, 2_000,
    )];
    uncompacted.extend(
        (0..5).map(|minute| {
            network_rate_with_counter("a", "eth0", 180 + minute * 60, 60, 1_600, 2_900)
        }),
    );
    let mut compacted = vec![network_rate_with_counter(
        "a", "eth0", 120, 60, 1_000, 2_000,
    )];
    let mut wide = network_rate_with_counter("a", "eth0", 180, 300, 1_600, 2_900);
    wide.sample_count = 5;
    compacted.push(wide);

    for step_secs in [60, 300] {
        let left =
            dashboard_network_rows_for_test(uncompacted.clone(), Some(180), Some(420), step_secs);
        let right =
            dashboard_network_rows_for_test(compacted.clone(), Some(180), Some(420), step_secs);
        assert_eq!(network_rate_summary(&left), network_rate_summary(&right));
    }
    let minute_rows = dashboard_network_rows_for_test(compacted, Some(180), Some(420), 60);
    assert!((minute_rows[0].rx_bps_avg - 80.0).abs() < f64::EPSILON);
    assert!(minute_rows[1..]
        .iter()
        .all(|row| row.rx_bps_avg.abs() < f64::EPSILON));
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
            Some(60),
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
            Some(60),
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
            Some(120),
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
        disk_total_bytes_max: 0,
        disk_available_bytes_avg: 0,
        disk_available_bytes_min: 0,
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

fn rollup_counts(rows: &[TelemetryRollupView]) -> Vec<(&str, i32)> {
    rows.iter()
        .map(|row| (row.bucket_start.as_str(), row.sample_count))
        .collect()
}

fn dashboard_network_rows_for_test(
    rows: Vec<TelemetryNetworkRateView>,
    start_unix: Option<u64>,
    end_unix: Option<u64>,
    step_secs: i32,
) -> Vec<TelemetryNetworkRateView> {
    let mut rows = derive_network_rates(select_dashboard_network_rows(
        rows, start_unix, end_unix, step_secs,
    ));
    rows.retain(|row| row.sample_count > 0);
    rows
}

fn network_rate_summary(rows: &[TelemetryNetworkRateView]) -> Vec<(&str, i32, i64, i64, u64, u64)> {
    rows.iter()
        .map(|row| {
            (
                row.bucket_start.as_str(),
                row.sample_count,
                row.rx_bytes_delta,
                row.tx_bytes_delta,
                row.rx_bps_avg.to_bits(),
                row.tx_bps_avg.to_bits(),
            )
        })
        .collect()
}

fn network_keys(rows: &[TelemetryNetworkRateView]) -> Vec<String> {
    rows.iter()
        .map(|row| format!("{}/{}:{}", row.client_id, row.interface, row.bucket_start))
        .collect()
}
