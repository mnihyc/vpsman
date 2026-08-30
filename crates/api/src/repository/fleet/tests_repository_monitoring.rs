use super::*;

#[test]
fn ping_retention_wake_follows_only_orphan_creating_mutations() {
    let source = include_str!("repository_monitoring.rs");
    let upsert = source
        .split_once("pub(crate) async fn upsert_ping_target")
        .unwrap()
        .1
        .split_once("pub(crate) async fn make_primary_ping_target")
        .unwrap()
        .0;
    assert!(upsert.contains("stored.generation != record.generation"));
    assert!(upsert.contains("generation_changed || removed_assignments > 0"));
    assert!(upsert.contains("notify_ping_topology_changed_in_tx"));

    let primary = source
        .split_once("pub(crate) async fn make_primary_ping_target")
        .unwrap()
        .1
        .split_once("pub(crate) async fn replace_ping_target_assignments_bulk")
        .unwrap()
        .0;
    assert!(!primary.contains("notify_ping_topology_changed_in_tx"));

    let replacement = source
        .split_once("pub(crate) async fn replace_ping_target_assignments_bulk")
        .unwrap()
        .1
        .split_once("pub(crate) async fn delete_ping_target")
        .unwrap()
        .0;
    assert!(replacement.contains("if removed_assignments > 0"));

    let delete = source
        .split_once("pub(crate) async fn delete_ping_target")
        .unwrap()
        .1
        .split_once("pub(crate) async fn mutate_ping_targets_bulk")
        .unwrap()
        .0;
    assert!(!delete.contains("notify_ping_topology_changed_in_tx"));

    let bulk = source
        .split_once("pub(crate) async fn mutate_ping_targets_bulk")
        .unwrap()
        .1
        .split_once("pub(crate) async fn ping_targets_for_client")
        .unwrap()
        .0;
    assert!(bulk.contains("generation = generation + 1"));
    assert!(bulk.contains("if topology_invalidations > 0"));
    assert!(bulk.contains("ON DELETE CASCADE"));

    let helper = source
        .split_once("async fn replace_postgres_ping_assignments")
        .unwrap()
        .1
        .split_once("async fn lock_postgres_ping_targets")
        .unwrap()
        .0;
    assert!(helper.contains("let removed = sqlx::query("));
    assert!(helper.contains("Ok(removed)"));
}

#[test]
fn ping_card_and_detail_history_read_the_effective_minute_owner() {
    let source = include_str!("repository_monitoring.rs");
    let (_, card_tail) = source
        .split_once("pub(crate) async fn list_raw_primary_ping_results_for_clients")
        .expect("primary Ping history reader");
    let (card, detail_tail) = card_tail
        .split_once("pub(crate) async fn list_raw_ping_results")
        .expect("Ping detail history reader");
    let (detail, _) = detail_tail
        .split_once("pub(crate) async fn list_ping_rollups_for_export")
        .expect("Ping detail history boundary");
    for reader in [card, detail] {
        assert!(reader.contains("JOIN telemetry_ping_points point"));
        assert!(reader.contains("point.bucket_secs = 60"));
        assert!(reader.contains("sum(latency_sum_ms) / sum(success_count)"));
        assert!(reader.contains("sum(loss_ratio_sum) / sum(sample_count)"));
        assert!(!reader.contains("FROM telemetry_ping_facts fact"));
    }
}

#[test]
fn postgres_current_telemetry_reads_use_only_the_projected_sample_pointer() {
    for query in [
        LATEST_TELEMETRY_UPTIMES_SQL,
        MONITORING_SYSTEM_INFORMATION_SQL,
    ] {
        assert!(query.contains("projection.latest_projected_sample_id"));
        assert!(!query.contains("LATERAL"));
        assert!(!query.contains("accepted_seq"));
        assert!(!query.contains("sample.observed_at"));
    }
}

