use super::*;

#[test]
fn scoped_all_start_distinguishes_ready_empty_from_initializing() {
    assert_eq!(
        dashboard_telemetry_start_or_initializing(DashboardTelemetryStart {
            start_unix: None,
            complete: true,
        })
        .unwrap(),
        None
    );

    let error = dashboard_telemetry_start_or_initializing(DashboardTelemetryStart {
        start_unix: Some(1),
        complete: false,
    })
    .unwrap_err();
    assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error.code, "dashboard_projection_initializing");
}

#[test]
fn chart_step_covers_inclusive_multi_day_endpoints() {
    let range = DashboardRange {
        mode: "all",
        window: None,
        start_unix: 0,
        end_unix: 4 * 24 * 60 * 60,
    };

    assert_eq!(
        dashboard_chart_step_secs(&range, 2),
        range.end_unix - range.start_unix
    );
}

#[test]
fn overview_singleflight_range_key_normalizes_relative_time() {
    let query = dashboard_query(Some("1d"), None, None);
    let first = prepare_dashboard_overview(&query, 2_000_000_000).unwrap();
    let second = prepare_dashboard_overview(&query, 2_000_000_001).unwrap();

    assert_eq!(
        dashboard_overview_range_singleflight_key(&query, &first),
        dashboard_overview_range_singleflight_key(&query, &second)
    );
}

#[test]
fn overview_singleflight_range_key_separates_custom_bounds() {
    let first_query = dashboard_query(None, Some(1_900_000_000), Some(1_900_000_100));
    let second_query = dashboard_query(None, Some(1_900_000_001), Some(1_900_000_100));
    let first = prepare_dashboard_overview(&first_query, 2_000_000_000).unwrap();
    let second = prepare_dashboard_overview(&second_query, 2_000_000_001).unwrap();

    assert_ne!(
        dashboard_overview_range_singleflight_key(&first_query, &first),
        dashboard_overview_range_singleflight_key(&second_query, &second)
    );
}

#[test]
fn overview_singleflight_range_key_separates_clamped_explicit_ends() {
    let now = 2_000_000_000;
    let near = dashboard_query(None, Some(now - 100), Some(now + 5));
    let far = dashboard_query(None, Some(now - 100), Some(now + 100));
    let near_prepared = prepare_dashboard_overview(&near, now).unwrap();
    let far_prepared = prepare_dashboard_overview(&far, now).unwrap();

    assert_eq!(near_prepared.range.end_unix, now);
    assert_eq!(far_prepared.range.end_unix, now);
    assert_ne!(
        dashboard_overview_range_singleflight_key(&near, &near_prepared),
        dashboard_overview_range_singleflight_key(&far, &far_prepared),
    );
}

#[test]
fn overview_range_validation_rejects_unknown_windows_and_malformed_text_bounds() {
    let mut query = dashboard_query(Some("6h"), None, None);
    let error = prepare_dashboard_overview(&query, 2_000_000_000)
        .err()
        .expect("unknown dashboard window must fail");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "invalid_dashboard_window");

    query = dashboard_query(None, None, None);
    query.start_at = Some("not-a-timestamp".to_string());
    let error = prepare_dashboard_overview(&query, 2_000_000_000)
        .err()
        .expect("invalid dashboard start must fail");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "invalid_dashboard_start");

    query.start_at = None;
    query.end_at = Some("not-a-timestamp".to_string());
    let error = prepare_dashboard_overview(&query, 2_000_000_000)
        .err()
        .expect("invalid dashboard end must fail");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "invalid_dashboard_end");
}

#[test]
fn overview_custom_range_keeps_the_public_one_year_bound() {
    let now = 2_000_000_000;
    let year = 365 * 24 * 60 * 60;
    assert!(
        prepare_dashboard_overview(&dashboard_query(None, Some(now - year), Some(now)), now,)
            .is_ok()
    );

    let error =
        prepare_dashboard_overview(&dashboard_query(None, Some(now - year - 1), Some(now)), now)
            .err()
            .expect("dashboard custom range above one year must fail");
    assert_eq!(error.code, "dashboard_time_range_too_large");
}

#[test]
fn overview_numeric_bounds_take_precedence_over_text_aliases() {
    let now = 2_000_000_000;
    let mut query = dashboard_query(None, Some(now - 60), Some(now));
    query.start_at = Some("not-a-timestamp".to_string());
    query.end_at = Some("also-not-a-timestamp".to_string());

    assert!(prepare_dashboard_overview(&query, now).is_ok());
}

