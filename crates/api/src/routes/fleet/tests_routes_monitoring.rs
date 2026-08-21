use std::collections::BTreeSet;

use super::{
    aligned_timeline_point_count, build_monitoring_cards_page, client_monitoring_view,
    enrich_monitoring_share_target_evidence, monitoring_agents, monitoring_cards_for_agents,
    monitoring_range, network_rate_is_current, public_billing_plan, public_monitoring_card,
    public_network_metric, public_traffic_metric, retained_resolution_for_age,
    retained_traffic_resolution_for_age, tier_aligned_step_secs, traffic_uses_exact_source,
    ClientMonitoringQuery,
};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{
        AgentView, BillingPlanView, MonitoringShareRecord, MonitoringShareTargetRecord,
        MonitoringShareView, MonitoringShareVisibilityRequest, MonitoringShareVisibilityView,
        PublicBillingPlanView, PublicMonitoringCardView, PublicMonitoringDataView,
        PublicMonitoringDetailView, PublicMonitoringRangeView, PublicMonitoringShareBootstrapView,
        PublicMonitoringShareView, PublicNetworkMetricView, PublicNetworkPointView,
        PublicPingMetricView, PublicPingPointView, PublicPortSpeedView, PublicResourceMetricView,
        PublicSystemInformationView, PublicTrafficHistoryPointView, PublicTrafficMetricView,
        TelemetryNetworkRateView, TelemetryRollupView, TelemetrySampleView,
    },
    model_alert_policies::TrafficAccountingRecord,
    repository::{MemoryState, Repository},
    state::{AppState, DispatcherRuntimeConfig},
};

fn router_test_state() -> AppState {
    let (events, _) = crate::state::WsEventBus::new(1);
    AppState {
        repo: Repository::Memory(MemoryState::default()),
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        job_output_artifact_min_bytes: 32_768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: "config/vpsman.toml".into(),
        dispatcher_config: DispatcherRuntimeConfig::default(),
    }
}