#[test]
fn fleet_live_projected_uptime_keeps_existing_unsigned_integer_semantics() {
    assert!(
        LATEST_TELEMETRY_UPTIMES_SQL.contains("latest.payload -> 'uptime_secs' AS uptime_value")
    );
    assert!(!LATEST_TELEMETRY_UPTIMES_SQL.contains("latest.payload AS payload"));

    let project = |value| {
        telemetry_uptime_from_projected_value(
            "client-a".to_string(),
            "2026-08-27T00:00:00Z".to_string(),
            value,
        )
    };

    assert_eq!(
        project(Some(serde_json::json!(42))).unwrap().uptime_secs,
        42
    );
    for absent_or_invalid in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!("42")),
        Some(serde_json::json!(-1)),
        Some(serde_json::json!(1.5)),
    ] {
        assert!(project(absent_or_invalid).is_none());
    }
}

#[test]
fn public_os_name_uses_display_fields_without_exposing_raw_release_data() {
    assert_eq!(
        public_os_name(
            "NAME=Debian GNU/Linux\nVERSION_ID=\"12\"\nPRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\nID=debian\n"
        )
        .as_deref(),
        Some("Debian GNU/Linux 12 (bookworm)")
    );
    assert_eq!(
        public_os_name("NAME=Alpine Linux\nVERSION_ID=3.20\nID=alpine\n").as_deref(),
        Some("Alpine Linux 3.20")
    );
    assert!(public_os_name("not-an-os-release-document").is_none());
}

#[test]
fn monitoring_audit_metadata_is_flat_and_reserves_provenance_fields() {
    let operator = crate::tests::test_operator();
    let metadata = base_monitoring_audit_metadata(
        &operator,
        serde_json::json!({
            "target_client_ids": ["v-1", "v-2"],
            "target_count": 2,
            "result": "overridden",
            "component": "overridden",
        }),
    );

    assert_eq!(
        metadata["target_client_ids"],
        serde_json::json!(["v-1", "v-2"])
    );
    assert_eq!(metadata["target_count"], 2);
    assert_eq!(metadata["result"], "succeeded");
    assert_eq!(metadata["component"], "monitoring-controller");
    assert_eq!(metadata["operator_id"], operator.operator.id.to_string());
    assert!(metadata.get("details").is_none());
}

#[test]
fn public_share_visitor_audit_uses_canonical_request_ip_key() {
    let share = MonitoringShareRecord {
        id: Uuid::new_v4(),
        name: "Status view".to_string(),
        token_secret: "d".repeat(64),
        selector_expression: "*".to_string(),
        targets: vec![MonitoringShareTargetRecord {
            client_id: "v-1".to_string(),
            public_client_key: "1".repeat(64),
        }],
        visibility: MonitoringShareVisibilityView {
            identity_context: false,
            billing: false,
            system_information: false,
            resources: true,
            network: true,
            traffic: true,
            ping: true,
            detail_history: false,
        },
        expires_at: "200".to_string(),
        revoked_at: None,
        created_at: "100".to_string(),
        updated_at: "100".to_string(),
    };
    let metadata = share_visitor_audit_metadata(
        &share,
        Uuid::new_v4(),
        "203.0.113.70",
        Some("visitor-browser"),
    );

    assert_eq!(metadata["remote_ip"], "203.0.113.70");
    assert!(metadata.get("source_ip").is_none());
}

#[test]
fn fresh_ping_results_aggregate() {
    let target = PingTargetRecord {
        id: Uuid::new_v4(),
        name: "Public resolver".to_string(),
        host: "1.1.1.1".to_string(),
        probe_kind: "icmp".to_string(),
        port: None,
        enabled: true,
        selector_expression: "*".to_string(),
        generation: 1,
        created_by: None,
        created_at: "100".to_string(),
        updated_at: "100".to_string(),
    };
    let result = PingTargetResult {
        target_id: target.id.to_string(),
        generation: 1,
        checked_unix: 120,
        status: "ok".to_string(),
        latency_avg_ms: Some(10.0),
        loss_ratio: 0.0,
        reason: None,
    };
    let mut rows = Vec::new();
    upsert_test_ping_rollup(&mut rows, "v-1", &target, &result, 120);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].sample_count, 1);

    let fresh = PingTargetResult {
        checked_unix: 150,
        latency_avg_ms: Some(20.0),
        ..result
    };
    upsert_test_ping_rollup(&mut rows, "v-1", &target, &fresh, 150);
    assert_eq!(rows[0].sample_count, 2);
    assert_eq!(rows[0].latency_avg_ms, Some(15.0));
    assert_eq!(rows[0].latest_checked_at, "150");
}