#[test]
fn every_overview_range_uses_the_same_resident_history_owner() {
    let source = include_str!("routes_dashboard.rs");
    let (_, resource_loader) = source
        .split_once("async fn load_dashboard_rollups")
        .expect("resource loader");
    let (resource_loader, network_and_rest) = resource_loader
        .split_once("async fn load_dashboard_network_rates")
        .expect("network loader");
    let (network_loader, traffic_and_rest) = network_and_rest
        .split_once("async fn load_dashboard_traffic")
        .expect("traffic loader");
    let (traffic_loader, _) = traffic_and_rest
        .split_once("pub(crate) fn dashboard_projection_initializing")
        .expect("traffic loader end");

    assert!(resource_loader.contains("dashboard_telemetry"));
    assert!(resource_loader.contains("resource_projection("));
    assert!(network_loader.contains("dashboard_telemetry"));
    assert!(network_loader.contains("network_projection("));
    assert!(traffic_loader.contains("dashboard_telemetry"));
    assert!(traffic_loader.contains("traffic_projection("));
    for loader in [resource_loader, network_loader, traffic_loader] {
        for forbidden in [
            "DashboardOverviewReadPlan",
            "RecentRaw",
            "ExplicitBoundedRetained",
            "Tile",
            "all_grid",
            "fallback",
            "DASHBOARD_SPARSE",
            "telemetry_dashboard_resource_blocks",
            "telemetry_dashboard_network_blocks",
            "telemetry_dashboard_traffic_blocks",
        ] {
            assert!(!loader.contains(forbidden), "loader contains {forbidden}");
        }
    }
}

#[test]
fn current_network_latest_excludes_interfaces_absent_from_newest_sample() {
    let rate = |interface: &str, bucket_start: &str, rx_bps_avg: f64| TelemetryNetworkRateView {
        client_id: "client-a".to_string(),
        interface: interface.to_string(),
        bucket_start: bucket_start.to_string(),
        bucket_secs: 60,
        sample_count: 1,
        rx_bytes_avg: 1,
        tx_bytes_avg: 1,
        latest_observed_at: bucket_start.to_string(),
        rx_bytes_delta: 1,
        tx_bytes_delta: 1,
        rx_bps_avg,
        tx_bps_avg: rx_bps_avg,
        updated_at: bucket_start.to_string(),
    };
    let rows = vec![
        rate("eth0", "2026-01-01T00:01:00Z", 20.0),
        rate("eth1", "2026-01-01T00:00:00Z", 10.0),
    ];

    let latest = coherent_latest_rates(latest_rates_by_client_interface(&rows));
    assert_eq!(latest.len(), 1);
    let aggregate = network_by_client(latest.values(), &HashMap::new());
    assert_eq!(aggregate["client-a"].rx_bps, 20.0);
    assert_eq!(aggregate["client-a"].interfaces.len(), 1);
    assert!(aggregate["client-a"].interfaces.contains("eth0"));
}

