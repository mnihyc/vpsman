use anyhow::{ensure, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::AgentUpdateHeartbeat;

use crate::model::AuditLogView;

pub(crate) async fn record_agent_update_heartbeat_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    heartbeat: &AgentUpdateHeartbeat,
) -> Result<()> {
    let audit = agent_update_heartbeat_audit(client_id, heartbeat);
    let inserted = sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, NULL, $2, $3, NULL, $4)
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(audit.id)
    .bind(&audit.action)
    .bind(&audit.target)
    .bind(&audit.metadata)
    .execute(&mut **tx)
    .await?;
    if inserted.rows_affected() == 0 {
        let existing = sqlx::query(
            r#"
            SELECT actor_id, action, target, command_hash, metadata
            FROM audit_logs
            WHERE id = $1
            "#,
        )
        .bind(audit.id)
        .fetch_one(&mut **tx)
        .await?;
        let existing = AuditLogView {
            id: audit.id,
            actor_id: existing.try_get("actor_id")?,
            action: existing.try_get("action")?,
            target: existing.try_get("target")?,
            command_hash: existing.try_get("command_hash")?,
            metadata: existing.try_get("metadata")?,
            created_at: audit.created_at.clone(),
        };
        ensure!(
            agent_update_heartbeat_audits_match(&existing, &audit),
            "agent_update_heartbeat_identity_conflict"
        );
    }
    Ok(())
}

fn agent_update_heartbeat_audit(client_id: &str, heartbeat: &AgentUpdateHeartbeat) -> AuditLogView {
    AuditLogView {
        id: agent_update_heartbeat_audit_id(client_id, heartbeat),
        actor_id: None,
        action: "agent_update.heartbeat_observed".to_string(),
        target: format!("client:{client_id}"),
        command_hash: None,
        metadata: json!({
            "client_id": client_id,
            "activation_job_id": heartbeat.activation_job_id,
            "artifact_sha256_hex": heartbeat.sha256_hex.to_ascii_lowercase(),
            "marker_unix": heartbeat.marker_unix,
            "observed_unix": heartbeat.observed_unix,
            "heartbeat": "post_restart_activation_marker",
            "status": "heartbeat_observed",
            "result": "succeeded",
            "origin_kind": "gateway_ingest",
            "component": "agent-update-lifecycle",
        }),
        created_at: heartbeat.observed_unix.to_string(),
    }
}

fn agent_update_heartbeat_audits_match(left: &AuditLogView, right: &AuditLogView) -> bool {
    left.actor_id == right.actor_id
        && left.action == right.action
        && left.target == right.target
        && left.command_hash == right.command_hash
        && left.metadata == right.metadata
}

fn agent_update_heartbeat_audit_id(client_id: &str, heartbeat: &AgentUpdateHeartbeat) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(b"vpsman:agent-update-heartbeat:v1\0");
    digest.update(client_id.as_bytes());
    digest.update(b"\0");
    digest.update(heartbeat.activation_job_id.as_bytes());
    digest.update(heartbeat.observed_unix.to_be_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}