#[test]
fn ping_result_timestamp_must_belong_to_the_accepted_sample_window() {
    let result = PingTargetResult {
        target_id: Uuid::new_v4().to_string(),
        generation: 1,
        checked_unix: 1_000,
        status: "ok".to_string(),
        latency_avg_ms: Some(1.0),
        loss_ratio: 0.0,
        reason: None,
    };
    assert!(valid_ping_result(&result, 1_000));
    assert!(!valid_ping_result(&result, 5_000));
    assert!(!valid_ping_result(&result, 600));
}

#[test]
fn retained_ping_keeps_a_whole_coarse_bucket_without_fabricating_minutes() {
    let target_id = Uuid::new_v4();
    let row = |bucket_start: u64, bucket_secs: i32, sample_count: i32| PingRollupView {
        client_id: "v-1".to_string(),
        target_id,
        target_name: "Gateway".to_string(),
        is_primary: true,
        generation: 1,
        bucket_start: bucket_start.to_string(),
        bucket_secs,
        sample_count,
        success_count: sample_count,
        latency_avg_ms: Some(12.0),
        latency_min_ms: Some(12.0),
        latency_max_ms: Some(12.0),
        loss_ratio_avg: 0.0,
        loss_ratio_max: 0.0,
        latest_status: "ok".to_string(),
        latest_reason: None,
        latest_checked_at: (bucket_start
            + (bucket_secs.max(60) as u64 / 60).saturating_sub(1) * 60
            + 23)
            .to_string(),
        latest_source_checked_unix: bucket_start,
    };
    let retained = fragment_ping_rollup(row(0, 300, 5), Some(60), Some(180), 120);
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].bucket_start, "0");
    assert_eq!(retained[0].bucket_secs, 300);
    assert_eq!(retained[0].sample_count, 5);
    assert_eq!(retained[0].success_count, 5);

    let inclusive_end = fragment_ping_rollup(row(300, 60, 1), Some(60), Some(300), 120);
    assert_eq!(inclusive_end.len(), 1);
    assert_eq!(inclusive_end[0].bucket_start, "240");
}

#[test]
fn current_ping_smooths_one_partial_batch_without_hiding_latest_details() {
    let target = current_ping_test_target();
    let mut rows = (0..15)
        .map(|minute| {
            current_ping_test_rollup(
                &target,
                120 + minute * 60,
                if minute == 14 { 1.0 / 3.0 } else { 0.0 },
                if minute == 14 { "degraded" } else { "ok" },
            )
        })
        .collect::<Vec<_>>();
    rows[14].latency_avg_ms = Some(37.0);
    rows[14].latest_reason = Some("packet_loss".to_string());
    let rolling_loss = current_ping_loss_ratio(&rows[14], rows.iter()).unwrap();
    let current = current_ping_view(&target, Some(&rows[14]), Some(rolling_loss));

    assert!((current.loss_ratio.unwrap() - 1.0 / 45.0).abs() < 1e-12);
    assert_eq!(current.state, "ok");
    assert_eq!(current.status.as_deref(), Some("ok"));
    assert_eq!(current.latency_avg_ms, Some(37.0));
    assert_eq!(current.reason.as_deref(), Some("packet_loss"));
    assert_eq!(current.checked_at, Some(rows[14].latest_checked_at.clone()));
}

#[test]
fn current_ping_degrades_at_ten_percent_without_a_warmup_exception() {
    let target = current_ping_test_target();
    let rows = (0..10)
        .map(|minute| {
            current_ping_test_rollup(
                &target,
                120 + minute * 60,
                if minute < 3 { 1.0 / 3.0 } else { 0.0 },
                if minute < 3 { "degraded" } else { "ok" },
            )
        })
        .collect::<Vec<_>>();
    let loss = current_ping_loss_ratio(&rows[9], rows.iter()).unwrap();
    assert!((loss - 0.1).abs() < 1e-12);
    assert_eq!(current_ping_status("ok", Some(loss)), "degraded");

    let below_threshold_loss = current_ping_loss_ratio(&rows[9], rows[1..].iter()).unwrap();
    assert!(below_threshold_loss < 0.1);
    assert_eq!(current_ping_status("ok", Some(below_threshold_loss)), "ok");

    let lone_partial = current_ping_test_rollup(&target, 2_000, 1.0 / 3.0, "degraded");
    let lone_loss = current_ping_loss_ratio(&lone_partial, std::iter::once(&lone_partial));
    assert_eq!(current_ping_status("degraded", lone_loss), "degraded");
}

