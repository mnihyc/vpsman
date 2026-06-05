use serde_json::json;
use uuid::Uuid;

use crate::{
    model::{AuditLogView, AuthContext, ListQuery, OperatorView},
    model_history::{
        HistoryDomain, HistoryRetentionPrunePlan, UpsertHistoryRetentionPolicyRequest,
    },
    repository::{MemoryState, Repository},
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

fn test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
        },
        session_id: Uuid::new_v4(),
    }
}
