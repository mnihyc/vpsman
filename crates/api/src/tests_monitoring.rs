use axum::{extract::State, Json};
use tokio::sync::broadcast;
use vpsman_common::{AgentPingProbeKind, AgentPingTarget, AgentRuntimeConfig, PingTargetResult};

use super::*;
use crate::repository_monitoring::monitoring_share_status;

#[tokio::test]
async fn ping_target_list_reports_durable_runtime_state_and_frozen_target_drift() {
    let repo = Repository::Memory(MemoryState::default());
    let state = monitoring_test_state(repo.clone());
    let (operator, headers) = crate::test_auth_context_and_headers(&state).await;
    seed_monitoring_agent(&repo, "v-1").await;
    let saved = save_ping_target(&repo, &operator, "gateway", true, &["v-1"], 3).await;

    let mut applied_config = AgentRuntimeConfig::default();
    applied_config.network.ping_targets.push(AgentPingTarget {
        id: saved.target.id.to_string(),
        generation: 3,
        name: "gateway".to_string(),
        host: "1.1.1.1".to_string(),
        kind: AgentPingProbeKind::Icmp,
        port: None,
    });
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    memory
        .runtime_config_apply_states
        .write()
        .await
        .push(RuntimeConfigApplyStateRecord {
            client_id: "v-1".to_string(),
            applied_version: Some(1),
            applied_content_hash: Some("applied".to_string()),
            applied_config: Some(applied_config),
            applied_job_id: Some(Uuid::new_v4()),
            applied_at: Some(crate::unix_now().to_string()),
            pending_version: None,
            pending_content_hash: None,
            pending_config: None,
            pending_job_id: None,
            pending_reason: None,
            pending_status: None,
            pending_error: None,
            pending_updated_at: None,
            updated_at: crate::unix_now().to_string(),
        });

    let Json(current) =
        crate::routes_monitoring::list_ping_targets(State(state.clone()), headers.clone())
            .await
            .unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].runtime_sync.state, "applied");
    assert!(!current[0].target_update_available);

    seed_monitoring_agent(&repo, "v-2").await;
    let Json(drifted) = crate::routes_monitoring::list_ping_targets(State(state), headers)
        .await
        .unwrap();
    assert_eq!(drifted[0].runtime_sync.state, "applied");
    assert!(drifted[0].target_update_available);
}

#[tokio::test]
async fn ping_capacity_failures_leave_bulk_lifecycle_and_target_updates_unchanged() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = monitoring_test_operator();
    seed_monitoring_agent(&repo, "v-1").await;
    seed_monitoring_agent(&repo, "v-2").await;

    for index in 0..15 {
        save_ping_target(
            &repo,
            &operator,
            &format!("enabled-{index:02}"),
            true,
            &["v-1"],
            1,
        )
        .await;
    }
    let disabled_a = save_ping_target(&repo, &operator, "disabled-a", false, &["v-1"], 1).await;
    let disabled_b = save_ping_target(&repo, &operator, "disabled-b", false, &["v-1"], 1).await;

    let error = repo
        .mutate_ping_targets_bulk(
            &[disabled_a.target.id, disabled_b.target.id],
            "enable",
            &operator,
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("ping_targets_per_client_too_many:v-1"));
    let targets = repo.list_ping_targets().await.unwrap();
    assert!(!target(&targets, disabled_a.target.id).enabled);
    assert!(!target(&targets, disabled_b.target.id).enabled);
    assert_eq!(repo.ping_targets_for_client("v-1").await.unwrap().len(), 15);

    repo.mutate_ping_targets_bulk(&[disabled_a.target.id], "enable", &operator)
        .await
        .unwrap();
    assert_eq!(repo.ping_targets_for_client("v-1").await.unwrap().len(), 16);
    assert!(
        !target(
            &repo.list_ping_targets().await.unwrap(),
            disabled_b.target.id
        )
        .enabled
    );

    let v2_target = save_ping_target(&repo, &operator, "v2-primary", true, &["v-2"], 1).await;
    repo.make_primary_ping_target(v2_target.target.id, &["v-2".to_string()], &operator)
        .await
        .unwrap();
    let error = repo
        .replace_ping_target_assignments_bulk(
            &[PingTargetAssignmentReplacement {
                expected_target: repo
                    .ping_target_record(v2_target.target.id)
                    .await
                    .unwrap()
                    .unwrap(),
                expected_client_ids: vec!["v-2".to_string()],
                next_client_ids: vec!["v-1".to_string()],
            }],
            &operator,
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("ping_targets_per_client_too_many:v-1"));
    let unchanged = repo
        .list_ping_target_assignment_records(Some(v2_target.target.id))
        .await
        .unwrap();
    assert_eq!(unchanged.len(), 1);
    assert_eq!(unchanged[0].client_id, "v-2");
    assert!(unchanged[0].is_primary);
}

