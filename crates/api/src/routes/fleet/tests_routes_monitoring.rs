use std::collections::BTreeSet;

use super::{
    aligned_timeline_point_count, public_billing_plan, public_monitoring_cards_singleflight_key,
    public_monitoring_detail_singleflight_key, public_network_metric, public_network_points,
    public_traffic_metric, public_visibility_uses_projected_telemetry, retained_resolution_for_age,
    retained_traffic_resolution_for_age, tier_aligned_step_secs, traffic_uses_exact_source,
    validate_bulk_ping_target_selection, validate_monitoring_share_targets, ClientMonitoringQuery,
    MonitoringCardsHistoryMode,
};
use uuid::Uuid;

use crate::{
    model::{
        AgentView, BillingPlanView, MonitoringShareRecord, MonitoringShareTargetRecord,
        MonitoringShareVisibilityRequest, MonitoringShareVisibilityView, PublicBillingPlanView,
        PublicMonitoringCardView, PublicMonitoringDataView, PublicMonitoringDetailView,
        PublicMonitoringRangeView, PublicMonitoringShareBootstrapView, PublicMonitoringShareView,
        PublicNetworkMetricView, PublicNetworkPointView, PublicPingMetricView, PublicPingPointView,
        PublicPortSpeedView, PublicResourceMetricView, PublicSystemInformationView,
        PublicTrafficHistoryPointView, PublicTrafficMetricView, TelemetryNetworkRateView,
    },
    model_alert_policies::TrafficAccountingRecord,
};

#[test]
fn monitoring_card_history_compaction_is_additive_and_opt_in() {
    assert_eq!(
        MonitoringCardsHistoryMode::default(),
        MonitoringCardsHistoryMode::PerInterface
    );
    assert_eq!(
        serde_json::from_str::<MonitoringCardsHistoryMode>("\"selected_aggregate\"").unwrap(),
        MonitoringCardsHistoryMode::SelectedAggregate
    );
    assert_eq!(
        MonitoringCardsHistoryMode::SelectedAggregate.as_str(),
        "selected_aggregate"
    );
}

#[test]
fn bulk_ping_target_selection_has_an_explicit_input_bound() {
    let bounded = (1..=500).map(Uuid::from_u128).collect::<Vec<_>>();
    validate_bulk_ping_target_selection(&bounded).expect("500 Ping targets are valid");
    let oversized = (1..=501).map(Uuid::from_u128).collect::<Vec<_>>();
    assert_eq!(
        validate_bulk_ping_target_selection(&oversized)
            .unwrap_err()
            .code,
        "ping_target_selection_too_large"
    );
    assert_eq!(
        validate_bulk_ping_target_selection(&[]).unwrap_err().code,
        "ping_target_selection_required"
    );
}

#[test]
fn bulk_ping_target_preview_uses_one_set_based_target_and_selector_read() {
    let source = include_str!("routes_monitoring.rs");
    let (_, bulk) = source
        .split_once("pub(crate) async fn bulk_update_ping_targets")
        .expect("Ping bulk-update route");
    let (bulk, _) = bulk
        .split_once("pub(crate) fn validate_bulk_ping_target_selection")
        .expect("Ping bulk-update route end");
    assert_eq!(bulk.matches("ping_target_records_by_ids").count(), 1);
    assert_eq!(
        bulk.matches("list_ping_target_assignment_records_for_targets")
            .count(),
        1
    );
    assert_eq!(bulk.matches("resolve_saved_selectors_batch").count(), 1);
    assert!(!bulk.contains("ping_target_record(*target_id)"));
    assert!(!bulk.contains("resolve_selector("));
}

#[test]
fn bulk_monitoring_share_refresh_uses_one_review_and_selector_snapshot() {
    let source = include_str!("routes_monitoring.rs");
    let (_, bulk) = source
        .split_once("pub(crate) async fn bulk_update_monitoring_share_targets")
        .expect("monitoring-share bulk-update route");
    let (bulk, _) = bulk
        .split_once("pub(crate) async fn revoke_monitoring_shares")
        .expect("monitoring-share bulk-update route end");
    assert_eq!(bulk.matches("monitoring_share_records_by_ids").count(), 1);
    assert_eq!(bulk.matches("resolve_saved_selectors_batch").count(), 1);
    assert!(!bulk.contains("monitoring_share_record(*share_id)"));
    assert!(!bulk.contains("resolve_selector("));
    assert!(!bulk.contains("monitoring_share_not_found_after_update"));
}

