use anyhow::Result;
use serde_json::json;
use uuid::Uuid;

use crate::{
    model::{AuditLogView, AuthContext},
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn record_suite_config_update_requested(
        &self,
        operator: &AuthContext,
        path: &str,
        changed_keys: &[String],
        old_config: serde_json::Value,
        new_config: serde_json::Value,
        request_id: Uuid,
    ) -> Result<()> {
        self.record_suite_config_audit_event(
            operator,
            "suite_config.update_requested",
            path,
            changed_keys,
            old_config,
            new_config,
            request_id,
            None,
        )
        .await
    }

    pub(crate) async fn record_suite_config_updated(
        &self,
        operator: &AuthContext,
        path: &str,
        changed_keys: &[String],
        old_config: serde_json::Value,
        new_config: serde_json::Value,
        request_id: Uuid,
    ) -> Result<()> {
        self.record_suite_config_audit_event(
            operator,
            "suite_config.updated",
            path,
            changed_keys,
            old_config,
            new_config,
            request_id,
            None,
        )
        .await
    }

    pub(crate) async fn record_suite_config_update_failed(
        &self,
        operator: &AuthContext,
        path: &str,
        changed_keys: &[String],
        old_config: serde_json::Value,
        new_config: serde_json::Value,
        request_id: Uuid,
        write_error: &str,
    ) -> Result<()> {
        self.record_suite_config_audit_event(
            operator,
            "suite_config.update_failed",
            path,
            changed_keys,
            old_config,
            new_config,
            request_id,
            Some(write_error),
        )
        .await
    }

    async fn record_suite_config_audit_event(
        &self,
        operator: &AuthContext,
        action: &str,
        path: &str,
        changed_keys: &[String],
        old_config: serde_json::Value,
        new_config: serde_json::Value,
        request_id: Uuid,
        write_error: Option<&str>,
    ) -> Result<()> {
        let mut metadata = json!({
            "path": path,
            "changed_keys": changed_keys,
            "old": old_config,
            "new": new_config,
            "request_id": request_id,
            "rollback_available": false,
        });
        if let Some(write_error) = write_error {
            metadata["write_error"] = json!(write_error);
        }
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: action.to_string(),
                    target: "suite_config".to_string(),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, 'suite_config', NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(action)
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
            session_id: Uuid::new_v4(),
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
            audits[1].metadata["write_error"],
            json!("suite_config_write_failed")
        );
    }
}
