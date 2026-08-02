use std::collections::BTreeSet;

use super::{monitoring_range, ClientMonitoringQuery};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use tokio::sync::broadcast;
use tower::ServiceExt;

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{
        MonitoringShareVisibilityView, PublicMonitoringCardView, PublicMonitoringDataView,
        PublicMonitoringDetailView, PublicMonitoringRangeView, PublicMonitoringShareBootstrapView,
        PublicMonitoringShareView, PublicNetworkMetricView, PublicNetworkPointView,
        PublicPingMetricView, PublicPingPointView, PublicPortSpeedView, PublicResourceMetricView,
        PublicTrafficHistoryPointView, PublicTrafficMetricView,
    },
    repository::{MemoryState, Repository},
    state::{AppState, DispatcherRuntimeConfig},
};

fn router_test_state() -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo: Repository::Memory(MemoryState::default()),
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32_768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: "config/vpsman.toml".into(),
        dispatcher_config: DispatcherRuntimeConfig::default(),
    }
}

#[tokio::test]
async fn monitoring_share_routes_are_registered_at_their_public_and_operator_paths() {
    let router = crate::routes::build_router(router_test_state());

    for (method, uri, body) in [
        ("GET", "/api/v1/monitoring-shares", Body::empty()),
        (
            "POST",
            "/api/v1/monitoring-shares",
            Body::from(
                r#"{"name":"Status","visibility":{},"expires_in_secs":3600,"confirmed":true}"#,
            ),
        ),
        (
            "POST",
            "/api/v1/monitoring-shares/extend",
            Body::from(r#"{"share_ids":[],"extend_by_secs":3600}"#),
        ),
        (
            "POST",
            "/api/v1/monitoring-shares/revoke",
            Body::from(r#"{"share_ids":[]}"#),
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }

    let mut public_request = Request::builder()
        .method("GET")
        .uri(format!(
            "/api/v1/public/monitoring-shares/{}/bootstrap",
            uuid::Uuid::new_v4()
        ))
        .body(Body::empty())
        .unwrap();
    public_request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:41000".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let response = router.oneshot(public_request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn fifteen_minutes_is_always_raw_without_a_legacy_alias() {
    let state = router_test_state();
    let Repository::Memory(memory) = &state.repo else {
        unreachable!()
    };
    memory.traffic_counter_samples.write().await.push(
        crate::model_alert_policies::TrafficCounterSampleRecord {
            client_id: "v-1".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            observed_at: crate::unix_now().saturating_sub(7_200).to_string(),
            observed_unix: i64::try_from(crate::unix_now().saturating_sub(7_200)).unwrap(),
            rx_bytes: 0,
            tx_bytes: 0,
            rx_counter_epoch: 0,
            tx_counter_epoch: 0,
            sample_source: "test".to_string(),
        },
    );
    let clients = vec!["v-1".to_string()];
    let query = |window: &str| ClientMonitoringQuery {
        window: Some(window.to_string()),
        start_unix: None,
        end_unix: None,
        points: None,
    };

    assert_eq!(
        monitoring_range(&state, &clients, &query("15m"))
            .await
            .unwrap()
            .source,
        "raw"
    );
    assert_eq!(
        monitoring_range(&state, &clients, &query("1h"))
            .await
            .unwrap()
            .source,
        "minute"
    );
    assert!(monitoring_range(&state, &clients, &query("R"))
        .await
        .is_err());
}

#[test]
fn public_monitoring_contract_has_exhaustive_explicit_allowlists() {
    let visibility = MonitoringShareVisibilityView {
        identity_context: true,
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
        step_secs: 60,
        points: 16,
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
        disk_total_bytes: 200,
        disk_available_bytes: 150,
        tcp_sockets: Some(3),
        udp_sockets: Some(4),
        connections_observed_at: Some("1".to_string()),
        observed_at: "1".to_string(),
    };
    let network = PublicNetworkMetricView {
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
    let traffic = PublicTrafficMetricView {
        configured: true,
        cycle_start: "1".to_string(),
        cycle_end: "2".to_string(),
        rx_bytes: 1,
        tx_bytes: 2,
        total_bytes: 3,
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
        tags: Some(vec!["provider:test".to_string()]),
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
            "detail_history",
            "identity_context",
            "network",
            "ping",
            "resources",
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
            "end_unix",
            "points",
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
            "disk_total_bytes",
            "load_1",
            "load_15",
            "load_5",
            "memory_available_bytes",
            "memory_total_bytes",
            "observed_at",
            "sample_count",
            "tcp_sockets",
            "udp_sockets",
        ],
    );
    assert_serialized_keys("network", &network, &["observed_at", "rx_bps", "tx_bps"]);
    assert_serialized_keys(
        "network point",
        &network_point,
        &["bucket_secs", "bucket_start", "rx_bps", "tx_bps"],
    );
    assert_serialized_keys("port speed", &port_speed, &["bps", "display"]);
    assert_serialized_keys(
        "traffic",
        &traffic,
        &[
            "configured",
            "cycle_end",
            "cycle_percent",
            "cycle_start",
            "observed_at",
            "port_speed",
            "quota_rx_bytes",
            "quota_total_bytes",
            "quota_tx_bytes",
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
            "client_key",
            "display_name",
            "network",
            "network_history",
            "primary_ping",
            "primary_ping_history",
            "resource_history",
            "resources",
            "status",
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
