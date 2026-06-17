use axum::{extract::State, Json};
use serde_json::json;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{AuditLogView, AuthContext, JobOutputView, ListQuery, OperatorView},
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
async fn history_retention_object_prune_partial_error_prunes_metadata_before_delete_failure() {
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

    let Json(response) = prune_history_retention(
        State(state),
        headers,
        Json(HistoryRetentionPruneRequest {
            domain: Some("job_outputs".to_string()),
            dry_run: false,
            metadata_only: Some(false),
            confirmed: true,
        }),
    )
    .await
    .unwrap();
    let domain = &response.domains[0];
    assert_eq!(domain.domain, "job_outputs");
    assert_eq!(domain.status, "partial_error");
    assert_eq!(domain.matched_rows, 2);
    assert_eq!(domain.pruned_rows, 2);
    assert_eq!(
        domain.object_keys,
        vec![missing_ok_key, delete_fails_key.clone()]
    );
    assert_eq!(domain.object_delete_errors.len(), 1);
    assert!(domain.object_delete_errors[0].contains(&delete_fails_key));
    if let Repository::Memory(memory) = &repo {
        let outputs = memory.job_outputs.read().await;
        assert_eq!(outputs.len(), 1);
        assert!(outputs.iter().any(|output| output.job_id == retained_job));
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