#[tokio::test]
async fn disabled_primary_stays_explicit_and_delete_never_selects_a_replacement() {
    let repo = Repository::Memory(MemoryState::default());
    seed_monitoring_agent(&repo, "v-1").await;
    let state = monitoring_test_state(repo.clone());
    let (operator, headers) = crate::test_auth_context_and_headers(&state).await;
    let primary = save_ping_target(&repo, &operator, "primary", true, &["v-1"], 1).await;
    let fallback = save_ping_target(
        &repo,
        &operator,
        "not-automatic-fallback",
        true,
        &["v-1"],
        1,
    )
    .await;
    repo.make_primary_ping_target(primary.target.id, &["v-1".to_string()], &operator)
        .await
        .unwrap();
    record_ping(&repo, primary.target.id, 1, crate::unix_now(), 14.0).await;

    let missing = Uuid::new_v4();
    let error = repo
        .mutate_ping_targets_bulk(&[primary.target.id, missing], "delete", &operator)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "ping_target_not_found");
    assert!(repo
        .ping_target_record(primary.target.id)
        .await
        .unwrap()
        .is_some());

    let unconfirmed = crate::routes_monitoring::bulk_ping_target_lifecycle(
        State(state.clone()),
        headers.clone(),
        Json(BulkPingTargetLifecycleRequest {
            target_ids: vec![primary.target.id],
            action: "disable".to_string(),
            confirmed: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        unconfirmed.code,
        "ping_target_lifecycle_confirmation_required"
    );

    let Json(disabled) = crate::routes_monitoring::bulk_ping_target_lifecycle(
        State(state.clone()),
        headers.clone(),
        Json(BulkPingTargetLifecycleRequest {
            target_ids: vec![primary.target.id],
            action: "disable".to_string(),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(disabled.action, "disable");
    let assignment = repo
        .list_ping_target_assignment_records(Some(primary.target.id))
        .await
        .unwrap();
    assert_eq!(assignment.len(), 1);
    assert!(assignment[0].is_primary);
    let current = repo
        .current_primary_ping_for_clients(&["v-1".to_string()])
        .await
        .unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].1.target_id, primary.target.id);
    assert!(!current[0].1.enabled);
    assert_eq!(current[0].1.state, "disabled");
    let agent_targets = repo.ping_targets_for_client("v-1").await.unwrap();
    assert_eq!(agent_targets.len(), 1);
    assert_eq!(agent_targets[0].id, fallback.target.id.to_string());

    let Json(deleted) = crate::routes_monitoring::bulk_ping_target_lifecycle(
        State(state),
        headers,
        Json(BulkPingTargetLifecycleRequest {
            target_ids: vec![primary.target.id],
            action: "delete".to_string(),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(deleted.action, "delete");
    assert!(repo
        .current_primary_ping_for_clients(&["v-1".to_string()])
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        repo.get_ping_target_detail(fallback.target.id)
            .await
            .unwrap()
            .unwrap()
            .target
            .primary_count,
        0
    );
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    assert!(memory
        .telemetry_ping_rollups
        .read()
        .await
        .iter()
        .all(|row| row.target_id != primary.target.id));
}

#[tokio::test]
async fn update_targets_preview_is_read_only_stale_guarded_and_exactly_applied() {
    let repo = Repository::Memory(MemoryState::default());
    seed_monitoring_agent(&repo, "v-1").await;
    let state = monitoring_test_state(repo.clone());
    let (operator, headers) = crate::test_auth_context_and_headers(&state).await;
    let saved = save_ping_target(&repo, &operator, "all-vps", true, &["v-1"], 1).await;
    seed_monitoring_agent(&repo, "v-2").await;

    let Json(preview) = crate::routes_monitoring::bulk_update_ping_targets(
        State(state.clone()),
        headers.clone(),
        Json(BulkUpdatePingTargetsRequest {
            target_ids: vec![saved.target.id],
            preview_hash: None,
            confirmed: false,
        }),
    )
    .await
    .unwrap();
    assert!(!preview.applied);
    assert_eq!(preview.changes.len(), 1);
    assert_eq!(preview.changes[0].added_client_ids, vec!["v-2"]);
    assert_eq!(assigned_clients(&repo, saved.target.id).await, vec!["v-1"]);

    seed_monitoring_agent(&repo, "v-3").await;
    let stale = crate::routes_monitoring::bulk_update_ping_targets(
        State(state.clone()),
        headers.clone(),
        Json(BulkUpdatePingTargetsRequest {
            target_ids: vec![saved.target.id],
            preview_hash: Some(preview.preview_hash),
            confirmed: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.code, "ping_target_preview_stale");
    assert_eq!(assigned_clients(&repo, saved.target.id).await, vec!["v-1"]);

    let Json(fresh) = crate::routes_monitoring::bulk_update_ping_targets(
        State(state.clone()),
        headers.clone(),
        Json(BulkUpdatePingTargetsRequest {
            target_ids: vec![saved.target.id],
            preview_hash: None,
            confirmed: false,
        }),
    )
    .await
    .unwrap();
    assert_eq!(fresh.changes[0].added_client_ids, vec!["v-2", "v-3"]);
    let Json(applied) = crate::routes_monitoring::bulk_update_ping_targets(
        State(state),
        headers,
        Json(BulkUpdatePingTargetsRequest {
            target_ids: vec![saved.target.id],
            preview_hash: Some(fresh.preview_hash),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert!(applied.applied);
    assert_eq!(
        assigned_clients(&repo, saved.target.id).await,
        vec!["v-1", "v-2", "v-3"]
    );

    let stale_snapshot = repo
        .replace_ping_target_assignments_bulk(
            &[PingTargetAssignmentReplacement {
                expected_target: repo
                    .ping_target_record(saved.target.id)
                    .await
                    .unwrap()
                    .unwrap(),
                expected_client_ids: vec!["v-1".to_string()],
                next_client_ids: vec!["v-1".to_string()],
            }],
            &operator,
        )
        .await
        .unwrap_err();
    assert_eq!(stale_snapshot.to_string(), "ping_target_preview_stale");
    assert_eq!(
        assigned_clients(&repo, saved.target.id).await,
        vec!["v-1", "v-2", "v-3"]
    );
}

#[tokio::test]
async fn a_deleted_ping_target_is_not_recreated_by_a_stale_update() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = monitoring_test_operator();
    seed_monitoring_agent(&repo, "v-1").await;
    let saved = save_ping_target(&repo, &operator, "concurrent", true, &["v-1"], 1).await;
    let stale_record = repo
        .ping_target_record(saved.target.id)
        .await
        .unwrap()
        .unwrap();

    repo.delete_ping_target(saved.target.id, &operator)
        .await
        .unwrap();
    let error = repo
        .upsert_ping_target(
            stale_record.clone(),
            &["v-1".to_string()],
            Some(&PingTargetAssignmentReplacement {
                expected_target: stale_record,
                expected_client_ids: vec!["v-1".to_string()],
                next_client_ids: vec!["v-1".to_string()],
            }),
            &operator,
            "ping_target.updated",
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "ping_target_not_found");
    assert!(repo
        .ping_target_record(saved.target.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_single_ping_edit_rejects_a_changed_assignment_snapshot() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = monitoring_test_operator();
    seed_monitoring_agent(&repo, "v-1").await;
    seed_monitoring_agent(&repo, "v-2").await;
    let saved = save_ping_target(&repo, &operator, "concurrent-edit", true, &["v-1"], 1).await;
    let expected_target = repo
        .ping_target_record(saved.target.id)
        .await
        .unwrap()
        .unwrap();
    repo.replace_ping_target_assignments_bulk(
        &[PingTargetAssignmentReplacement {
            expected_target: expected_target.clone(),
            expected_client_ids: vec!["v-1".to_string()],
            next_client_ids: vec!["v-1".to_string(), "v-2".to_string()],
        }],
        &operator,
    )
    .await
    .unwrap();

    let mut edited = expected_target.clone();
    edited.name = "must-not-overwrite".to_string();
    let error = repo
        .upsert_ping_target(
            edited,
            &["v-1".to_string()],
            Some(&PingTargetAssignmentReplacement {
                expected_target,
                expected_client_ids: vec!["v-1".to_string()],
                next_client_ids: vec!["v-1".to_string()],
            }),
            &operator,
            "ping_target.updated",
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "ping_target_update_stale");
    assert_eq!(
        assigned_clients(&repo, saved.target.id).await,
        vec!["v-1", "v-2"]
    );
}

#[tokio::test]
async fn shared_view_records_each_new_visitor_once_and_touches_active_access() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = monitoring_test_operator();
    let now = crate::unix_now();
    let share = MonitoringShareRecord {
        id: Uuid::new_v4(),
        name: "Public status".to_string(),
        token_digest: vpsman_common::payload_hash(b"share-secret"),
        selector_expression: "*".to_string(),
        target_client_ids: Vec::new(),
        visibility: MonitoringShareVisibilityView {
            identity_context: false,
            resources: true,
            network: true,
            traffic: true,
            ping: true,
            detail_history: true,
        },
        expires_at: now.saturating_add(3_600).to_string(),
        revoked_at: None,
        revoked_by: None,
        created_by: Some(operator.operator.id),
        created_at: now.to_string(),
        updated_at: now.to_string(),
    };
    repo.create_monitoring_share(share.clone(), &operator)
        .await
        .unwrap();
    let visitor_id = Uuid::new_v4();
    let first = repo
        .record_monitoring_share_visitor(
            &share,
            Some(visitor_id),
            "198.51.100.20",
            Some("browser-a"),
        )
        .await
        .unwrap();
    let repeated = repo
        .record_monitoring_share_visitor(
            &share,
            Some(visitor_id),
            "198.51.100.21",
            Some("browser-b"),
        )
        .await
        .unwrap();
    assert_eq!(first, (visitor_id, true));
    assert_eq!(repeated, (visitor_id, false));
    assert!(repo
        .touch_monitoring_share_visitor(share.id, visitor_id)
        .await
        .unwrap());
    assert!(!repo
        .touch_monitoring_share_visitor(share.id, Uuid::new_v4())
        .await
        .unwrap());

    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    assert_eq!(memory.monitoring_share_visitors.read().await.len(), 1);
    assert_eq!(
        memory
            .audits
            .read()
            .await
            .iter()
            .filter(|audit| audit.action == "monitoring_share.visitor_opened")
            .count(),
        1
    );
}

#[tokio::test]
async fn probe_changes_advance_generation_and_stale_results_never_cross_it() {
    let repo = Repository::Memory(MemoryState::default());
    seed_monitoring_agent(&repo, "v-1").await;
    let state = monitoring_test_state(repo.clone());
    let (operator, headers) = crate::test_auth_context_and_headers(&state).await;
    let saved = save_ping_target(&repo, &operator, "gateway", true, &["v-1"], 1).await;
    let observed = crate::unix_now();
    record_ping(&repo, saved.target.id, 1, observed, 10.0).await;

    let Json(metadata_only) = crate::routes_monitoring::update_ping_target(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(saved.target.id),
        Json(PingTargetMutationRequest {
            name: "gateway-renamed".to_string(),
            host: "1.1.1.1".to_string(),
            probe_kind: "icmp".to_string(),
            port: None,
            enabled: true,
            selector_expression: "*".to_string(),
            target_client_ids: vec!["v-1".to_string()],
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(metadata_only.target.target.generation, 1);

    repo.mutate_ping_targets_bulk(&[saved.target.id], "disable", &operator)
        .await
        .unwrap();
    assert_eq!(
        repo.ping_target_record(saved.target.id)
            .await
            .unwrap()
            .unwrap()
            .generation,
        2
    );
    repo.mutate_ping_targets_bulk(&[saved.target.id], "enable", &operator)
        .await
        .unwrap();
    assert_eq!(
        repo.ping_target_record(saved.target.id)
            .await
            .unwrap()
            .unwrap()
            .generation,
        3
    );

    let Json(probe_changed) = crate::routes_monitoring::update_ping_target(
        State(state),
        headers,
        axum::extract::Path(saved.target.id),
        Json(PingTargetMutationRequest {
            name: "gateway-renamed".to_string(),
            host: "1.0.0.1".to_string(),
            probe_kind: "icmp".to_string(),
            port: None,
            enabled: true,
            selector_expression: "*".to_string(),
            target_client_ids: vec!["v-1".to_string()],
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(probe_changed.target.target.generation, 4);

    record_ping(&repo, saved.target.id, 1, observed + 60, 99.0).await;
    record_ping(&repo, saved.target.id, 4, observed + 60, 12.0).await;
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    let rows = memory.telemetry_ping_rollups.read().await;
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|row| row.generation == 1 && row.latency_avg_ms == Some(10.0)));
    assert!(rows
        .iter()
        .any(|row| row.generation == 4 && row.latency_avg_ms == Some(12.0)));
    assert!(!rows.iter().any(|row| row.latency_avg_ms == Some(99.0)));
}

#[tokio::test]
async fn ping_history_contains_only_current_assignments() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = monitoring_test_operator();
    seed_monitoring_agent(&repo, "v-1").await;
    seed_monitoring_agent(&repo, "v-2").await;
    let saved = save_ping_target(&repo, &operator, "assignment-bound", true, &["v-1"], 1).await;
    let observed = crate::unix_now();
    record_ping(&repo, saved.target.id, 1, observed, 8.0).await;
    assert_eq!(
        repo.list_ping_rollups("v-1", None, None, 10, 60)
            .await
            .unwrap()
            .len(),
        1
    );

    repo.replace_ping_target_assignments_bulk(
        &[PingTargetAssignmentReplacement {
            expected_target: repo
                .ping_target_record(saved.target.id)
                .await
                .unwrap()
                .unwrap(),
            expected_client_ids: vec!["v-1".to_string()],
            next_client_ids: vec!["v-2".to_string()],
        }],
        &operator,
    )
    .await
    .unwrap();
    assert!(repo
        .list_ping_rollups("v-1", None, None, 10, 60)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn authoritative_traffic_history_preserves_counter_reset_gaps() {
    let repo = Repository::Memory(MemoryState::default());
    seed_monitoring_agent(&repo, "v-1").await;
    let now = (crate::unix_now() / 60) * 60;
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    memory
        .vps_rule_values
        .write()
        .await
        .push(crate::model_alert_policies::VpsRuleValueRecord {
            client_id: "v-1".to_string(),
            key: crate::model_alert_policies::VPS_RULE_KEY_TRAFFIC_SELECTORS.to_string(),
            value_raw: "eth0+rx".to_string(),
            value_json: serde_json::json!({"selectors": [{"interface": "eth0"}]}),
            parsed_display: "eth0".to_string(),
            state: "ok".to_string(),
            validation_errors: Vec::new(),
            source_kind: "test".to_string(),
            source_id: None,
            updated_by: None,
            updated_at: now.to_string(),
        });
    let samples = [
        (now - 180, 100, 200, 0, 0),
        (now - 120, 150, 240, 0, 0),
        (now - 60, 10, 280, 1, 0),
        (now, 30, 300, 1, 0),
    ]
    .into_iter()
    .map(
        |(observed_unix, rx_bytes, tx_bytes, rx_counter_epoch, tx_counter_epoch)| {
            crate::model_alert_policies::TrafficCounterSampleRecord {
                client_id: "v-1".to_string(),
                source_kind: "host".to_string(),
                interface: "eth0".to_string(),
                observed_at: observed_unix.to_string(),
                observed_unix: observed_unix as i64,
                rx_bytes,
                tx_bytes,
                rx_counter_epoch,
                tx_counter_epoch,
                sample_source: "test".to_string(),
            }
        },
    )
    .collect::<Vec<_>>();
    memory.traffic_counter_samples.write().await.extend(samples);

    let history = repo
        .list_traffic_history("v-1", now - 120, now, 60, false)
        .await
        .unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].rx_bytes, Some(50));
    assert_eq!(history[0].tx_bytes, Some(40));
    assert_eq!(history[1].bucket_start, (now - 60).to_string());
    assert_eq!(history[1].sample_count, 1);
    assert_eq!(history[1].reset_count, 1);
    assert_eq!(history[1].rx_bytes, None);
    assert_eq!(history[1].tx_bytes, Some(40));
    assert_eq!(history[1].total_bytes, None);
    assert_eq!(history[2].rx_bytes, Some(20));
    assert_eq!(history[2].tx_bytes, Some(20));

    for (observed_unix, rx_bytes, tx_bytes) in
        [(now - 60, 1_000_u64, 2_000_u64), (now, 1_025, 2_040)]
    {
        let metrics = vpsman_common::AgentMetrics {
            observed_unix,
            networks: vec![vpsman_common::NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes,
                tx_bytes,
            }],
            ..Default::default()
        };
        memory
            .telemetry_samples
            .write()
            .await
            .push(TelemetrySampleView {
                id: Uuid::new_v4(),
                client_id: "v-1".to_string(),
                observed_at: observed_unix.to_string(),
                cpu_load_1: 0.0,
                memory_total_bytes: 0,
                memory_available_bytes: 0,
                payload: serde_json::to_value(metrics).unwrap(),
            });
    }
    let raw = repo
        .list_traffic_history("v-1", now, now, 60, true)
        .await
        .unwrap();
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].rx_bytes, Some(25));
    assert_eq!(raw[0].tx_bytes, Some(40));
}

#[tokio::test]
async fn deleting_a_vps_removes_live_ping_but_preserves_frozen_share_scope() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = monitoring_test_operator();
    seed_monitoring_agent(&repo, "v-1").await;
    let target = save_ping_target(&repo, &operator, "cleanup", true, &["v-1"], 1).await;
    let removed_target =
        save_ping_target(&repo, &operator, "cleanup-delete", true, &["v-1"], 1).await;
    let now = crate::unix_now();
    let share = MonitoringShareRecord {
        id: Uuid::new_v4(),
        name: "cleanup".to_string(),
        token_digest: "digest".to_string(),
        selector_expression: "*".to_string(),
        target_client_ids: vec!["v-1".to_string()],
        visibility: MonitoringShareVisibilityView {
            identity_context: false,
            resources: true,
            network: true,
            traffic: true,
            ping: true,
            detail_history: true,
        },
        expires_at: now.saturating_add(3_600).to_string(),
        revoked_at: None,
        revoked_by: None,
        created_by: Some(operator.operator.id),
        created_at: now.to_string(),
        updated_at: now.to_string(),
    };
    repo.create_monitoring_share(share.clone(), &operator)
        .await
        .unwrap();
    repo.delete_agent("v-1", Some("retired"), &operator)
        .await
        .unwrap();
    assert!(repo
        .list_ping_target_assignment_records(Some(target.target.id))
        .await
        .unwrap()
        .is_empty());
    let Repository::Memory(memory) = &repo else {
        unreachable!("test repository is in-memory")
    };
    assert_eq!(
        memory
            .agents
            .read()
            .await
            .iter()
            .find(|agent| agent.id == "v-1")
            .unwrap()
            .status,
        "deleted"
    );
    assert!(memory
        .ping_target_assignments
        .read()
        .await
        .iter()
        .any(|assignment| {
            assignment.target_id == target.target.id && assignment.client_id == "v-1"
        }));
    assert!(repo
        .mutate_ping_targets_bulk(&[target.target.id], "disable", &operator)
        .await
        .unwrap()
        .is_empty());
    assert!(repo
        .mutate_ping_targets_bulk(&[target.target.id], "enable", &operator)
        .await
        .unwrap()
        .is_empty());
    assert!(repo
        .delete_ping_target(removed_target.target.id, &operator)
        .await
        .unwrap()
        .is_empty());
    assert!(repo
        .ping_target_record(removed_target.target.id)
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .list_agents_for_client_ids(&["v-1".to_string()])
        .await
        .unwrap()
        .is_empty());
    let retained_share = repo
        .monitoring_share_record(share.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained_share.target_client_ids, vec!["v-1".to_string()]);
    let visible_target_count = repo
        .list_agents_for_client_ids(&retained_share.target_client_ids)
        .await
        .unwrap()
        .len();
    assert_eq!(
        crate::routes_monitoring::public_monitoring_share(&retained_share, visible_target_count,)
            .target_count,
        0,
    );

    let stale_target = PingTargetRecord {
        id: Uuid::new_v4(),
        name: "stale-resolution".to_string(),
        host: "1.1.1.1".to_string(),
        probe_kind: "icmp".to_string(),
        port: None,
        enabled: true,
        selector_expression: "*".to_string(),
        generation: 1,
        created_by: Some(operator.operator.id),
        created_at: now.to_string(),
        updated_at: now.to_string(),
    };
    let error = repo
        .upsert_ping_target(
            stale_target,
            &["v-1".to_string()],
            None,
            &operator,
            "ping_target.created",
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("ping_target_resolution_stale"));

    let error = repo
        .replace_ping_target_assignments_bulk(
            &[PingTargetAssignmentReplacement {
                expected_target: repo
                    .ping_target_record(target.target.id)
                    .await
                    .unwrap()
                    .unwrap(),
                expected_client_ids: Vec::new(),
                next_client_ids: vec!["v-1".to_string()],
            }],
            &operator,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("ping_target_resolution_stale"));

    let mut stale_share = share;
    stale_share.id = Uuid::new_v4();
    stale_share.name = "stale share resolution".to_string();
    stale_share.target_client_ids = vec!["v-1".to_string()];
    let error = repo
        .create_monitoring_share(stale_share, &operator)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("monitoring_share_resolution_stale"));
}

#[test]
fn monitoring_share_expiry_is_fail_closed_for_invalid_values() {
    let now = crate::unix_now();
    let mut share = MonitoringShareRecord {
        id: Uuid::new_v4(),
        name: "expiry".to_string(),
        token_digest: "digest".to_string(),
        selector_expression: "*".to_string(),
        target_client_ids: Vec::new(),
        visibility: MonitoringShareVisibilityView {
            identity_context: false,
            resources: true,
            network: true,
            traffic: true,
            ping: true,
            detail_history: true,
        },
        expires_at: "not-a-timestamp".to_string(),
        revoked_at: None,
        revoked_by: None,
        created_by: None,
        created_at: now.to_string(),
        updated_at: now.to_string(),
    };
    assert_eq!(monitoring_share_status(&share, now), "expired");
    share.expires_at = "2099-01-01 00:00:00+00".to_string();
    assert_eq!(monitoring_share_status(&share, now), "active");
}

async fn save_ping_target(
    repo: &Repository,
    operator: &AuthContext,
    name: &str,
    enabled: bool,
    client_ids: &[&str],
    generation: i64,
) -> PingTargetDetailView {
    let now = crate::unix_now().to_string();
    repo.upsert_ping_target(
        PingTargetRecord {
            id: Uuid::new_v4(),
            name: name.to_string(),
            host: "1.1.1.1".to_string(),
            probe_kind: "icmp".to_string(),
            port: None,
            enabled,
            selector_expression: "*".to_string(),
            generation,
            created_by: Some(operator.operator.id),
            created_at: now.clone(),
            updated_at: now,
        },
        &client_ids
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>(),
        None,
        operator,
        "ping_target.test_seeded",
    )
    .await
    .unwrap()
}

async fn record_ping(
    repo: &Repository,
    target_id: Uuid,
    generation: u64,
    checked_unix: u64,
    latency_ms: f64,
) {
    repo.record_ping_results_memory(
        "v-1",
        checked_unix,
        &[PingTargetResult {
            target_id: target_id.to_string(),
            generation,
            checked_unix,
            status: "ok".to_string(),
            latency_avg_ms: Some(latency_ms),
            loss_ratio: 0.0,
            reason: None,
        }],
    )
    .await
    .unwrap();
}

async fn assigned_clients(repo: &Repository, target_id: Uuid) -> Vec<String> {
    let mut clients = repo
        .list_ping_target_assignment_records(Some(target_id))
        .await
        .unwrap()
        .into_iter()
        .map(|assignment| assignment.client_id)
        .collect::<Vec<_>>();
    clients.sort();
    clients
}

fn target(targets: &[PingTargetView], target_id: Uuid) -> &PingTargetView {
    targets
        .iter()
        .find(|target| target.id == target_id)
        .expect("Ping target")
}

async fn seed_monitoring_agent(repo: &Repository, client_id: &str) {
    let Repository::Memory(memory) = repo else {
        unreachable!()
    };
    memory.agents.write().await.push(AgentView {
        id: client_id.to_string(),
        display_name: client_id.to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: Some(crate::unix_now().to_string()),
        arch: Some("x86_64".to_string()),
        internal_build_number: 1,
        process_incarnation_id: Some(Uuid::new_v4()),
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    });
}

fn monitoring_test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(16);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: crate::gateway_client::GatewayDispatchClient::test_privilege_auto_approve(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman-test-missing.toml"),
        dispatcher_config: Default::default(),
    }
}

fn monitoring_test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "monitoring-test".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Some(Uuid::new_v4()),
    }
}
