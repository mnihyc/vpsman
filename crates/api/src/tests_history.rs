use axum::{extract::State, Json};
use serde_json::json;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{
        AuditLogView, AuthContext, ClientStatusHistoryView, GatewaySessionView, JobOutputView,
        ListQuery, OperatorView, ServerJobView, TelemetryNetworkRateView,
    },
    model_history::{
        HistoryDomain, HistoryRetentionPrunePlan, HistoryRetentionPruneRequest,
        UpsertHistoryRetentionPolicyRequest,
    },
    object_store::BackupObjectStore,
    repository::{MemoryState, Repository},
    routes_history::prune_history_retention,
    state::AppState,
    unix_now,
};
use vpsman_common::{
    SERVER_JOB_STATUS_FAILED, SERVER_JOB_STATUS_RUNNING, SERVER_JOB_TYPE_ARTIFACT_CLEANUP,
};

#[tokio::test]
async fn history_retention_policy_updates_and_prunes_memory_audit() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();

    let defaults = repo.list_history_retention_policies().await.unwrap();
    assert_eq!(defaults.len(), HistoryDomain::ALL.len());
    assert!(defaults.iter().any(|policy| {
        policy.domain == "backup_artifacts"
            && policy.retention_days == 3650
            && policy.built_in_default
    }));
    assert!(defaults.iter().any(|policy| {
        policy.domain == "telemetry_rollups"
            && policy.retention_days == 3650
            && policy.built_in_default
    }));

    let updated = repo
        .upsert_history_retention_policy(
            UpsertHistoryRetentionPolicyRequest {
                domain: "audit_logs".to_string(),
                retention_days: Some(1),
                prune_limit: Some(10),
                enabled: Some(true),
                metadata_only: Some(false),
                export_enabled: Some(true),
                notes: Some("daily audit trim".to_string()),
                clear_notes: false,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(updated.domain, "audit_logs");
    assert_eq!(updated.retention_days, 1);
    assert!(!updated.built_in_default);

    let old_audit = audit_with_created_at(
        "audit:old",
        unix_now().saturating_sub(3 * 86_400).to_string(),
    );
    let new_audit = audit_with_created_at("audit:new", unix_now().to_string());
    if let Repository::Memory(memory) = &repo {
        let mut audits = memory.audits.write().await;
        audits.clear();
        audits.push(old_audit);
        audits.push(new_audit);
    }

    let plan = HistoryRetentionPrunePlan {
        domain: HistoryDomain::AuditLogs,
        prune_limit: 10,
        enabled: true,
    };
    let cutoff = unix_now().saturating_sub(86_400);
    let dry_run = repo
        .prune_history_domain(&plan, cutoff, true)
        .await
        .unwrap();
    assert_eq!(dry_run.matched_rows, 1);
    assert_eq!(dry_run.pruned_rows, 0);
    assert_eq!(repo.list_audit_logs(10).await.unwrap().len(), 2);

    let pruned = repo
        .prune_history_domain(&plan, cutoff, false)
        .await
        .unwrap();
    assert_eq!(pruned.matched_rows, 1);
    assert_eq!(pruned.pruned_rows, 1);
    let remaining = repo.list_audit_logs(10).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].target, "audit:new");
}