#[test]
fn retained_client_detail_uses_complete_projection_owners() {
    let source = include_str!("routes_monitoring.rs");
    let (_, detail) = source
        .split_once("async fn client_monitoring_view")
        .expect("client monitoring loader");
    let (detail, _) = detail
        .split_once("async fn monitoring_range")
        .expect("client monitoring loader end");

    assert!(detail.contains("list_projected_telemetry_resource_history"));
    assert!(detail.contains("list_projected_telemetry_network_history"));
    assert!(detail.contains("dashboard_projection_initializing"));
    assert!(!detail.contains("list_dashboard_telemetry_rollups"));
    assert!(!detail.contains("list_dashboard_telemetry_network_rates_selected"));
}

#[test]
fn current_excluded_interfaces_are_owned_only_by_single_vps_detail() {
    let source = include_str!("routes_monitoring.rs");
    let (_, operator_detail) = source
        .split_once("pub(crate) async fn get_client_monitoring")
        .expect("operator VPS detail route");
    let (operator_detail, _) = operator_detail
        .split_once("pub(crate) async fn list_ping_targets")
        .expect("operator VPS detail route end");
    assert!(operator_detail.contains("CurrentNetworkDetail::SingleVpsDetail"));

    let (_, public_detail) = source
        .split_once("pub(crate) async fn public_monitoring_share_data")
        .expect("public shared detail route");
    let (public_detail, _) = public_detail
        .split_once("fn public_monitoring_cards_singleflight_key")
        .expect("public shared detail route end");
    assert!(public_detail.contains("CurrentNetworkDetail::Hidden"));
    assert!(!public_detail.contains("CurrentNetworkDetail::SingleVpsDetail"));

    let (_, detail) = source
        .split_once("async fn client_monitoring_view")
        .expect("client monitoring loader");
    let (detail, _) = detail
        .split_once("async fn monitoring_range")
        .expect("client monitoring loader end");
    assert!(detail.contains("list_latest_telemetry_network_rates_for_vps_detail(client_id)"));
    assert!(detail.contains("list_telemetry_tunnels_for_vps_detail(client_id)"));
    assert!(detail.contains("network_current_detail,"));
    assert!(detail.contains("tunnel_current_detail,"));
    assert!(detail.contains("list_projected_telemetry_network_history"));

    let frontend = include_str!("../../../../../frontend/src/panels/VpsMonitoringDetailPanel.tsx");
    let frontend_text = frontend.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(frontend.contains("[...data.network_current_detail].sort"));
    assert!(frontend.contains("currentNetworkDetail.map"));
    assert!(frontend.contains("currentTunnelDetail.map"));
    assert!(frontend.contains("Current interface telemetry"));
    assert!(frontend.contains("formatByteRateFromBitsPerSecond(rate.rx_bps_avg)"));
    assert!(frontend.contains("formatByteRateFromBitsPerSecond(rate.tx_bps_avg)"));
    assert!(frontend_text.contains("Interfaces excluded by network.interfaces are shown only here"));
    assert!(
        frontend_text.contains("eligible evidence follows history and traffic-accounting rules")
    );
    assert!(frontend.contains("networkChart(data.network, timeline"));
    assert!(!frontend.contains("networkChart(data.network_current_detail"));
    assert!(!frontend.contains("networkChart(data.tunnel_current_detail"));
}

#[test]
fn every_shared_view_mutation_requires_a_nonempty_bounded_target_set() {
    let empty = validate_monitoring_share_targets(&[])
        .expect_err("active shared views cannot have an empty frozen target set");
    assert_eq!(empty.code, "monitoring_share_target_selection_required");

    let bounded = vec!["vps".to_string(); super::MAX_SHARE_TARGETS];
    validate_monitoring_share_targets(&bounded).expect("the documented maximum is valid");

    let oversized = vec!["vps".to_string(); super::MAX_SHARE_TARGETS + 1];
    let oversized = validate_monitoring_share_targets(&oversized)
        .expect_err("the shared-view target maximum must be enforced consistently");
    assert_eq!(oversized.code, "monitoring_share_target_count_too_large");
}

