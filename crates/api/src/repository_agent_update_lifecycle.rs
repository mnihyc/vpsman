use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::json;
use sqlx::Row;
use tracing::debug;
use uuid::Uuid;
use vpsman_common::AgentUpdateHeartbeat;

use crate::{
    model::{AgentUpdateRolloutTargetView, AuditLogView},
    repository::Repository,
    repository_rollouts::{
        rollout_status_for_activation_failed_targets, rollout_status_for_activation_targets,
        rollout_status_for_heartbeat_targets, rollout_status_for_rollback_targets,
        rollout_status_for_timeout_targets, rollout_target_summary,
    },
};

fn agent_update_heartbeat_metadata(
    rollout_id: Uuid,
    rollout_job_id: Uuid,
    client_id: &str,
    heartbeat: &AgentUpdateHeartbeat,
) -> serde_json::Value {
    json!({
        "rollout_id": rollout_id,
        "rollout_job_id": rollout_job_id,
        "client_id": client_id,
        "activation_job_id": heartbeat.activation_job_id,
        "artifact_sha256_hex": heartbeat.sha256_hex.to_ascii_lowercase(),
        "marker_unix": heartbeat.marker_unix,
        "observed_unix": heartbeat.observed_unix,
        "heartbeat": "post_restart_activation_marker",
    })
}

fn agent_update_activation_metadata(
    rollout_id: Uuid,
    rollout_job_id: Uuid,
    activation_job_id: Uuid,
    client_id: &str,
    staged_sha256_hex: &str,
) -> serde_json::Value {
    json!({
        "rollout_id": rollout_id,
        "rollout_job_id": rollout_job_id,
        "activation_job_id": activation_job_id,
        "client_id": client_id,
        "artifact_sha256_hex": staged_sha256_hex.to_ascii_lowercase(),
        "status": "activation_pending_restart",
    })
}

struct ActivationFailedMetadata<'a> {
    rollout_id: Uuid,
    rollout_job_id: Uuid,
    activation_job_id: Uuid,
    client_id: &'a str,
    staged_sha256_hex: &'a str,
    previous_status: &'a str,
    outcome_status: &'a str,
    exit_code: Option<i32>,
    message: &'a str,
}

fn agent_update_activation_failed_metadata(
    input: ActivationFailedMetadata<'_>,
) -> serde_json::Value {
    json!({
        "rollout_id": input.rollout_id,
        "rollout_job_id": input.rollout_job_id,
        "activation_job_id": input.activation_job_id,
        "client_id": input.client_id,
        "artifact_sha256_hex": input.staged_sha256_hex.to_ascii_lowercase(),
        "previous_status": input.previous_status,
        "activation_outcome_status": input.outcome_status,
        "exit_code": input.exit_code,
        "message": input.message,
        "status": "activation_failed",
        "rollback_recommended": true,
    })
}

fn agent_update_heartbeat_timeout_metadata(
    rollout_id: Uuid,
    rollout_job_id: Uuid,
    client_id: &str,
    artifact_sha256_hex: &str,
    heartbeat_timeout_secs: i32,
    target_updated_at: &str,
) -> serde_json::Value {
    json!({
        "rollout_id": rollout_id,
        "rollout_job_id": rollout_job_id,
        "client_id": client_id,
        "artifact_sha256_hex": artifact_sha256_hex.to_ascii_lowercase(),
        "heartbeat_timeout_secs": heartbeat_timeout_secs,
        "target_updated_at": target_updated_at,
        "status": "heartbeat_timeout",
    })
}

fn agent_update_rollback_metadata(
    rollout_id: Uuid,
    rollout_job_id: Uuid,
    rollback_job_id: Uuid,
    client_id: &str,
    rollback_sha256_hex: Option<&str>,
    previous_status: &str,
) -> serde_json::Value {
    json!({
        "rollout_id": rollout_id,
        "rollout_job_id": rollout_job_id,
        "rollback_job_id": rollback_job_id,
        "client_id": client_id,
        "rollback_sha256_hex": rollback_sha256_hex.map(str::to_ascii_lowercase),
        "previous_status": previous_status,
        "status": "rolled_back",
    })
}

fn memory_rollout_target_is_expired(
    target: &AgentUpdateRolloutTargetView,
    now_unix: u64,
    timeout_secs: i32,
) -> bool {
    if target.status != "activation_pending_restart" {
        return false;
    }
    let Ok(updated_unix) = target.updated_at.parse::<u64>() else {
        return false;
    };
    updated_unix.saturating_add(timeout_secs.max(1) as u64) <= now_unix
}

