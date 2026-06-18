use anyhow::Result;
use serde_json::json;
use uuid::Uuid;
use vpsman_common::AgentUpdateHeartbeat;

use crate::{model::AuditLogView, repository::Repository};

impl Repository {
    pub(crate) async fn record_agent_update_rollback_completed(
        &self,
        client_id: &str,
        rollback_job_id: Uuid,
        rollback_sha256_hex: Option<&str>,
    ) -> Result<()> {
        let metadata = json!({
            "rollback_job_id": rollback_job_id,
            "client_id": client_id,
            "rollback_sha256_hex": rollback_sha256_hex.map(str::to_ascii_lowercase),
            "status": "rolled_back",
        });
        self.record_agent_update_lifecycle_audit(
            "agent_update.rollback_completed",
            client_id,
            metadata,
            None,
        )
        .await
    }

    pub(crate) async fn record_agent_update_rollback_failed(
        &self,
        client_id: &str,
        rollback_job_id: Uuid,
        rollback_sha256_hex: Option<&str>,
        outcome_status: &str,
        exit_code: Option<i32>,
        message: &str,
    ) -> Result<()> {
        let metadata = json!({
            "rollback_job_id": rollback_job_id,
            "client_id": client_id,
            "rollback_sha256_hex": rollback_sha256_hex.map(str::to_ascii_lowercase),
            "rollback_outcome_status": outcome_status,
            "exit_code": exit_code,
            "message": message,
            "status": "rollback_failed",
        });
        self.record_agent_update_lifecycle_audit(
            "agent_update.rollback_failed",
            client_id,
            metadata,
            None,
        )
        .await
    }

    pub(crate) async fn record_agent_update_activation_completed(
        &self,
        client_id: &str,
        activation_job_id: Uuid,
        staged_sha256_hex: &str,
    ) -> Result<()> {
        let metadata = json!({
            "activation_job_id": activation_job_id,
            "client_id": client_id,
            "artifact_sha256_hex": staged_sha256_hex.to_ascii_lowercase(),
            "status": "activation_completed",
        });
        self.record_agent_update_lifecycle_audit(
            "agent_update.activation_completed",
            client_id,
            metadata,
            None,
        )
        .await
    }

    pub(crate) async fn record_agent_update_activation_failed(
        &self,
        client_id: &str,
        activation_job_id: Uuid,
        staged_sha256_hex: &str,
        outcome_status: &str,
        exit_code: Option<i32>,
        message: &str,
    ) -> Result<()> {
        let metadata = json!({
            "activation_job_id": activation_job_id,
            "client_id": client_id,
            "artifact_sha256_hex": staged_sha256_hex.to_ascii_lowercase(),
            "activation_outcome_status": outcome_status,
            "exit_code": exit_code,
            "message": message,
            "status": "activation_failed",
            "rollback_recommended": true,
        });
        self.record_agent_update_lifecycle_audit(
            "agent_update.activation_failed",
            client_id,
            metadata,
            None,
        )
        .await
    }

    pub(crate) async fn record_agent_update_heartbeat(
        &self,
        client_id: &str,
        heartbeat: &AgentUpdateHeartbeat,
    ) -> Result<()> {
        let metadata = json!({
            "client_id": client_id,
            "activation_job_id": heartbeat.activation_job_id,
            "artifact_sha256_hex": heartbeat.sha256_hex.to_ascii_lowercase(),
            "marker_unix": heartbeat.marker_unix,
            "observed_unix": heartbeat.observed_unix,
            "heartbeat": "post_restart_activation_marker",
            "status": "heartbeat_observed",
        });
        self.record_agent_update_lifecycle_audit(
            "agent_update.heartbeat_observed",
            client_id,
            metadata,
            Some(heartbeat.observed_unix.to_string()),
        )
        .await
    }

    async fn record_agent_update_lifecycle_audit(
        &self,
        action: &str,
        client_id: &str,
        metadata: serde_json::Value,
        created_at_override: Option<String>,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: None,
                    action: action.to_string(),
                    target: format!("client:{client_id}"),
                    command_hash: None,
                    metadata,
                    created_at: created_at_override
                        .unwrap_or_else(|| crate::unix_now().to_string()),
                });
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, $2, $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(action)
                .bind(format!("client:{client_id}"))
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}