#[test]
fn monitoring_share_optional_visibility_defaults_remain_private() {
    let visibility: MonitoringShareVisibilityRequest =
        serde_json::from_value(serde_json::json!({})).unwrap();

    assert!(!visibility.identity_context);
    assert!(!visibility.billing);
    assert!(!visibility.system_information);
    assert!(visibility.resources);
    assert!(visibility.network);
    assert!(visibility.traffic);
    assert!(visibility.ping);
    assert!(visibility.detail_history);
}

#[test]
fn public_projection_age_follows_every_projected_telemetry_group() {
    let mut visibility = MonitoringShareVisibilityView {
        identity_context: false,
        billing: false,
        system_information: false,
        resources: false,
        network: false,
        traffic: false,
        ping: false,
        detail_history: false,
    };
    assert!(!public_visibility_uses_projected_telemetry(&visibility));

    let selectors: [fn(&mut MonitoringShareVisibilityView); 5] = [
        |value: &mut MonitoringShareVisibilityView| value.system_information = true,
        |value: &mut MonitoringShareVisibilityView| value.resources = true,
        |value: &mut MonitoringShareVisibilityView| value.network = true,
        |value: &mut MonitoringShareVisibilityView| value.traffic = true,
        |value: &mut MonitoringShareVisibilityView| value.ping = true,
    ];
    for select in selectors {
        select(&mut visibility);
        assert!(public_visibility_uses_projected_telemetry(&visibility));
        visibility.system_information = false;
        visibility.resources = false;
        visibility.network = false;
        visibility.traffic = false;
        visibility.ping = false;
    }
}

#[test]
fn public_read_cache_keys_are_fenced_by_the_monotonic_share_revision() {
    let client = AgentView {
        id: "shared-vps".to_string(),
        display_name: "Shared VPS".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    };
    let mut share = MonitoringShareRecord {
        id: uuid::Uuid::new_v4(),
        name: "Status".to_string(),
        token_secret: "secret".to_string(),
        selector_expression: "*".to_string(),
        targets: vec![MonitoringShareTargetRecord {
            client_id: client.id.clone(),
            public_client_key: "a".repeat(64),
        }],
        visibility: MonitoringShareVisibilityView {
            identity_context: false,
            billing: false,
            system_information: false,
            resources: true,
            network: true,
            traffic: true,
            ping: true,
            detail_history: true,
        },
        expires_at: "100".to_string(),
        revoked_at: None,
        created_at: "1".to_string(),
        updated_at: "revision-1".to_string(),
    };
    let range = ClientMonitoringQuery {
        window: Some("1d".to_string()),
        start_unix: None,
        end_unix: Some(90),
        points: Some(80),
    };
    let cards_before =
        public_monitoring_cards_singleflight_key(&share, std::slice::from_ref(&client), 0, 100, 1);
    let detail_before =
        public_monitoring_detail_singleflight_key(&share, &client.id, "public-key", &range);

    share.updated_at = "revision-2".to_string();
    assert_ne!(
        cards_before,
        public_monitoring_cards_singleflight_key(&share, std::slice::from_ref(&client), 0, 100, 1,)
    );
    assert_ne!(
        detail_before,
        public_monitoring_detail_singleflight_key(&share, &client.id, "public-key", &range)
    );

    let cards_before_total_change =
        public_monitoring_cards_singleflight_key(&share, std::slice::from_ref(&client), 0, 100, 1);
    assert_ne!(
        cards_before_total_change,
        public_monitoring_cards_singleflight_key(&share, std::slice::from_ref(&client), 0, 100, 2,),
        "pagination metadata must fence an otherwise unchanged public page"
    );
}