impl Repository {
    pub(crate) async fn record_agent_update_rollback_completed(
        &self,
        client_id: &str,
        rollback_job_id: Uuid,
        rollback_sha256_hex: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let now = crate::unix_now().to_string();
                let mut rollouts = memory.agent_update_rollouts.write().await;
                let Some(rollout) = rollouts.iter_mut().rev().find(|rollout| {
                    rollout.targets.iter().any(|target| {
                        target.client_id == client_id
                            && matches!(
                                target.status.as_str(),
                                "activation_pending_restart"
                                    | "heartbeat_timeout"
                                    | "activation_failed"
                                    | "heartbeat_verified"
                            )
                    })
                }) else {
                    return Ok(());
                };
                let Some(target) = rollout
                    .targets
                    .iter_mut()
                    .find(|target| target.client_id == client_id)
                else {
                    return Ok(());
                };
                if !matches!(
                    target.status.as_str(),
                    "activation_pending_restart"
                        | "heartbeat_timeout"
                        | "activation_failed"
                        | "heartbeat_verified"
                ) {
                    return Ok(());
                }
                let previous_status = target.status.clone();
                target.status = "rolled_back".to_string();
                target.exit_code = Some(0);
                target.updated_at = now.clone();
                let (completed_count, failed_count, pending_count) =
                    rollout_target_summary(&rollout.targets);
                rollout.completed_count = completed_count;
                rollout.failed_count = failed_count;
                rollout.pending_count = pending_count;
                rollout.status = rollout_status_for_rollback_targets(&rollout.targets).to_string();
                rollout.updated_at = now.clone();
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: None,
                    action: "agent_update.rollback_completed".to_string(),
                    target: format!("client:{client_id}"),
                    command_hash: None,
                    metadata: agent_update_rollback_metadata(
                        rollout.id,
                        rollout.job_id,
                        rollback_job_id,
                        client_id,
                        rollback_sha256_hex,
                        &previous_status,
                    ),
                    created_at: now,
                });
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT rollout.id, rollout.job_id, target.status AS previous_status
                    FROM agent_update_rollouts rollout
                    JOIN agent_update_rollout_targets target
                      ON target.rollout_id = rollout.id
                    WHERE target.client_id = $1
                      AND target.status IN (
                        'activation_pending_restart',
                        'heartbeat_timeout',
                        'activation_failed',
                        'heartbeat_verified'
                      )
                    ORDER BY rollout.created_at DESC, rollout.id DESC
                    LIMIT 1
                    "#,
                )
                .bind(client_id)
                .fetch_optional(pool)
                .await?
                else {
                    debug!(client_id, "no matching rollout for agent update rollback");
                    return Ok(());
                };
                let rollout_id: Uuid = row.try_get("id")?;
                let rollout_job_id: Uuid = row.try_get("job_id")?;
                let previous_status: String = row.try_get("previous_status")?;
                let updated = sqlx::query(
                    r#"
                    UPDATE agent_update_rollout_targets
                    SET status = 'rolled_back',
                        exit_code = 0,
                        updated_at = now()
                    WHERE rollout_id = $1
                      AND client_id = $2
                      AND status IN (
                        'activation_pending_restart',
                        'heartbeat_timeout',
                        'activation_failed',
                        'heartbeat_verified'
                      )
                    "#,
                )
                .bind(rollout_id)
                .bind(client_id)
                .execute(pool)
                .await?;
                if updated.rows_affected() == 0 {
                    debug!(
                        client_id,
                        rollout_id = %rollout_id,
                        "agent update rollback did not update rollout target"
                    );
                    return Ok(());
                }
                let targets = self.list_agent_update_rollout_targets(rollout_id).await?;
                sqlx::query(
                    r#"
                    UPDATE agent_update_rollouts
                    SET status = $2,
                        updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(rollout_id)
                .bind(rollout_status_for_rollback_targets(&targets))
                .execute(pool)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, $2, $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind("agent_update.rollback_completed")
                .bind(format!("client:{client_id}"))
                .bind(agent_update_rollback_metadata(
                    rollout_id,
                    rollout_job_id,
                    rollback_job_id,
                    client_id,
                    rollback_sha256_hex,
                    &previous_status,
                ))
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn expire_agent_update_heartbeat_timeouts(
        &self,
        default_timeout_secs: i32,
    ) -> Result<i64> {
        let default_timeout_secs = default_timeout_secs.clamp(1, 86_400);
        match self {
            Self::Memory(memory) => {
                let now_unix = crate::unix_now();
                let now = now_unix.to_string();
                let mut expired_count = 0_i64;
                let mut audits = Vec::new();
                {
                    let mut rollouts = memory.agent_update_rollouts.write().await;
                    for rollout in rollouts.iter_mut() {
                        let timeout_secs = rollout
                            .heartbeat_timeout_secs
                            .unwrap_or(default_timeout_secs);
                        let mut changed = false;
                        for target in rollout.targets.iter_mut() {
                            if !memory_rollout_target_is_expired(target, now_unix, timeout_secs) {
                                continue;
                            }
                            let previous_updated_at = target.updated_at.clone();
                            target.status = "heartbeat_timeout".to_string();
                            target.exit_code = None;
                            target.updated_at = now.clone();
                            expired_count += 1;
                            changed = true;
                            audits.push(AuditLogView {
                                id: Uuid::new_v4(),
                                actor_id: None,
                                action: "agent_update.heartbeat_timeout".to_string(),
                                target: format!("client:{}", target.client_id),
                                command_hash: None,
                                metadata: agent_update_heartbeat_timeout_metadata(
                                    rollout.id,
                                    rollout.job_id,
                                    &target.client_id,
                                    &rollout.artifact_sha256_hex,
                                    timeout_secs,
                                    &previous_updated_at,
                                ),
                                created_at: now.clone(),
                            });
                        }
                        if changed {
                            let (completed_count, failed_count, pending_count) =
                                rollout_target_summary(&rollout.targets);
                            rollout.completed_count = completed_count;
                            rollout.failed_count = failed_count;
                            rollout.pending_count = pending_count;
                            rollout.status =
                                rollout_status_for_timeout_targets(&rollout.targets).to_string();
                            rollout.updated_at = now.clone();
                        }
                    }
                }
                if !audits.is_empty() {
                    memory.audits.write().await.extend(audits);
                }
                Ok(expired_count)
            }
            Self::Postgres(pool) => {
                let expired_rows = sqlx::query(
                    r#"
                    SELECT
                        rollout.id,
                        rollout.job_id,
                        rollout.artifact_sha256_hex,
                        target.client_id,
                        COALESCE(rollout.heartbeat_timeout_secs, $1)::integer AS heartbeat_timeout_secs,
                        target.updated_at::text AS target_updated_at
                    FROM agent_update_rollouts rollout
                    JOIN agent_update_rollout_targets target
                      ON target.rollout_id = rollout.id
                    WHERE target.status = 'activation_pending_restart'
                      AND target.updated_at <= now() - (
                          COALESCE(rollout.heartbeat_timeout_secs, $1)::text || ' seconds'
                      )::interval
                    "#,
                )
                .bind(default_timeout_secs)
                .fetch_all(pool)
                .await?;
                let mut expired_count = 0_i64;
                let mut affected_rollouts = BTreeSet::new();
                for row in expired_rows {
                    let rollout_id: Uuid = row.try_get("id")?;
                    let rollout_job_id: Uuid = row.try_get("job_id")?;
                    let artifact_sha256_hex: String = row.try_get("artifact_sha256_hex")?;
                    let client_id: String = row.try_get("client_id")?;
                    let heartbeat_timeout_secs: i32 = row.try_get("heartbeat_timeout_secs")?;
                    let target_updated_at: String = row.try_get("target_updated_at")?;
                    let updated = sqlx::query(
                        r#"
                        UPDATE agent_update_rollout_targets
                        SET status = 'heartbeat_timeout',
                            exit_code = NULL,
                            updated_at = now()
                        WHERE rollout_id = $1
                          AND client_id = $2
                          AND status = 'activation_pending_restart'
                        "#,
                    )
                    .bind(rollout_id)
                    .bind(&client_id)
                    .execute(pool)
                    .await?;
                    if updated.rows_affected() == 0 {
                        continue;
                    }
                    expired_count += 1;
                    affected_rollouts.insert(rollout_id);
                    sqlx::query(
                        r#"
                        INSERT INTO audit_logs (
                            id, actor_id, action, target, command_hash, metadata
                        )
                        VALUES ($1, NULL, $2, $3, NULL, $4)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind("agent_update.heartbeat_timeout")
                    .bind(format!("client:{client_id}"))
                    .bind(agent_update_heartbeat_timeout_metadata(
                        rollout_id,
                        rollout_job_id,
                        &client_id,
                        &artifact_sha256_hex,
                        heartbeat_timeout_secs,
                        &target_updated_at,
                    ))
                    .execute(pool)
                    .await?;
                }
                for rollout_id in affected_rollouts {
                    let targets = self.list_agent_update_rollout_targets(rollout_id).await?;
                    sqlx::query(
                        r#"
                        UPDATE agent_update_rollouts
                        SET status = $2,
                            updated_at = now()
                        WHERE id = $1
                        "#,
                    )
                    .bind(rollout_id)
                    .bind(rollout_status_for_timeout_targets(&targets))
                    .execute(pool)
                    .await?;
                }
                Ok(expired_count)
            }
        }
    }

    pub(crate) async fn record_agent_update_activation_completed(
        &self,
        client_id: &str,
        activation_job_id: Uuid,
        staged_sha256_hex: &str,
    ) -> Result<()> {
        let sha256_hex = staged_sha256_hex.to_ascii_lowercase();
        match self {
            Self::Memory(memory) => {
                let now = crate::unix_now().to_string();
                let mut rollouts = memory.agent_update_rollouts.write().await;
                let Some(rollout) = rollouts.iter_mut().rev().find(|rollout| {
                    rollout.artifact_sha256_hex == sha256_hex
                        && rollout
                            .targets
                            .iter()
                            .any(|target| target.client_id == client_id)
                }) else {
                    return Ok(());
                };
                let Some(target) = rollout
                    .targets
                    .iter_mut()
                    .find(|target| target.client_id == client_id)
                else {
                    return Ok(());
                };
                if !matches!(
                    target.status.as_str(),
                    "completed" | "activation_pending_restart"
                ) {
                    return Ok(());
                }
                target.status = "activation_pending_restart".to_string();
                target.exit_code = Some(0);
                target.updated_at = now.clone();
                let (completed_count, failed_count, pending_count) =
                    rollout_target_summary(&rollout.targets);
                rollout.completed_count = completed_count;
                rollout.failed_count = failed_count;
                rollout.pending_count = pending_count;
                rollout.status =
                    rollout_status_for_activation_targets(&rollout.targets).to_string();
                rollout.updated_at = now.clone();
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: None,
                    action: "agent_update.activation_pending_restart".to_string(),
                    target: format!("client:{client_id}"),
                    command_hash: None,
                    metadata: agent_update_activation_metadata(
                        rollout.id,
                        rollout.job_id,
                        activation_job_id,
                        client_id,
                        &sha256_hex,
                    ),
                    created_at: now,
                });
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT rollout.id, rollout.job_id
                    FROM agent_update_rollouts rollout
                    JOIN agent_update_rollout_targets target
                      ON target.rollout_id = rollout.id
                    WHERE target.client_id = $1
                      AND rollout.artifact_sha256_hex = $2
                    ORDER BY rollout.created_at DESC, rollout.id DESC
                    LIMIT 1
                    "#,
                )
                .bind(client_id)
                .bind(&sha256_hex)
                .fetch_optional(pool)
                .await?
                else {
                    debug!(
                        client_id,
                        sha256_hex, "no matching rollout for agent update activation"
                    );
                    return Ok(());
                };
                let rollout_id: Uuid = row.try_get("id")?;
                let rollout_job_id: Uuid = row.try_get("job_id")?;
                let updated = sqlx::query(
                    r#"
                    UPDATE agent_update_rollout_targets
                    SET status = 'activation_pending_restart',
                        exit_code = 0,
                        updated_at = now()
                    WHERE rollout_id = $1
                      AND client_id = $2
                      AND status IN ('completed', 'activation_pending_restart')
                    "#,
                )
                .bind(rollout_id)
                .bind(client_id)
                .execute(pool)
                .await?;
                if updated.rows_affected() == 0 {
                    debug!(
                        client_id,
                        sha256_hex,
                        rollout_id = %rollout_id,
                        "agent update activation did not update rollout target"
                    );
                    return Ok(());
                }
                let targets = self.list_agent_update_rollout_targets(rollout_id).await?;
                sqlx::query(
                    r#"
                    UPDATE agent_update_rollouts
                    SET status = $2,
                        updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(rollout_id)
                .bind(rollout_status_for_activation_targets(&targets))
                .execute(pool)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, $2, $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind("agent_update.activation_pending_restart")
                .bind(format!("client:{client_id}"))
                .bind(agent_update_activation_metadata(
                    rollout_id,
                    rollout_job_id,
                    activation_job_id,
                    client_id,
                    &sha256_hex,
                ))
                .execute(pool)
                .await?;
            }
        }
        Ok(())
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
        let sha256_hex = staged_sha256_hex.to_ascii_lowercase();
        match self {
            Self::Memory(memory) => {
                let now = crate::unix_now().to_string();
                let mut rollouts = memory.agent_update_rollouts.write().await;
                let Some(rollout) = rollouts.iter_mut().rev().find(|rollout| {
                    rollout.artifact_sha256_hex == sha256_hex
                        && rollout
                            .targets
                            .iter()
                            .any(|target| target.client_id == client_id)
                }) else {
                    return Ok(());
                };
                let Some(target) = rollout
                    .targets
                    .iter_mut()
                    .find(|target| target.client_id == client_id)
                else {
                    return Ok(());
                };
                if !matches!(
                    target.status.as_str(),
                    "completed" | "activation_pending_restart" | "activation_failed"
                ) {
                    return Ok(());
                }
                let previous_status = target.status.clone();
                target.status = "activation_failed".to_string();
                target.exit_code = exit_code;
                target.updated_at = now.clone();
                let (completed_count, failed_count, pending_count) =
                    rollout_target_summary(&rollout.targets);
                rollout.completed_count = completed_count;
                rollout.failed_count = failed_count;
                rollout.pending_count = pending_count;
                rollout.status =
                    rollout_status_for_activation_failed_targets(&rollout.targets).to_string();
                rollout.updated_at = now.clone();
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: None,
                    action: "agent_update.activation_failed".to_string(),
                    target: format!("client:{client_id}"),
                    command_hash: None,
                    metadata: agent_update_activation_failed_metadata(ActivationFailedMetadata {
                        rollout_id: rollout.id,
                        rollout_job_id: rollout.job_id,
                        activation_job_id,
                        client_id,
                        staged_sha256_hex: &sha256_hex,
                        previous_status: &previous_status,
                        outcome_status,
                        exit_code,
                        message,
                    }),
                    created_at: now,
                });
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT rollout.id, rollout.job_id, target.status AS previous_status
                    FROM agent_update_rollouts rollout
                    JOIN agent_update_rollout_targets target
                      ON target.rollout_id = rollout.id
                    WHERE target.client_id = $1
                      AND rollout.artifact_sha256_hex = $2
                    ORDER BY rollout.created_at DESC, rollout.id DESC
                    LIMIT 1
                    "#,
                )
                .bind(client_id)
                .bind(&sha256_hex)
                .fetch_optional(pool)
                .await?
                else {
                    debug!(
                        client_id,
                        sha256_hex, "no matching rollout for failed agent update activation"
                    );
                    return Ok(());
                };
                let rollout_id: Uuid = row.try_get("id")?;
                let rollout_job_id: Uuid = row.try_get("job_id")?;
                let previous_status: String = row.try_get("previous_status")?;
                let updated = sqlx::query(
                    r#"
                    UPDATE agent_update_rollout_targets
                    SET status = 'activation_failed',
                        exit_code = $3,
                        updated_at = now()
                    WHERE rollout_id = $1
                      AND client_id = $2
                      AND status IN (
                        'completed',
                        'activation_pending_restart',
                        'activation_failed'
                      )
                    "#,
                )
                .bind(rollout_id)
                .bind(client_id)
                .bind(exit_code)
                .execute(pool)
                .await?;
                if updated.rows_affected() == 0 {
                    debug!(
                        client_id,
                        sha256_hex,
                        rollout_id = %rollout_id,
                        "failed agent update activation did not update rollout target"
                    );
                    return Ok(());
                }
                let targets = self.list_agent_update_rollout_targets(rollout_id).await?;
                sqlx::query(
                    r#"
                    UPDATE agent_update_rollouts
                    SET status = $2,
                        updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(rollout_id)
                .bind(rollout_status_for_activation_failed_targets(&targets))
                .execute(pool)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, $2, $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind("agent_update.activation_failed")
                .bind(format!("client:{client_id}"))
                .bind(agent_update_activation_failed_metadata(
                    ActivationFailedMetadata {
                        rollout_id,
                        rollout_job_id,
                        activation_job_id,
                        client_id,
                        staged_sha256_hex: &sha256_hex,
                        previous_status: &previous_status,
                        outcome_status,
                        exit_code,
                        message,
                    },
                ))
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_agent_update_heartbeat(
        &self,
        client_id: &str,
        heartbeat: &AgentUpdateHeartbeat,
    ) -> Result<()> {
        let sha256_hex = heartbeat.sha256_hex.to_ascii_lowercase();
        match self {
            Self::Memory(memory) => {
                let mut rollouts = memory.agent_update_rollouts.write().await;
                let Some(rollout) = rollouts.iter_mut().rev().find(|rollout| {
                    rollout.artifact_sha256_hex == sha256_hex
                        && rollout
                            .targets
                            .iter()
                            .any(|target| target.client_id == client_id)
                }) else {
                    return Ok(());
                };
                let Some(target) = rollout
                    .targets
                    .iter_mut()
                    .find(|target| target.client_id == client_id)
                else {
                    return Ok(());
                };
                if target.status == "heartbeat_verified" {
                    return Ok(());
                }
                if !matches!(
                    target.status.as_str(),
                    "completed" | "activation_pending_restart"
                ) {
                    return Ok(());
                }
                target.status = "heartbeat_verified".to_string();
                target.exit_code = Some(0);
                target.updated_at = heartbeat.observed_unix.to_string();
                let (completed_count, failed_count, pending_count) =
                    rollout_target_summary(&rollout.targets);
                rollout.completed_count = completed_count;
                rollout.failed_count = failed_count;
                rollout.pending_count = pending_count;
                rollout.status = rollout_status_for_heartbeat_targets(&rollout.targets).to_string();
                rollout.updated_at = heartbeat.observed_unix.to_string();
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: None,
                    action: "agent_update.heartbeat_verified".to_string(),
                    target: format!("client:{client_id}"),
                    command_hash: None,
                    metadata: agent_update_heartbeat_metadata(
                        rollout.id,
                        rollout.job_id,
                        client_id,
                        heartbeat,
                    ),
                    created_at: heartbeat.observed_unix.to_string(),
                });
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT rollout.id, rollout.job_id
                    FROM agent_update_rollouts rollout
                    JOIN agent_update_rollout_targets target
                      ON target.rollout_id = rollout.id
                    WHERE target.client_id = $1
                      AND rollout.artifact_sha256_hex = $2
                    ORDER BY rollout.created_at DESC, rollout.id DESC
                    LIMIT 1
                    "#,
                )
                .bind(client_id)
                .bind(&sha256_hex)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(());
                };
                let rollout_id: Uuid = row.try_get("id")?;
                let rollout_job_id: Uuid = row.try_get("job_id")?;
                let updated = sqlx::query(
                    r#"
                    UPDATE agent_update_rollout_targets
                    SET status = 'heartbeat_verified',
                        exit_code = 0,
                        updated_at = now()
                    WHERE rollout_id = $1
                      AND client_id = $2
                      AND status IN ('completed', 'activation_pending_restart')
                    "#,
                )
                .bind(rollout_id)
                .bind(client_id)
                .execute(pool)
                .await?;
                if updated.rows_affected() == 0 {
                    return Ok(());
                }
                let targets = self.list_agent_update_rollout_targets(rollout_id).await?;
                sqlx::query(
                    r#"
                    UPDATE agent_update_rollouts
                    SET status = $2,
                        updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(rollout_id)
                .bind(rollout_status_for_heartbeat_targets(&targets))
                .execute(pool)
                .await?;
                debug!(
                    client_id,
                    sha256_hex,
                    rollout_id = %rollout_id,
                    rollout_status = rollout_status_for_heartbeat_targets(&targets),
                    target_statuses = ?targets
                        .iter()
                        .map(|target| (&target.client_id, &target.status))
                        .collect::<Vec<_>>(),
                    "agent update heartbeat updated rollout"
                );
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, $2, $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind("agent_update.heartbeat_verified")
                .bind(format!("client:{client_id}"))
                .bind(agent_update_heartbeat_metadata(
                    rollout_id,
                    rollout_job_id,
                    client_id,
                    heartbeat,
                ))
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}
