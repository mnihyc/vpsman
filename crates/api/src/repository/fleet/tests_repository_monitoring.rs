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