#[test]
fn retained_history_tiers_and_chart_steps_are_epoch_compatible() {
    const DAY: u64 = 86_400;
    assert_eq!(retained_resolution_for_age(2 * DAY), 60);
    assert_eq!(retained_resolution_for_age(2 * DAY + 1), 300);
    assert_eq!(retained_resolution_for_age(8 * DAY + 1), 1_800);
    assert_eq!(retained_resolution_for_age(31 * DAY + 1), 3_600);
    assert_eq!(retained_resolution_for_age(91 * DAY + 1), 10_800);
    assert_eq!(retained_resolution_for_age(181 * DAY + 1), 21_600);
    assert_eq!(retained_resolution_for_age(366 * DAY + 1), DAY);

    let exact_days = vpsman_common::TRAFFIC_COUNTER_RAW_RETENTION_DAYS as u64;
    // Exact/raw selection owns the one-day boundary; the retained-only
    // source begins with its hourly tier on either side of that boundary.
    assert_eq!(retained_traffic_resolution_for_age(exact_days * DAY), 3_600);
    assert_eq!(
        retained_traffic_resolution_for_age(exact_days * DAY + 1),
        3_600
    );
    assert_eq!(retained_traffic_resolution_for_age(366 * DAY + 1), DAY);
    assert!(traffic_uses_exact_source(exact_days * DAY));
    assert!(!traffic_uses_exact_source(exact_days * DAY + 1));

    assert_eq!(aligned_timeline_point_count(21_000, 23_000, 21_600), 2);
    assert_eq!(aligned_timeline_point_count(21_600, 23_000, 21_600), 1);

    assert_eq!(
        tier_aligned_step_secs(30 * DAY, 61 * 60, 30 * 60, 720),
        60 * 60
    );
    assert_eq!(
        tier_aligned_step_secs(30 * DAY, 121 * 60, 30 * 60, 360),
        120 * 60
    );
    assert_eq!(
        tier_aligned_step_secs(90 * DAY, 181 * 60, 60 * 60, 720),
        180 * 60
    );
    assert_eq!(
        tier_aligned_step_secs(180 * DAY, 723 * 60, 3 * 60 * 60, 360),
        12 * 60 * 60
    );
    assert_eq!(
        tier_aligned_step_secs(365 * DAY, 732 * 60, 6 * 60 * 60, 720),
        12 * 60 * 60
    );
}

#[test]
fn public_network_projection_preserves_whether_rates_are_expected() {
    let intentionally_empty = public_network_metric(&[], false);
    assert!(!intentionally_empty.rate_expected);
    assert_eq!(intentionally_empty.rx_bps, None);
    assert_eq!(intentionally_empty.tx_bps, None);
    assert_eq!(intentionally_empty.observed_at, None);

    let missing_expected = public_network_metric(&[], true);
    assert!(missing_expected.rate_expected);
    assert_eq!(missing_expected.rx_bps, None);
    assert_eq!(missing_expected.tx_bps, None);
    assert_eq!(missing_expected.observed_at, None);

    let rate = |interface: &str,
                latest_observed_at: &str,
                updated_at: &str,
                rx_bps_avg: f64,
                tx_bps_avg: f64| TelemetryNetworkRateView {
        client_id: "v-1".to_string(),
        interface: interface.to_string(),
        bucket_start: latest_observed_at.to_string(),
        bucket_secs: 60,
        sample_count: 1,
        rx_bytes_avg: 0,
        tx_bytes_avg: 0,
        latest_observed_at: latest_observed_at.to_string(),
        rx_bytes_delta: 0,
        tx_bytes_delta: 0,
        rx_bps_avg,
        tx_bps_avg,
        updated_at: updated_at.to_string(),
    };
    let projected = public_network_metric(
        &[
            rate(
                "removed0",
                "2026-01-01T00:00:00+00:00",
                "2026-01-01T00:20:00+00:00",
                100.0,
                200.0,
            ),
            rate(
                "eth0",
                "2026-01-01T00:05:00+00:00",
                "2026-01-01T00:05:01+00:00",
                10.0,
                20.0,
            ),
            rate(
                "eth1",
                "2026-01-01T00:05:00+00:00",
                "2026-01-01T00:05:02+00:00",
                30.0,
                40.0,
            ),
        ],
        true,
    );
    assert_eq!(projected.rx_bps, Some(40.0));
    assert_eq!(projected.tx_bps, Some(60.0));
    assert_eq!(
        projected.observed_at.as_deref(),
        Some("2026-01-01T00:05:00+00:00")
    );
}

