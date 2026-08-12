use super::*;

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

#[tokio::test]
async fn monitoring_system_information_combines_session_facts_with_latest_uptime() {
    let repo = Repository::Memory(crate::repository::MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    memory.agents.write().await.push(crate::model::AgentView {
        id: "v-1".to_string(),
        display_name: "VPS 1".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: Some("200".to_string()),
        arch: Some("x86_64".to_string()),
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    });
    memory.client_system_facts.write().await.insert(
        "v-1".to_string(),
        crate::model::ClientSystemFactsRecord {
            os_release: "PRETTY_NAME=\"Debian GNU/Linux 12\"\nSECRET=value\n".to_string(),
            architecture: "x86_64".to_string(),
            cpu_model: Some("AMD EPYC".to_string()),
            kernel_release: Some("6.12.1".to_string()),
            virtualization: Some("kvm".to_string()),
            reported_at: "100".to_string(),
        },
    );
    memory
        .telemetry_samples
        .write()
        .await
        .push(crate::model::TelemetrySampleView {
            id: Uuid::from_u128(1),
            client_id: "v-1".to_string(),
            observed_at: "200".to_string(),
            cpu_load_1: 0.1,
            memory_total_bytes: 1,
            memory_available_bytes: 1,
            payload: serde_json::json!({"uptime_secs": 86400, "hostname": "private"}),
        });

    let views = repo
        .monitoring_system_information_for_clients(&["v-1".to_string()])
        .await
        .unwrap();
    let view = views.get("v-1").unwrap();
    assert_eq!(view.os_name.as_deref(), Some("Debian GNU/Linux 12"));
    assert_eq!(view.architecture.as_deref(), Some("x86_64"));
    assert_eq!(view.cpu_model.as_deref(), Some("AMD EPYC"));
    assert_eq!(view.kernel_release.as_deref(), Some("6.12.1"));
    assert_eq!(view.virtualization.as_deref(), Some("kvm"));
    assert_eq!(view.uptime_secs, Some(86_400));
    assert_eq!(view.uptime_observed_at.as_deref(), Some("200"));

    memory
        .telemetry_samples
        .write()
        .await
        .push(crate::model::TelemetrySampleView {
            id: Uuid::from_u128(2),
            client_id: "v-1".to_string(),
            observed_at: "200".to_string(),
            cpu_load_1: 0.1,
            memory_total_bytes: 1,
            memory_available_bytes: 1,
            payload: serde_json::json!({"uptime_secs": 25}),
        });
    let same_timestamp_view = repo
        .monitoring_system_information_for_clients(&["v-1".to_string()])
        .await
        .unwrap()
        .remove("v-1")
        .unwrap();
    assert_eq!(same_timestamp_view.uptime_secs, Some(25));
    assert_eq!(
        same_timestamp_view.uptime_observed_at.as_deref(),
        Some("200")
    );

    memory.agents.write().await[0].status = "revoked".to_string();
    assert!(repo
        .monitoring_system_information_for_clients(&["v-1".to_string()])
        .await
        .unwrap()
        .contains_key("v-1"));

    memory
        .telemetry_samples
        .write()
        .await
        .push(crate::model::TelemetrySampleView {
            id: Uuid::new_v4(),
            client_id: "v-1".to_string(),
            observed_at: "300".to_string(),
            cpu_load_1: 0.1,
            memory_total_bytes: 1,
            memory_available_bytes: 1,
            payload: serde_json::json!({"hostname": "private"}),
        });
    let view_without_uptime = repo
        .monitoring_system_information_for_clients(&["v-1".to_string()])
        .await
        .unwrap()
        .remove("v-1")
        .unwrap();
    assert_eq!(view_without_uptime.uptime_secs, None);
    assert_eq!(view_without_uptime.uptime_observed_at, None);

    memory
        .hidden_clients
        .write()
        .await
        .insert("v-1".to_string());
    assert!(repo
        .monitoring_system_information_for_clients(&["v-1".to_string()])
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn latest_telemetry_uptimes_use_only_each_visible_clients_newest_raw_sample() {
    let repo = Repository::Memory(crate::repository::MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    memory.agents.write().await.extend([
        uptime_test_agent("visible-reboot"),
        uptime_test_agent("newest-missing"),
        uptime_test_agent("same-timestamp"),
        uptime_test_agent("hidden-deleted"),
    ]);
    memory
        .hidden_clients
        .write()
        .await
        .insert("hidden-deleted".to_string());

    let sample = |id: u128, client_id: &str, observed_at: &str, payload: serde_json::Value| {
        crate::model::TelemetrySampleView {
            id: Uuid::from_u128(id),
            client_id: client_id.to_string(),
            observed_at: observed_at.to_string(),
            cpu_load_1: 0.1,
            memory_total_bytes: 1,
            memory_available_bytes: 1,
            payload,
        }
    };
    memory.telemetry_samples.write().await.extend([
        sample(
            1,
            "visible-reboot",
            "100",
            serde_json::json!({"uptime_secs": 50_000}),
        ),
        sample(
            2,
            "visible-reboot",
            "200",
            serde_json::json!({"uptime_secs": 25}),
        ),
        sample(
            3,
            "newest-missing",
            "100",
            serde_json::json!({"uptime_secs": 9_000}),
        ),
        sample(4, "newest-missing", "200", serde_json::json!({})),
        sample(
            5,
            "same-timestamp",
            "250",
            serde_json::json!({"uptime_secs": 900}),
        ),
        sample(
            6,
            "same-timestamp",
            "250",
            serde_json::json!({"uptime_secs": 12}),
        ),
        sample(
            7,
            "hidden-deleted",
            "300",
            serde_json::json!({"uptime_secs": 300}),
        ),
        sample(8, "orphan", "400", serde_json::json!({"uptime_secs": 400})),
    ]);

    let uptimes = repo.list_latest_telemetry_uptimes().await.unwrap();

    assert_eq!(uptimes.len(), 2);
    assert_eq!(uptimes[0].client_id, "same-timestamp");
    assert_eq!(uptimes[0].uptime_secs, 12);
    assert_eq!(uptimes[0].observed_at, "250");
    assert_eq!(uptimes[1].client_id, "visible-reboot");
    assert_eq!(uptimes[1].uptime_secs, 25);
    assert_eq!(uptimes[1].observed_at, "200");
}

fn uptime_test_agent(client_id: &str) -> crate::model::AgentView {
    crate::model::AgentView {
        id: client_id.to_string(),
        display_name: client_id.to_string(),
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
    }
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
    upsert_memory_ping_rollup(&mut rows, "v-1", &target, &result, 120);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].sample_count, 1);

    let fresh = PingTargetResult {
        checked_unix: 150,
        latency_avg_ms: Some(20.0),
        ..result
    };
    upsert_memory_ping_rollup(&mut rows, "v-1", &target, &fresh, 150);
    assert_eq!(rows[0].sample_count, 2);
    assert_eq!(rows[0].latency_avg_ms, Some(15.0));
    assert_eq!(rows[0].latest_checked_at, "150");
}

#[tokio::test]
async fn memory_ping_source_identity_counts_equal_chart_times_and_deduplicates_cached_checks() {
    let repo = Repository::Memory(crate::repository::MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    let target = current_ping_test_target();
    memory.ping_targets.write().await.push(target.clone());
    memory
        .ping_target_assignments
        .write()
        .await
        .push(PingTargetAssignmentRecord {
            target_id: target.id,
            client_id: "v-1".to_string(),
            is_primary: true,
            assigned_at: "100".to_string(),
        });
    let first = PingTargetResult {
        target_id: target.id.to_string(),
        generation: 1,
        checked_unix: 120,
        status: "ok".to_string(),
        latency_avg_ms: Some(10.0),
        loss_ratio: 0.0,
        reason: None,
    };
    let second = PingTargetResult {
        status: "degraded".to_string(),
        latency_avg_ms: Some(20.0),
        loss_ratio: 0.5,
        reason: Some("higher source identity".to_string()),
        ..first.clone()
    };

    repo.record_ping_results_memory("v-1", 120, &[first.clone(), second.clone()], &[200, 201])
        .await
        .unwrap();
    repo.record_ping_results_memory("v-1", 180, &[second], &[201])
        .await
        .unwrap();

    let rows = memory.telemetry_ping_rollups.read().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].sample_count, 2);
    assert_eq!(rows[0].success_count, 2);
    assert_eq!(rows[0].latency_avg_ms, Some(15.0));
    assert_eq!(rows[0].latest_status, "degraded");
    assert_eq!(
        rows[0].latest_reason.as_deref(),
        Some("higher source identity")
    );
    assert_eq!(rows[0].latest_checked_at, "120");
    assert_eq!(rows[0].latest_source_checked_unix, 201);
    assert_eq!(memory.telemetry_ping_source_checks.read().await.len(), 2);
    assert_eq!(
        memory.telemetry_ping_source_checks.read().await.get(&(
            "v-1".to_string(),
            target.id,
            1,
            201,
        )),
        Some(&180),
    );
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

#[tokio::test]
async fn memory_current_ping_isolates_the_active_generation_before_smoothing() {
    let repo = Repository::Memory(crate::repository::MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    let mut target = current_ping_test_target();
    target.generation = 2;
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
    let mut old_generation = current_ping_test_rollup(&target, 960, 1.0, "degraded");
    old_generation.generation = 1;
    rows.push(old_generation);
    memory.ping_targets.write().await.push(target.clone());
    memory
        .ping_target_assignments
        .write()
        .await
        .push(PingTargetAssignmentRecord {
            target_id: target.id,
            client_id: "v-1".to_string(),
            is_primary: true,
            assigned_at: "100".to_string(),
        });
    memory.telemetry_ping_rollups.write().await.extend(rows);

    let current = repo
        .current_primary_ping_for_clients(&["v-1".to_string()])
        .await
        .unwrap();
    assert_eq!(current.len(), 1);
    assert!((current[0].1.loss_ratio.unwrap() - 1.0 / 45.0).abs() < 1e-12);
    assert_eq!(current[0].1.state, "ok");
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
