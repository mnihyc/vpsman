use super::*;

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
            resources: true,
            network: true,
            traffic: true,
            ping: true,
            detail_history: false,
        },
        expires_at: "200".to_string(),
        revoked_at: None,
        revoked_by: None,
        created_by: None,
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
fn cached_ping_results_are_counted_once_and_fresh_checks_aggregate() {
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
    upsert_memory_ping_rollup(&mut rows, "v-1", &target, &result);
    upsert_memory_ping_rollup(&mut rows, "v-1", &target, &result);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].sample_count, 1);

    let fresh = PingTargetResult {
        checked_unix: 150,
        latency_avg_ms: Some(20.0),
        ..result
    };
    upsert_memory_ping_rollup(&mut rows, "v-1", &target, &fresh);
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
fn adaptive_ping_fragmentation_matches_uncompacted_minutes() {
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
    };
    let uncompacted = (0..5)
        .flat_map(|minute| {
            fragment_ping_rollup(row(120 + minute * 60, 60, 1), Some(180), Some(360), 120)
        })
        .collect::<Vec<_>>();
    let compacted = fragment_ping_rollup(row(120, 300, 5), Some(180), Some(360), 120);
    let summarize = |rows: Vec<PingRollupView>| {
        aggregate_memory_ping_rollups(rows, 120)
            .into_iter()
            .map(|row| {
                (
                    row.bucket_start,
                    row.sample_count,
                    row.success_count,
                    row.latency_avg_ms.map(f64::to_bits),
                    row.latest_checked_at,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(summarize(uncompacted), summarize(compacted));
}