#[test]
fn public_network_history_is_identical_after_selected_interface_compaction() {
    let rate = |interface: &str, rx_bps_avg: f64, tx_bps_avg: f64| TelemetryNetworkRateView {
        client_id: "v-1".to_string(),
        interface: interface.to_string(),
        bucket_start: "1000".to_string(),
        bucket_secs: 60,
        sample_count: 1,
        rx_bytes_avg: 0,
        tx_bytes_avg: 0,
        latest_observed_at: "1000".to_string(),
        rx_bytes_delta: 0,
        tx_bytes_delta: 0,
        rx_bps_avg,
        tx_bps_avg,
        updated_at: "1000".to_string(),
    };
    let legacy = public_network_points(vec![rate("eth0", 8.0, 16.0), rate("eth1", 24.0, 32.0)]);
    let compact = public_network_points(vec![rate("", 32.0, 48.0)]);

    assert_eq!(
        serde_json::to_value(compact).unwrap(),
        serde_json::to_value(legacy).unwrap()
    );
}

#[test]
fn public_monitoring_contract_has_exhaustive_explicit_allowlists() {
    let visibility = MonitoringShareVisibilityView {
        identity_context: true,
        billing: true,
        system_information: true,
        resources: true,
        network: true,
        traffic: true,
        ping: true,
        detail_history: true,
    };
    let share = PublicMonitoringShareView {
        id: uuid::Uuid::new_v4(),
        name: "Status".to_string(),
        target_count: 1,
        visibility: visibility.clone(),
        expires_at: "2".to_string(),
    };
    let range = PublicMonitoringRangeView {
        window: "15m".to_string(),
        source: "raw".to_string(),
        start_unix: 1,
        end_unix: 2,
        requested_step_secs: 60,
        effective_resolution_secs: 60,
        step_secs: 60,
        points: 16,
        effective_points: 2,
        resolutions: crate::model::MonitoringResolutionView {
            resources: 60,
            network: 60,
            ping: 60,
            traffic: 60,
        },
    };
    let resource = PublicResourceMetricView {
        bucket_start: "1".to_string(),
        bucket_secs: 60,
        sample_count: 1,
        cpu_usage_avg: Some(0.5),
        cpu_cores: 2,
        load_1: 0.1,
        load_5: 0.2,
        load_15: 0.3,
        memory_total_bytes: 100,
        memory_available_bytes: 50,
        memory_used_ratio_avg: 0.5,
        swap_sample_count: 1,
        swap_total_bytes: Some(50),
        swap_available_bytes: Some(25),
        swap_used_ratio_avg: Some(0.5),
        disk_sample_count: 1,
        disk_total_bytes: 200,
        disk_available_bytes: 150,
        disk_used_ratio_avg: 0.25,
        tcp_sockets: Some(3),
        udp_sockets: Some(4),
        connections_observed_at: Some("1".to_string()),
        observed_at: "1".to_string(),
    };
    let network = PublicNetworkMetricView {
        rate_expected: true,
        rx_bps: Some(1.0),
        tx_bps: Some(2.0),
        observed_at: Some("1".to_string()),
    };
    let network_point = PublicNetworkPointView {
        bucket_start: "1".to_string(),
        bucket_secs: 60,
        rx_bps: 1.0,
        tx_bps: 2.0,
    };
    let port_speed = PublicPortSpeedView {
        bps: 1_000_000_000,
        display: "1 Gbps".to_string(),
    };
    let billing = PublicBillingPlanView {
        disabled: false,
        display: "5.00 USD/m".to_string(),
        period_code: Some("m".to_string()),
        cycle: Some("day 1 monthly".to_string()),
    };
    let system_information = PublicSystemInformationView {
        os_name: Some("Debian GNU/Linux 12".to_string()),
        architecture: Some("x86_64".to_string()),
        cpu_model: Some("AMD EPYC".to_string()),
        kernel_release: Some("6.12.1".to_string()),
        virtualization: Some("kvm".to_string()),
        reported_at: Some("1".to_string()),
        uptime_secs: Some(86_400),
        uptime_observed_at: Some("2".to_string()),
    };
    let traffic = PublicTrafficMetricView {
        configured: true,
        reset_day: Some(1),
        reset_hour: Some(0),
        cycle_start: Some("1".to_string()),
        cycle_end: Some("2".to_string()),
        rx_bytes: Some(1),
        tx_bytes: Some(2),
        total_bytes: Some(3),
        diagnostic_rx_bytes: Some(10),
        diagnostic_tx_bytes: Some(20),
        diagnostic_total_bytes: Some(30),
        quota_rx_bytes: Some(4),
        quota_tx_bytes: Some(5),
        quota_total_bytes: Some(9),
        cycle_percent: Some(33.3),
        state: "ok".to_string(),
        observed_at: Some("1".to_string()),
        port_speed: Some(port_speed.clone()),
    };
    let traffic_point = PublicTrafficHistoryPointView {
        bucket_start: "1".to_string(),
        bucket_secs: 60,
        sample_count: 1,
        reset_count: 0,
        rx_bytes: Some(1),
        tx_bytes: Some(2),
        total_bytes: Some(3),
    };
    let ping = PublicPingMetricView {
        target_name: "Gateway".to_string(),
        state: "ok".to_string(),
        status: Some("ok".to_string()),
        latency_avg_ms: Some(10.0),
        loss_ratio: Some(0.0),
        checked_at: Some("1".to_string()),
    };
    let ping_point = PublicPingPointView {
        target_name: "Gateway".to_string(),
        bucket_start: "1".to_string(),
        bucket_secs: 60,
        sample_count: 1,
        latency_avg_ms: Some(10.0),
        loss_ratio: 0.0,
        status: "ok".to_string(),
        checked_at: "1".to_string(),
    };
    let card = PublicMonitoringCardView {
        client_key: "a".repeat(64),
        display_name: "VPS".to_string(),
        status: "online".to_string(),
        projection_pending_since: Some("2".to_string()),
        projection_checked_at: Some("13".to_string()),
        tags: Some(vec!["provider:test".to_string()]),
        product_name: Some("Storage-Box 4".to_string()),
        billing: Some(billing.clone()),
        system_information: Some(system_information.clone()),
        resources: Some(resource.clone()),
        resource_history: Some(vec![resource.clone()]),
        network: Some(network.clone()),
        network_history: Some(vec![network_point.clone()]),
        traffic: Some(traffic.clone()),
        primary_ping: Some(ping.clone()),
        primary_ping_history: Some(vec![ping_point.clone()]),
    };
    let detail = PublicMonitoringDetailView {
        client_key: card.client_key.clone(),
        range: range.clone(),
        resources: Some(vec![resource.clone()]),
        network: Some(vec![network_point.clone()]),
        traffic: Some(vec![traffic_point.clone()]),
        ping_targets: Some(vec![ping.clone()]),
        ping: Some(vec![ping_point.clone()]),
    };
    let data = PublicMonitoringDataView {
        share: share.clone(),
        cards: vec![card.clone()],
        offset: 0,
        total: 1,
        next_offset: None,
        detail: Some(detail.clone()),
    };
    let bootstrap = PublicMonitoringShareBootstrapView {
        share: share.clone(),
        visitor_id: uuid::Uuid::new_v4(),
    };

    assert_serialized_keys(
        "visibility",
        &visibility,
        &[
            "billing",
            "detail_history",
            "identity_context",
            "network",
            "ping",
            "resources",
            "system_information",
            "traffic",
        ],
    );
    assert_serialized_keys(
        "share",
        &share,
        &["expires_at", "id", "name", "target_count", "visibility"],
    );
    assert_serialized_keys("bootstrap", &bootstrap, &["share", "visitor_id"]);
    assert_serialized_keys(
        "range",
        &range,
        &[
            "effective_points",
            "effective_resolution_secs",
            "end_unix",
            "points",
            "requested_step_secs",
            "resolutions",
            "source",
            "start_unix",
            "step_secs",
            "window",
        ],
    );
    assert_serialized_keys(
        "resource",
        &resource,
        &[
            "bucket_secs",
            "bucket_start",
            "connections_observed_at",
            "cpu_cores",
            "cpu_usage_avg",
            "disk_available_bytes",
            "disk_sample_count",
            "disk_total_bytes",
            "disk_used_ratio_avg",
            "load_1",
            "load_15",
            "load_5",
            "memory_available_bytes",
            "memory_total_bytes",
            "memory_used_ratio_avg",
            "observed_at",
            "sample_count",
            "swap_available_bytes",
            "swap_sample_count",
            "swap_total_bytes",
            "swap_used_ratio_avg",
            "tcp_sockets",
            "udp_sockets",
        ],
    );
    assert_serialized_keys(
        "network",
        &network,
        &["observed_at", "rate_expected", "rx_bps", "tx_bps"],
    );
    assert_serialized_keys(
        "network point",
        &network_point,
        &["bucket_secs", "bucket_start", "rx_bps", "tx_bps"],
    );
    assert_serialized_keys("port speed", &port_speed, &["bps", "display"]);
    assert_serialized_keys(
        "billing",
        &billing,
        &["cycle", "disabled", "display", "period_code"],
    );
    assert_serialized_keys(
        "system information",
        &system_information,
        &[
            "architecture",
            "cpu_model",
            "kernel_release",
            "os_name",
            "reported_at",
            "uptime_observed_at",
            "uptime_secs",
            "virtualization",
        ],
    );
    assert_serialized_keys(
        "traffic",
        &traffic,
        &[
            "configured",
            "cycle_end",
            "cycle_percent",
            "cycle_start",
            "diagnostic_rx_bytes",
            "diagnostic_total_bytes",
            "diagnostic_tx_bytes",
            "observed_at",
            "port_speed",
            "quota_rx_bytes",
            "quota_total_bytes",
            "quota_tx_bytes",
            "reset_day",
            "reset_hour",
            "rx_bytes",
            "state",
            "total_bytes",
            "tx_bytes",
        ],
    );
    assert_serialized_keys(
        "traffic point",
        &traffic_point,
        &[
            "bucket_secs",
            "bucket_start",
            "reset_count",
            "rx_bytes",
            "sample_count",
            "total_bytes",
            "tx_bytes",
        ],
    );
    assert_serialized_keys(
        "ping",
        &ping,
        &[
            "checked_at",
            "latency_avg_ms",
            "loss_ratio",
            "state",
            "status",
            "target_name",
        ],
    );
    assert_serialized_keys(
        "ping point",
        &ping_point,
        &[
            "bucket_secs",
            "bucket_start",
            "checked_at",
            "latency_avg_ms",
            "loss_ratio",
            "sample_count",
            "status",
            "target_name",
        ],
    );
    assert_serialized_keys(
        "card",
        &card,
        &[
            "billing",
            "client_key",
            "display_name",
            "network",
            "network_history",
            "primary_ping",
            "primary_ping_history",
            "product_name",
            "projection_checked_at",
            "projection_pending_since",
            "resource_history",
            "resources",
            "status",
            "system_information",
            "tags",
            "traffic",
        ],
    );
    assert_serialized_keys(
        "detail",
        &detail,
        &[
            "client_key",
            "network",
            "ping",
            "ping_targets",
            "range",
            "resources",
            "traffic",
        ],
    );
    assert_serialized_keys(
        "data",
        &data,
        &["cards", "detail", "next_offset", "offset", "share", "total"],
    );
}