#[test]
fn compact_network_projection_preserves_presented_fleet_and_top_client() {
    let rate = |client_id: &str,
                bucket_start: &str,
                sample_count: i32,
                rx_bytes_delta: i64,
                tx_bytes_delta: i64,
                rx_bps_avg: f64,
                tx_bps_avg: f64| TelemetryNetworkRateView {
        client_id: client_id.to_string(),
        interface: "eth0".to_string(),
        bucket_start: bucket_start.to_string(),
        bucket_secs: 60,
        sample_count,
        rx_bytes_avg: 0,
        tx_bytes_avg: 0,
        latest_observed_at: bucket_start.to_string(),
        rx_bytes_delta,
        tx_bytes_delta,
        rx_bps_avg,
        tx_bps_avg,
        updated_at: bucket_start.to_string(),
    };
    let full = vec![
        rate("a", "60", 2, 100, 40, 10.0, 4.0),
        rate("b", "60", 3, 10, 4, 1.0, 0.4),
        rate("a", "120", 2, 80, 20, 8.0, 2.0),
        rate("b", "120", 3, 8, 2, 0.8, 0.2),
    ];
    let fleet = vec![
        rate("__fleet__", "60", 5, 110, 44, 11.0, 4.4),
        rate("__fleet__", "120", 5, 88, 22, 8.8, 2.2),
    ];
    let traffic = DashboardTelemetryTrafficProjection {
        client_points: vec![
            DashboardTelemetryTrafficPoint {
                client_id: "a".to_string(),
                bucket_start: "60".to_string(),
                rx_bytes: Some(100),
                tx_bytes: Some(40),
            },
            DashboardTelemetryTrafficPoint {
                client_id: "a".to_string(),
                bucket_start: "120".to_string(),
                rx_bytes: Some(80),
                tx_bytes: Some(20),
            },
        ],
        fleet_points: vec![
            DashboardTelemetryTrafficPoint {
                client_id: "__fleet__".to_string(),
                bucket_start: "60".to_string(),
                rx_bytes: Some(110),
                tx_bytes: Some(44),
            },
            DashboardTelemetryTrafficPoint {
                client_id: "__fleet__".to_string(),
                bucket_start: "120".to_string(),
                rx_bytes: Some(88),
                tx_bytes: Some(22),
            },
        ],
        interfaces_by_client: HashMap::from([("a".to_string(), vec!["eth0".to_string()])]),
        client_ids_in_rank_order: vec!["a".to_string()],
    };
    let range = DashboardRange {
        mode: "1h",
        window: None,
        start_unix: 0,
        end_unix: 180,
    };
    let expected = build_network(
        &full,
        &traffic,
        &HashMap::new(),
        &HashMap::new(),
        1,
        &range,
        60,
    );
    let actual = build_network(
        &fleet,
        &traffic,
        &HashMap::new(),
        &HashMap::new(),
        1,
        &range,
        60,
    );

    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[test]
fn requested_full_span_network_and_traffic_points_are_not_truncated() {
    let rate = |bucket: u64| TelemetryNetworkRateView {
        client_id: "__fleet__".to_string(),
        interface: "__fleet__".to_string(),
        bucket_start: bucket.to_string(),
        bucket_secs: 60,
        sample_count: 1,
        rx_bytes_avg: 0,
        tx_bytes_avg: 0,
        latest_observed_at: bucket.to_string(),
        rx_bytes_delta: 1,
        tx_bytes_delta: 1,
        rx_bps_avg: 1.0,
        tx_bps_avg: 2.0,
        updated_at: bucket.to_string(),
    };
    let buckets = (0..240).map(|index| index * 60).collect::<Vec<_>>();
    let rates = buckets.iter().copied().map(rate).collect::<Vec<_>>();
    let traffic = DashboardTelemetryTrafficProjection {
        client_points: Vec::new(),
        fleet_points: buckets
            .iter()
            .copied()
            .map(|bucket| DashboardTelemetryTrafficPoint {
                client_id: "__fleet__".to_string(),
                bucket_start: bucket.to_string(),
                rx_bytes: Some(1),
                tx_bytes: Some(2),
            })
            .collect(),
        interfaces_by_client: HashMap::new(),
        client_ids_in_rank_order: Vec::new(),
    };
    let range = DashboardRange {
        mode: "all",
        window: None,
        start_unix: 0,
        end_unix: 239 * 60,
    };

    let network = build_network(
        &rates,
        &traffic,
        &HashMap::new(),
        &HashMap::new(),
        8,
        &range,
        60,
    );

    assert_eq!(network.points.len(), 240);
    assert_eq!(network.traffic_points.len(), 240);
    assert_eq!(
        network.points.first().unwrap().bucket_start,
        unix_to_rfc3339(0)
    );
    assert_eq!(
        network.points.last().unwrap().bucket_start,
        unix_to_rfc3339(239 * 60)
    );
    assert_eq!(
        network.traffic_points.first().unwrap().bucket_start,
        unix_to_rfc3339(0)
    );
    assert_eq!(
        network.traffic_points.last().unwrap().bucket_start,
        unix_to_rfc3339(239 * 60)
    );
}

fn dashboard_query(
    window: Option<&str>,
    start_unix: Option<u64>,
    end_unix: Option<u64>,
) -> DashboardOverviewQuery {
    DashboardOverviewQuery {
        window: window.map(str::to_string),
        start_unix,
        end_unix,
        start_at: None,
        end_at: None,
        scope_kind: None,
        scope_value: None,
        group_by: None,
        resource_metric: None,
        chart_points: None,
    }
}