#[test]
fn current_ping_latest_hard_failure_overrides_a_healthy_rolling_window() {
    let target = current_ping_test_target();
    let mut rows = (0..15)
        .map(|minute| current_ping_test_rollup(&target, 120 + minute * 60, 0.0, "ok"))
        .collect::<Vec<_>>();
    rows[14].success_count = 0;
    rows[14].latency_avg_ms = None;
    rows[14].loss_ratio_avg = 1.0;
    rows[14].loss_ratio_max = 1.0;
    rows[14].latest_status = "down".to_string();
    rows[14].latest_reason = Some("timeout".to_string());
    let rolling_loss = current_ping_loss_ratio(&rows[14], rows.iter()).unwrap();
    let current = current_ping_view(&target, Some(&rows[14]), Some(rolling_loss));

    assert!((current.loss_ratio.unwrap() - 1.0 / 15.0).abs() < 1e-12);
    assert_eq!(current.state, "down");
    assert_eq!(current.status.as_deref(), Some("down"));
    assert_eq!(current.latency_avg_ms, None);
    assert_eq!(current.reason.as_deref(), Some("timeout"));
    assert_eq!(current_ping_status("error", Some(0.0)), "error");
}

#[test]
fn current_ping_window_uses_whole_authoritative_coarse_rows() {
    let target = current_ping_test_target();
    let mut coarse = current_ping_test_rollup(&target, 0, 1.0, "degraded");
    coarse.bucket_secs = 600;
    coarse.sample_count = 10;
    coarse.success_count = 10;
    coarse.latest_checked_at = "599".to_string();
    let outside = current_ping_test_rollup(&target, 300, 1.0, "degraded");
    let latest = current_ping_test_rollup(&target, 1_200, 0.0, "ok");
    let rows = [coarse, outside, latest];

    let authoritative = retain_authoritative_ping_rows(rows.to_vec());
    let latest = authoritative
        .iter()
        .max_by_key(|row| parse_timestamp_unix(&row.latest_checked_at))
        .unwrap();
    let loss = current_ping_loss_ratio(latest, authoritative.iter()).unwrap();
    assert!((loss - 10.0 / 11.0).abs() < 1e-12);
}

fn current_ping_test_target() -> PingTargetRecord {
    PingTargetRecord {
        id: Uuid::new_v4(),
        name: "Current Ping".to_string(),
        host: "192.0.2.1".to_string(),
        probe_kind: "icmp".to_string(),
        port: None,
        enabled: true,
        selector_expression: "*".to_string(),
        generation: 1,
        created_by: None,
        created_at: "100".to_string(),
        updated_at: "100".to_string(),
    }
}

fn current_ping_test_rollup(
    target: &PingTargetRecord,
    checked_unix: u64,
    loss_ratio: f64,
    status: &str,
) -> PingRollupView {
    PingRollupView {
        client_id: "v-1".to_string(),
        target_id: target.id,
        target_name: target.name.clone(),
        is_primary: true,
        generation: target.generation,
        bucket_start: (checked_unix / 60 * 60).to_string(),
        bucket_secs: 60,
        sample_count: 1,
        success_count: 1,
        latency_avg_ms: Some(12.0),
        latency_min_ms: Some(12.0),
        latency_max_ms: Some(12.0),
        loss_ratio_avg: loss_ratio,
        loss_ratio_max: loss_ratio,
        latest_status: status.to_string(),
        latest_reason: None,
        latest_checked_at: checked_unix.to_string(),
        latest_source_checked_unix: checked_unix,
    }
}
