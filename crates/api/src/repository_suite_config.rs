use anyhow::Result;
use serde_json::json;
use uuid::Uuid;

use crate::{
    model::{AuditLogView, AuthContext},
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn record_suite_config_audit(
        &self,
        operator: &AuthContext,
        path: &str,
        changed_keys: &[String],
        old_config: serde_json::Value,
        new_config: serde_json::Value,
    ) -> Result<()> {
        let metadata = json!({
            "path": path,
            "changed_keys": changed_keys,
            "old": old_config,
            "new": new_config,
            "rollback_available": false,
        });
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "suite_config.updated".to_string(),
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
                    VALUES ($1, $2, 'suite_config.updated', 'suite_config', NULL, $3)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}