#[tokio::test]
async fn audit_list_query_sorts_searches_and_offsets_memory_rows() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        let mut audits = memory.audits.write().await;
        audits.push(audit_with_created_at("target:zeta", "1".to_string()));
        audits.push(audit_with_created_at("target:alpha", "2".to_string()));
        audits.push(audit_with_created_at("target:beta", "3".to_string()));
    }

    let by_target = repo
        .query_audit_logs(&ListQuery {
            limit: Some(2),
            offset: None,
            sort: Some("target".to_string()),
            dir: Some("asc".to_string()),
            q: None,
        })
        .await
        .unwrap();
    assert_eq!(
        by_target
            .iter()
            .map(|audit| audit.target.as_str())
            .collect::<Vec<_>>(),
        vec!["target:alpha", "target:beta"]
    );

    let searched = repo
        .query_audit_logs(&ListQuery {
            limit: Some(10),
            offset: Some(0),
            sort: Some("created_at".to_string()),
            dir: Some("desc".to_string()),
            q: Some("zeta".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(searched.len(), 1);
    assert_eq!(searched[0].target, "target:zeta");

    let offset = repo
        .query_audit_logs(&ListQuery {
            limit: Some(1),
            offset: Some(1),
            sort: Some("created_at".to_string()),
            dir: Some("desc".to_string()),
            q: None,
        })
        .await
        .unwrap();
    assert_eq!(offset[0].target, "target:alpha");
}

#[tokio::test]
async fn history_retention_object_prune_partial_error_preserves_metadata_after_delete_failure() {
    let repo = Repository::Memory(MemoryState::default());
    let object_root = std::env::temp_dir().join(format!(
        "vpsman-api-history-prune-partial-{}",
        Uuid::new_v4()
    ));
    let store = BackupObjectStore::filesystem(object_root.clone()).unwrap();
    let state = test_state_with_store(repo.clone(), store.clone());
    let headers = crate::test_auth_headers(&state).await;
    let old_job = Uuid::new_v4();
    let failed_job = Uuid::new_v4();
    let retained_job = Uuid::new_v4();
    let missing_ok_key = "job-outputs/client-a/missing-ok.bin".to_string();
    let delete_fails_key = "job-outputs/client-a/delete-fails.bin".to_string();
    let retained_key = "job-outputs/client-a/retained.bin".to_string();
    store.put_new(&missing_ok_key, b"old").await.unwrap();
    store.put_new(&delete_fails_key, b"fail").await.unwrap();
    store.put_new(&retained_key, b"keep").await.unwrap();
    tokio::fs::remove_file(object_root.join(&missing_ok_key))
        .await
        .unwrap();
    tokio::fs::remove_file(object_root.join(&delete_fails_key))
        .await
        .unwrap();
    tokio::fs::create_dir(object_root.join(&delete_fails_key))
        .await
        .unwrap();
    if let Repository::Memory(memory) = &repo {
        let old_created_at = unix_now().saturating_sub(40 * 86_400).to_string();
        let mut outputs = memory.job_outputs.write().await;
        outputs.push(job_output(
            old_job,
            0,
            Some(missing_ok_key.clone()),
            &old_created_at,
        ));
        outputs.push(job_output(
            failed_job,
            1,
            Some(delete_fails_key.clone()),
            &old_created_at,
        ));
        outputs.push(job_output(
            retained_job,
            2,
            Some(retained_key.clone()),
            &unix_now().to_string(),
        ));
    }

    let Json(dry_run) = prune_history_retention(
        State(state.clone()),
        headers.clone(),
        Json(HistoryRetentionPruneRequest {
            domain: Some("job_outputs".to_string()),
            dry_run: true,
            metadata_only: Some(false),
            preview_hash: None,
            confirmed: false,
        }),
    )
    .await
    .unwrap();

    let Json(response) = prune_history_retention(
        State(state),
        headers,
        Json(HistoryRetentionPruneRequest {
            domain: Some("job_outputs".to_string()),
            dry_run: false,
            metadata_only: Some(false),
            preview_hash: Some(dry_run.preview_hash),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    let domain = &response.domains[0];
    assert_eq!(domain.domain, "job_outputs");
    assert_eq!(domain.status, "partial_error");
    assert_eq!(domain.matched_rows, 2);
    assert_eq!(domain.pruned_rows, 1);
    assert_eq!(
        domain.object_keys,
        vec![missing_ok_key, delete_fails_key.clone()]
    );
    assert_eq!(domain.object_delete_errors.len(), 1);
    assert!(domain.object_delete_errors[0].contains(&delete_fails_key));
    if let Repository::Memory(memory) = &repo {
        let outputs = memory.job_outputs.read().await;
        assert_eq!(outputs.len(), 2);
        assert!(outputs.iter().any(|output| output.job_id == retained_job));
        assert!(outputs.iter().any(|output| output.job_id == failed_job));
    }

    let _ = tokio::fs::remove_dir_all(object_root).await;
}

#[tokio::test]
async fn history_retention_rejects_unconfirmed_policy_update() {
    let repo = Repository::Memory(MemoryState::default());
    let error = repo
        .upsert_history_retention_policy(
            UpsertHistoryRetentionPolicyRequest {
                domain: "audit_logs".to_string(),
                retention_days: Some(7),
                prune_limit: None,
                enabled: None,
                metadata_only: None,
                export_enabled: None,
                notes: None,
                clear_notes: false,
                confirmed: false,
            },
            &test_operator(),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("history_retention_update_requires_confirmation"));
}

#[tokio::test]
async fn history_retention_prunes_network_rates_lifecycle_and_ended_gateway_sessions() {
    let repo = Repository::Memory(MemoryState::default());
    let old = unix_now().saturating_sub(400 * 86_400).to_string();
    let recent = unix_now().to_string();
    if let Repository::Memory(memory) = &repo {
        memory.telemetry_network_rates.write().await.extend([
            network_rate("edge-a", "eth0", &old),
            network_rate("edge-b", "eth0", &recent),
        ]);
        memory.client_status_history.write().await.extend([
            client_status_history("edge-a", &old),
            client_status_history("edge-b", &recent),
        ]);
        memory.gateway_sessions.write().await.extend([
            gateway_session("edge-a", "ended", &old, Some(old.clone())),
            gateway_session("edge-b", "active", &old, None),
            gateway_session("edge-c", "ended", &recent, Some(recent.clone())),
        ]);
    }
    let cutoff = unix_now().saturating_sub(365 * 86_400);
    for domain in [
        HistoryDomain::TelemetryNetworkRates,
        HistoryDomain::ClientStatusHistory,
        HistoryDomain::GatewaySessions,
    ] {
        let outcome = repo
            .prune_history_domain(
                &HistoryRetentionPrunePlan {
                    domain,
                    prune_limit: 10,
                    enabled: true,
                },
                cutoff,
                false,
            )
            .await
            .unwrap();
        assert_eq!(outcome.matched_rows, 1, "{domain:?}");
        assert_eq!(outcome.pruned_rows, 1, "{domain:?}");
    }
    if let Repository::Memory(memory) = &repo {
        assert_eq!(memory.telemetry_network_rates.read().await.len(), 1);
        assert_eq!(memory.client_status_history.read().await.len(), 1);
        let sessions = memory.gateway_sessions.read().await;
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|session| session.status == "active"));
    }
}

#[tokio::test]
async fn artifact_cleanup_running_timeout_marks_stale_jobs_failed_without_reclaim() {
    let repo = Repository::Memory(MemoryState::default());
    let old = unix_now().saturating_sub(7 * 60 * 60).to_string();
    let recent = unix_now().to_string();
    let old_job_id = Uuid::new_v4();
    if let Repository::Memory(memory) = &repo {
        memory.server_jobs.write().await.extend([
            server_job(old_job_id, SERVER_JOB_STATUS_RUNNING, Some(old)),
            server_job(Uuid::new_v4(), SERVER_JOB_STATUS_RUNNING, Some(recent)),
        ]);
    }
    let expired = repo
        .expire_stale_running_artifact_cleanup_jobs()
        .await
        .unwrap();
    assert_eq!(expired, 1);
    let jobs = repo.list_server_jobs(10).await.unwrap();
    let stale = jobs.iter().find(|job| job.id == old_job_id).unwrap();
    assert_eq!(stale.status, SERVER_JOB_STATUS_FAILED);
    assert_eq!(
        stale.error.as_deref(),
        Some("artifact_cleanup_running_timeout")
    );
    assert_eq!(
        jobs.iter()
            .filter(|job| job.status == SERVER_JOB_STATUS_RUNNING)
            .count(),
        1
    );
}

fn audit_with_created_at(target: &str, created_at: String) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: None,
        action: "test.audit".to_string(),
        target: target.to_string(),
        command_hash: None,
        metadata: json!({}),
        created_at,
    }
}

fn network_rate(client_id: &str, interface: &str, bucket_start: &str) -> TelemetryNetworkRateView {
    TelemetryNetworkRateView {
        client_id: client_id.to_string(),
        interface: interface.to_string(),
        bucket_start: bucket_start.to_string(),
        bucket_secs: 60,
        sample_count: 1,
        rx_bytes_avg: 1,
        tx_bytes_avg: 1,
        rx_bytes_delta: 1,
        tx_bytes_delta: 1,
        rx_bps_avg: 1.0,
        tx_bps_avg: 1.0,
        updated_at: bucket_start.to_string(),
    }
}

fn client_status_history(client_id: &str, created_at: &str) -> ClientStatusHistoryView {
    ClientStatusHistoryView {
        id: Uuid::new_v4(),
        client_id: client_id.to_string(),
        from_status: Some("online".to_string()),
        to_status: "offline".to_string(),
        reason: "test".to_string(),
        metadata: json!({}),
        created_at: created_at.to_string(),
    }
}

fn gateway_session(
    client_id: &str,
    status: &str,
    last_seen_at: &str,
    ended_at: Option<String>,
) -> GatewaySessionView {
    GatewaySessionView {
        id: Uuid::new_v4(),
        gateway_id: "gateway-a".to_string(),
        client_id: client_id.to_string(),
        status: status.to_string(),
        noise_public_key_hex: None,
        started_at: last_seen_at.to_string(),
        last_seen_at: last_seen_at.to_string(),
        ended_at,
        end_reason: None,
    }
}

fn server_job(id: Uuid, status: &str, started_at: Option<String>) -> ServerJobView {
    ServerJobView {
        id,
        job_type: SERVER_JOB_TYPE_ARTIFACT_CLEANUP.to_string(),
        status: status.to_string(),
        expression: Some("artifact.domain = \"job_output\"".to_string()),
        preview_hash: Some("a".repeat(64)),
        matched_count: 1,
        matched_bytes: 1,
        deleted_count: 0,
        deleted_bytes: 0,
        error: None,
        created_by: None,
        metadata: json!({}),
        created_at: unix_now().to_string(),
        started_at,
        completed_at: None,
        canceled_at: None,
    }
}

fn job_output(
    job_id: Uuid,
    seq: i32,
    object_key: Option<String>,
    created_at: &str,
) -> JobOutputView {
    JobOutputView {
        job_id,
        client_id: "client-a".to_string(),
        seq,
        stream: "stdout".to_string(),
        data_base64: String::new(),
        storage: if object_key.is_some() {
            "object".to_string()
        } else {
            "inline".to_string()
        },
        artifact_object_key: object_key,
        artifact_sha256_hex: None,
        artifact_size_bytes: None,
        exit_code: None,
        done: false,
        received_at: None,
        created_at: created_at.to_string(),
    }
}

fn test_state_with_store(repo: Repository, store: BackupObjectStore) -> AppState {
    let (events, _) = broadcast::channel(1);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::test_privilege_auto_approve(),
        backup_object_store: Some(store),
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}

fn test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Uuid::new_v4(),
    }
}