fn assert_serialized_keys(label: &str, value: &impl serde::Serialize, expected: &[&str]) {
    let value = serde_json::to_value(value).unwrap();
    let actual = value
        .as_object()
        .unwrap_or_else(|| panic!("{label} must serialize as an object"))
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "{label} public field allowlist changed");
}

#[test]
fn unconfigured_public_traffic_omits_retained_cycle_evidence() {
    let traffic = PublicTrafficMetricView {
        configured: false,
        reset_day: None,
        reset_hour: None,
        cycle_start: None,
        cycle_end: None,
        rx_bytes: None,
        tx_bytes: None,
        total_bytes: None,
        diagnostic_rx_bytes: None,
        diagnostic_tx_bytes: None,
        diagnostic_total_bytes: None,
        quota_rx_bytes: None,
        quota_tx_bytes: None,
        quota_total_bytes: None,
        cycle_percent: None,
        state: "unconfigured".to_string(),
        observed_at: None,
        port_speed: Some(PublicPortSpeedView {
            bps: 1_000_000_000,
            display: "1 Gbps".to_string(),
        }),
    };
    assert_serialized_keys(
        "unconfigured traffic",
        &traffic,
        &["configured", "port_speed", "reset_hour", "state"],
    );
}

#[test]
fn public_traffic_keeps_diagnostics_separate_from_billing_totals() {
    let projected = public_traffic_metric(
        TrafficAccountingRecord {
            client_id: "v-1".to_string(),
            selectors: vec!["eth0+tx".to_string()],
            selector_hash: "selector-hash".to_string(),
            cycle_start: Some("1".to_string()),
            cycle_end: Some("2".to_string()),
            reset_day: Some(1),
            reset_hour: Some(5),
            rx_bytes: 0,
            tx_bytes: 200,
            total_bytes: 200,
            diagnostic_rx_bytes: 100,
            diagnostic_tx_bytes: 200,
            diagnostic_total_bytes: 300,
            latest_rx_bytes: 0,
            latest_tx_bytes: 2_000,
            latest_total_bytes: 2_000,
            quota_rx_bytes: None,
            quota_tx_bytes: None,
            quota_total_bytes: Some(1_000),
            cycle_percent: Some(20.0),
            state: "ok".to_string(),
            incomplete_reasons: Vec::new(),
            last_sample_at: Some("2".to_string()),
            counter_epochs_seen: 1,
            updated_at: "2".to_string(),
            selector_breakdown: Vec::new(),
        },
        None,
    );

    assert_eq!(projected.rx_bytes, Some(0));
    assert_eq!(projected.tx_bytes, Some(200));
    assert_eq!(projected.total_bytes, Some(200));
    assert_eq!(projected.diagnostic_rx_bytes, Some(100));
    assert_eq!(projected.diagnostic_tx_bytes, Some(200));
    assert_eq!(projected.diagnostic_total_bytes, Some(300));
    assert_eq!(projected.cycle_percent, Some(20.0));
    assert_eq!(projected.reset_day, Some(1));
    assert_eq!(projected.reset_hour, Some(5));
}

