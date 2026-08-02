use serde_json::json;

use super::*;
use crate::{
    auth_model::{OperatorPreferences, OperatorView},
    repository::MemoryState,
};

fn test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "operator".to_string(),
            status: "active".to_string(),
            role: "admin".to_string(),
            scopes: vec![],
            preferences: OperatorPreferences::default(),
            totp_enabled: false,
            session_refresh_ttl_secs: 3600,
            created_at: "0".to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Some(Uuid::new_v4()),
    }
}

#[tokio::test]
async fn suite_config_audit_records_intent_and_failure_with_shared_request_id() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let request_id = Uuid::new_v4();
    let changed = vec!["database.postgres_url".to_string()];

    repo.record_suite_config_update_requested(
        &operator,
        "config/vpsman.toml",
        &changed,
        json!({"old": true}),
        json!({"new": true}),
        request_id,
        "suite-config-payload-hash",
    )
    .await
    .unwrap();
    repo.record_suite_config_update_failed(
        &operator,
        "config/vpsman.toml",
        &changed,
        json!({"old": true}),
        json!({"new": true}),
        request_id,
        "suite-config-payload-hash",
        "suite_config_write_failed",
    )
    .await
    .unwrap();

    let Repository::Memory(memory) = &repo else {
        unreachable!("test uses memory repo")
    };
    let audits = memory.audits.read().await;
    assert_eq!(audits.len(), 2);
    assert_eq!(audits[0].action, "suite_config.update_requested");
    assert_eq!(audits[1].action, "suite_config.update_failed");
    assert_eq!(audits[0].metadata["request_id"], json!(request_id));
    assert_eq!(audits[1].metadata["request_id"], json!(request_id));
    assert_eq!(
        audits[0].command_hash.as_deref(),
        Some("suite-config-payload-hash")
    );
    assert_eq!(
        audits[1].metadata["operator_session_id"],
        json!(operator.audit_session_id())
    );
    assert_eq!(
        audits[1].metadata["write_error"],
        json!("suite_config_write_failed")
    );
}