#[tokio::test]
async fn suspended_agent_is_absent_from_monitoring_cards_and_detail() {
    let state = router_test_state();
    let agent = AgentView {
        id: "suspended-a".to_string(),
        display_name: "Suspended VPS".to_string(),
        status: "suspended".to_string(),
        tags: vec!["provider:test".to_string()],
        registration_ip: None,
        last_ip: None,
        last_seen_at: Some(crate::unix_now().to_string()),
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: Some(uuid::Uuid::new_v4()),
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    };
    let Repository::Memory(memory) = &state.repo else {
        unreachable!();
    };
    memory.agents.write().await.push(agent.clone());

    assert!(monitoring_agents(&state, None).await.unwrap().is_empty());
    assert!(monitoring_cards_for_agents(&state, vec![agent])
        .await
        .unwrap()
        .is_empty());
    let error = client_monitoring_view(
        &state,
        "suspended-a",
        &ClientMonitoringQuery {
            window: None,
            start_unix: None,
            end_unix: None,
            points: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::NOT_FOUND);
    assert_eq!(error.code, "monitoring_client_not_found");
}

#[tokio::test]
async fn home_monitoring_projection_omits_only_unrendered_histories() {
    let state = router_test_state();
    let Repository::Memory(memory) = &state.repo else {
        unreachable!()
    };
    let now = crate::unix_now();
    let observed_at = now.saturating_sub(30).to_string();
    let agent = AgentView {
        id: "v-history".to_string(),
        display_name: "History VPS".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: Some(observed_at.clone()),
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    };
    memory.agents.write().await.push(agent.clone());
    memory
        .telemetry_samples
        .write()
        .await
        .push(TelemetrySampleView {
            id: uuid::Uuid::new_v4(),
            client_id: agent.id.clone(),
            observed_at: observed_at.clone(),
            cpu_load_1: 0.5,
            memory_total_bytes: 1_000,
            memory_available_bytes: 500,
            payload: serde_json::to_value(vpsman_common::AgentMetrics::default()).unwrap(),
        });
    memory
        .telemetry_rollups
        .write()
        .await
        .push(monitoring_test_rollup(&agent.id, &observed_at));

    let default_projection = build_monitoring_cards_page(&state, None, 1_000, 0, true)
        .await
        .unwrap();
    let home_projection = build_monitoring_cards_page(&state, None, 1_000, 0, false)
        .await
        .unwrap();

    assert_eq!(default_projection.total, 1);
    assert_eq!(default_projection.items.len(), 1);
    assert!(!default_projection.items[0].resource_history.is_empty());
    assert_eq!(home_projection.total, 1);
    assert_eq!(home_projection.items.len(), 1);
    assert_eq!(home_projection.next_offset, None);
    assert!(home_projection.items[0].resources.is_some());
    assert!(home_projection.items[0].resource_history.is_empty());
    assert!(home_projection.items[0].network_history.is_empty());
    assert!(home_projection.items[0].primary_ping_history.is_empty());
}

fn monitoring_test_rollup(client_id: &str, observed_at: &str) -> TelemetryRollupView {
    TelemetryRollupView {
        client_id: client_id.to_string(),
        bucket_start: observed_at.to_string(),
        bucket_secs: 60,
        sample_count: 1,
        cpu_usage_sample_count: 0,
        cpu_usage_avg: None,
        cpu_usage_max: None,
        cpu_cores_max: 1,
        cpu_load_1_avg: 0.5,
        cpu_load_1_max: 0.5,
        cpu_load_5_avg: 0.5,
        cpu_load_5_max: 0.5,
        cpu_load_15_avg: 0.5,
        cpu_load_15_max: 0.5,
        memory_total_bytes_max: 1_000,
        memory_available_bytes_avg: 500,
        memory_available_bytes_min: 500,
        memory_used_ratio_avg: 0.5,
        memory_used_ratio_max: 0.5,
        swap_sample_count: 0,
        swap_total_bytes_max: None,
        swap_available_bytes_avg: None,
        swap_available_bytes_min: None,
        swap_used_ratio_avg: None,
        swap_used_ratio_max: None,
        disk_sample_count: 1,
        disk_total_bytes_max: 2_000,
        disk_available_bytes_avg: 1_000,
        disk_available_bytes_min: 1_000,
        disk_used_ratio_avg: 0.5,
        disk_used_ratio_max: 0.5,
        network_rx_bytes_max: 0,
        network_tx_bytes_max: 0,
        connections_sample_count: 0,
        tcp_sockets_latest: None,
        udp_sockets_latest: None,
        connections_observed_at: None,
        latest_observed_at: observed_at.to_string(),
        updated_at: observed_at.to_string(),
    }
}

#[tokio::test]
async fn product_name_is_a_canonical_narrow_private_and_identity_gated_projection() {
    let state = router_test_state();
    let Repository::Memory(memory) = &state.repo else {
        unreachable!()
    };
    let agent = AgentView {
        id: "v-1".to_string(),
        display_name: "VPS 1".to_string(),
        status: "online".to_string(),
        tags: vec!["provider:test".to_string()],
        registration_ip: None,
        last_ip: None,
        last_seen_at: Some(crate::unix_now().to_string()),
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    };
    memory.agents.write().await.push(agent.clone());
    memory
        .vps_rule_values
        .write()
        .await
        .push(crate::model_alert_policies::VpsRuleValueRecord {
            client_id: agent.id.clone(),
            key: vpsman_common::VPS_RULE_KEY_PRODUCT_NAME.to_string(),
            value_raw: "  Storage-Box\t 4  ".to_string(),
            stored_value_raw: None,
            value_json: serde_json::json!({"stale": true}),
            parsed_display: "stale".to_string(),
            state: "ok".to_string(),
            validation_errors: Vec::new(),
            source_kind: "operator".to_string(),
            source_id: None,
            updated_by: None,
            updated_at: "1".to_string(),
        });

    let card = monitoring_cards_for_agents(&state, vec![agent.clone()])
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(card.product_name.as_deref(), Some("Storage-Box 4"));
    let detail = client_monitoring_view(
        &state,
        &agent.id,
        &ClientMonitoringQuery {
            window: Some("15m".to_string()),
            start_unix: None,
            end_unix: None,
            points: Some(2),
        },
    )
    .await
    .unwrap();
    assert_eq!(detail.product_name.as_deref(), Some("Storage-Box 4"));

    let share = |identity_context| MonitoringShareRecord {
        id: uuid::Uuid::new_v4(),
        name: "Status".to_string(),
        token_secret: "secret".to_string(),
        selector_expression: "*".to_string(),
        targets: vec![MonitoringShareTargetRecord {
            client_id: agent.id.clone(),
            public_client_key: "a".repeat(64),
        }],
        visibility: MonitoringShareVisibilityView {
            identity_context,
            billing: false,
            system_information: false,
            resources: false,
            network: false,
            traffic: false,
            ping: false,
            detail_history: false,
        },
        expires_at: crate::unix_now().saturating_add(3_600).to_string(),
        revoked_at: None,
        revoked_by: None,
        created_by: None,
        created_at: "1".to_string(),
        updated_at: "1".to_string(),
    };
    let visible = public_monitoring_card(card.clone(), &share(true)).unwrap();
    assert_eq!(visible.product_name.as_deref(), Some("Storage-Box 4"));
    assert_eq!(
        serde_json::to_value(visible).unwrap()["product_name"],
        "Storage-Box 4"
    );

    let hidden = public_monitoring_card(card, &share(false)).unwrap();
    assert!(hidden.product_name.is_none());
    assert!(serde_json::to_value(hidden)
        .unwrap()
        .get("product_name")
        .is_none());
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

#[tokio::test]
async fn shared_view_list_exposes_frozen_targets_and_drift_for_operator() {
    let state = router_test_state();
    let Repository::Memory(memory) = &state.repo else {
        unreachable!()
    };
    memory.agents.write().await.push(AgentView {
        id: "v-1".to_string(),
        display_name: "v-1".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: Some(crate::unix_now().to_string()),
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    });
    let mut shares = vec![MonitoringShareView {
        id: uuid::Uuid::new_v4(),
        name: "Status".to_string(),
        selector_expression: "*".to_string(),
        target_count: 1,
        target_client_ids: vec!["v-1".to_string()],
        target_update_available: false,
        target_update_evidence_available: false,
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
        status: "active".to_string(),
        expires_at: crate::unix_now().saturating_add(3_600).to_string(),
        revoked_at: None,
        created_by: Some("operator".to_string()),
        created_at: crate::unix_now().to_string(),
        updated_at: crate::unix_now().to_string(),
        visitor_count: 0,
        first_visited_at: None,
        last_visited_at: None,
    }];

    enrich_monitoring_share_target_evidence(&state, &mut shares, true)
        .await
        .unwrap();
    assert!(!shares[0].target_update_available);
    assert!(shares[0].target_update_evidence_available);
    memory.agents.write().await.push(AgentView {
        id: "v-2".to_string(),
        display_name: "v-2".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: Some(crate::unix_now().to_string()),
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    });
    enrich_monitoring_share_target_evidence(&state, &mut shares, true)
        .await
        .unwrap();
    assert!(shares[0].target_update_available);
    let json = serde_json::to_value(&shares[0]).unwrap();
    assert_eq!(json["target_client_ids"], serde_json::json!(["v-1"]));
    assert_eq!(json["target_update_available"], true);

    shares[0].selector_expression = "vps.rules:traffic.reset_day".to_string();
    enrich_monitoring_share_target_evidence(&state, &mut shares, false)
        .await
        .unwrap();
    assert_eq!(shares[0].target_client_ids, ["v-1"]);
    assert!(!shares[0].target_update_available);
    assert!(!shares[0].target_update_evidence_available);

    shares[0].selector_expression = "(".to_string();
    enrich_monitoring_share_target_evidence(&state, &mut shares, true)
        .await
        .unwrap();
    assert_eq!(shares[0].target_client_ids, ["v-1"]);
    assert!(!shares[0].target_update_evidence_available);

    shares[0].selector_expression = "vps.rules:traffic.reset_day".to_string();
    let mut ordinary_share = shares[0].clone();
    ordinary_share.id = uuid::Uuid::new_v4();
    ordinary_share.selector_expression = "*".to_string();
    shares.push(ordinary_share);
    memory
        .vps_rule_values
        .write()
        .await
        .push(crate::model_alert_policies::VpsRuleValueRecord {
            client_id: "v-1".to_string(),
            key: vpsman_common::VPS_RULE_KEY_NETWORK_PORT_SPEED.to_string(),
            value_raw: "bogus".to_string(),
            stored_value_raw: None,
            value_json: serde_json::json!({"bps": 1}),
            parsed_display: "bogus".to_string(),
            state: "ok".to_string(),
            validation_errors: Vec::new(),
            source_kind: "operator".to_string(),
            source_id: None,
            updated_by: None,
            updated_at: "1".to_string(),
        });
    enrich_monitoring_share_target_evidence(&state, &mut shares, true)
        .await
        .unwrap();
    assert_eq!(shares[0].target_client_ids, ["v-1"]);
    assert!(!shares[0].target_update_evidence_available);
    assert_eq!(shares[1].target_client_ids, ["v-1"]);
    assert!(shares[1].target_update_evidence_available);
    assert!(shares[1].target_update_available);
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
            "/api/v1/monitoring-shares/update-targets",
            Body::from(r#"{"share_ids":[]}"#),
        ),
        (
            "PUT",
            "/api/v1/monitoring-shares/00000000-0000-4000-8000-000000000001",
            Body::from(
                r#"{"name":"Status","selector_expression":"*","target_client_ids":[],"visibility":{},"expected_updated_at":"1"}"#,
            ),
        ),
        (
            "POST",
            "/api/v1/monitoring-shares/revoke",
            Body::from(r#"{"share_ids":[]}"#),
        ),
        (
            "GET",
            "/api/v1/monitoring-shares/00000000-0000-4000-8000-000000000001/url",
            Body::empty(),
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

    let viewer_only = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/monitoring-shares/00000000-0000-4000-8000-000000000001")
                .header("content-type", "application/json")
                .header("x-vpsman-share-token", "viewer-bearer-is-read-only")
                .body(Body::from(
                    r#"{"name":"Status","selector_expression":"*","target_client_ids":[],"visibility":{},"expected_updated_at":"1"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(viewer_only.status(), StatusCode::UNAUTHORIZED);

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
        "retained"
    );
    assert!(monitoring_range(&state, &clients, &query("R"))
        .await
        .is_err());
}

#[tokio::test]
async fn narrow_old_custom_range_counts_both_aligned_coarse_slots() {
    const DAY: u64 = 86_400;
    const SIX_HOURS: u64 = 6 * 60 * 60;
    let state = router_test_state();
    let clients = vec!["v-1".to_string()];
    let old_epoch = crate::unix_now().saturating_sub(200 * DAY);
    let boundary = old_epoch / SIX_HOURS * SIX_HOURS;
    let start_unix = boundary.saturating_sub(30 * 60);
    let end_unix = boundary.saturating_add(30 * 60);
    let range = monitoring_range(
        &state,
        &clients,
        &ClientMonitoringQuery {
            window: Some("custom".to_string()),
            start_unix: Some(start_unix),
            end_unix: Some(end_unix),
            points: Some(720),
        },
    )
    .await
    .unwrap();

    assert_eq!(range.source, "retained");
    assert_eq!(range.requested_step_secs, 60);
    assert_eq!(range.effective_resolution_secs, SIX_HOURS as i32);
    assert_eq!(range.step_secs, SIX_HOURS as i32);
    assert_eq!(range.effective_points, 2);
    assert_eq!(range.resolutions.resources, SIX_HOURS as i32);
    assert_eq!(range.resolutions.network, SIX_HOURS as i32);
    assert_eq!(range.resolutions.ping, SIX_HOURS as i32);
    assert_eq!(range.resolutions.traffic, SIX_HOURS as i32);
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

    assert_eq!(retained_traffic_resolution_for_age(32 * DAY), 60);
    assert_eq!(retained_traffic_resolution_for_age(32 * DAY + 1), 3_600);
    assert_eq!(retained_traffic_resolution_for_age(366 * DAY + 1), DAY);
    assert!(traffic_uses_exact_source(true, 32 * DAY));
    assert!(!traffic_uses_exact_source(true, 32 * DAY + 1));
    assert!(!traffic_uses_exact_source(false, DAY));

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
fn current_card_rates_reject_stale_and_future_interface_rows() {
    let rate = |bucket_start: &str| TelemetryNetworkRateView {
        client_id: "v-1".to_string(),
        interface: "eth0".to_string(),
        bucket_start: bucket_start.to_string(),
        bucket_secs: 60,
        sample_count: 1,
        rx_bytes_avg: 1,
        tx_bytes_avg: 2,
        rx_bytes_last: 1,
        tx_bytes_last: 2,
        rx_counter_epoch: 0,
        tx_counter_epoch: 0,
        latest_observed_at: bucket_start.to_string(),
        rx_bytes_delta: 1,
        tx_bytes_delta: 2,
        rx_bps_avg: 8.0,
        tx_bps_avg: 16.0,
        updated_at: bucket_start.to_string(),
    };

    assert!(network_rate_is_current(&rate("1000"), 1_180));
    assert!(!network_rate_is_current(&rate("1000"), 1_181));
    assert!(!network_rate_is_current(&rate("1361"), 1_180));
    assert!(!network_rate_is_current(&rate("invalid"), 1_180));
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
        &["configured", "port_speed", "state"],
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