#[test]
fn public_no_reset_traffic_exposes_sentinel_without_inventing_cycle_bounds() {
    let projected = public_traffic_metric(
        TrafficAccountingRecord {
            client_id: "v-1".to_string(),
            selectors: vec!["eth0".to_string()],
            selector_hash: "selector-hash".to_string(),
            cycle_start: None,
            cycle_end: None,
            reset_day: Some(-1),
            reset_hour: None,
            rx_bytes: 100,
            tx_bytes: 200,
            total_bytes: 300,
            diagnostic_rx_bytes: 100,
            diagnostic_tx_bytes: 200,
            diagnostic_total_bytes: 300,
            latest_rx_bytes: 1_000,
            latest_tx_bytes: 2_000,
            latest_total_bytes: 3_000,
            quota_rx_bytes: None,
            quota_tx_bytes: None,
            quota_total_bytes: Some(-1),
            cycle_percent: None,
            state: "ok".to_string(),
            incomplete_reasons: Vec::new(),
            last_sample_at: Some("2".to_string()),
            counter_epochs_seen: 1,
            updated_at: "2".to_string(),
            selector_breakdown: Vec::new(),
        },
        None,
    );

    assert!(projected.configured);
    assert_eq!(projected.reset_day, Some(-1));
    assert_eq!(projected.reset_hour, None);
    assert_eq!(projected.cycle_start, None);
    assert_eq!(projected.cycle_end, None);
    assert_eq!(projected.total_bytes, Some(300));
}

#[test]
fn public_billing_projection_allowlists_period_code_for_cycle_formatting() {
    let projected = public_billing_plan(BillingPlanView {
        disabled: false,
        price: Some("120.00".to_string()),
        currency: Some("USD".to_string()),
        currency_display: Some("USD".to_string()),
        period: Some("year".to_string()),
        period_code: Some("y".to_string()),
        cycle: Some("06-15".to_string()),
        display: "120.00 USD/y".to_string(),
    });

    assert_eq!(projected.period_code.as_deref(), Some("y"));
    assert_eq!(projected.cycle.as_deref(), Some("06-15"));
}
