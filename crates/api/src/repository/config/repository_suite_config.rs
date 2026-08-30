use anyhow::Result;
use serde_json::json;
use uuid::Uuid;

use crate::{model::AuthContext, repository::Repository};

impl Repository {
    pub(crate) async fn record_suite_config_update_requested(
        &self,
        operator: &AuthContext,
        path: &str,
        changed_keys: &[String],
        old_config: serde_json::Value,
        new_config: serde_json::Value,
        request_id: Uuid,
        payload_hash: &str,
    ) -> Result<()> {
        self.record_suite_config_audit_event(
            operator,
            "suite_config.update_requested",
            path,
            changed_keys,
            old_config,
            new_config,
            request_id,
            payload_hash,
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
        payload_hash: &str,
    ) -> Result<()> {
        self.record_suite_config_audit_event(
            operator,
            "suite_config.updated",
            path,
            changed_keys,
            old_config,
            new_config,
            request_id,
            payload_hash,
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
        payload_hash: &str,
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
            payload_hash,
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
        payload_hash: &str,
        write_error: Option<&str>,
    ) -> Result<()> {
        let result = match action {
            "suite_config.update_requested" => "requested",
            "suite_config.updated" => "succeeded",
            "suite_config.update_failed" => "failed",
            _ => anyhow::bail!("unsupported_suite_config_audit_action"),
        };
        let mut metadata = json!({
            "path": path,
            "changed_keys": changed_keys,
            "old": old_config,
            "new": new_config,
            "request_id": request_id,
            "result": result,
            "operator_id": operator.operator.id,
            "operator_username": operator.operator.username,
            "operator_role": operator.operator.role,
            "operator_session_id": operator.audit_session_id(),
            "origin_kind": "operator_request",
            "component": "suite-config-controller",
            "rollback_available": false,
        });
        if let Some(write_error) = write_error {
            metadata["write_error"] = json!(write_error);
        }
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, 'suite_config', $4, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(action)
                .bind(payload_hash)
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}
