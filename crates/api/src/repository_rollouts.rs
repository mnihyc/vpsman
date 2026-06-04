use anyhow::Result;
use serde_json::json;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::payload_hash;

use crate::{
    model::{
        AgentUpdateRolloutControlRequest, AgentUpdateRolloutTargetView, AgentUpdateRolloutView,
        AuditLogView, AuthContext,
    },
    model_rollout_policies::ResolvedAgentUpdateRolloutPolicy,
    repository::{MemoryState, Repository},
};

pub(crate) const AGENT_UPDATE_ROLLOUT_ACTIVATION_POLICY: &str = "manual_staging_only";
pub(crate) const DEFAULT_AGENT_UPDATE_HEARTBEAT_TIMEOUT_SECS: i32 = 900;
pub(crate) const ROLLOUT_HEALTH_GATE_HEARTBEAT_VERIFIED: &str = "heartbeat_verified";
pub(crate) const ROLLOUT_HEALTH_GATE_MANUAL_AFTER_CANARY: &str = "manual_after_canary";
pub(crate) const ROLLOUT_HEALTH_GATE_MANUAL_ONLY: &str = "manual_only";
pub(crate) const ROLLOUT_DELEGATED_ACTION_ROLLBACK: &str = "agent_update_rollback";
pub(crate) const ROLLOUT_DELEGATED_ACTION_ACTIVATE: &str = "agent_update_activate";

pub(crate) fn rollout_status_for_job_status(job_status: &str) -> &'static str {
    match job_status {
        "completed" => "staged",
        "partially_completed" => "partially_staged",
        "dispatch_failed" => "dispatch_failed",
        _ => "staging_requested",
    }
}

pub(crate) fn update_signing_key_sha256(signing_key_hex: Option<&str>) -> Option<String> {
    signing_key_hex.map(|key| payload_hash(key.to_ascii_lowercase().as_bytes()))
}

pub(crate) fn rollout_target_summary(targets: &[AgentUpdateRolloutTargetView]) -> (i32, i32, i32) {
    let mut completed = 0_i32;
    let mut failed = 0_i32;
    let mut pending = 0_i32;
    for target in targets {
        match target.status.as_str() {
            "completed" | "heartbeat_verified" | "rolled_back" => completed += 1,
            "queued" | "accepted" | "dispatching" | "activation_pending_restart" => pending += 1,
            _ => failed += 1,
        }
    }
    (completed, failed, pending)
}

pub(crate) fn rollout_status_for_activation_targets(
    targets: &[AgentUpdateRolloutTargetView],
) -> &'static str {
    if targets
        .iter()
        .all(|target| target.status == "activation_pending_restart")
    {
        "activation_pending_restart"
    } else {
        "partially_activation_pending_restart"
    }
}

pub(crate) fn rollout_status_for_activation_failed_targets(
    targets: &[AgentUpdateRolloutTargetView],
) -> &'static str {
    if targets
        .iter()
        .all(|target| target.status == "activation_failed")
    {
        "activation_failed"
    } else {
        "partially_activation_failed"
    }
}

pub(crate) fn rollout_status_for_heartbeat_targets(
    targets: &[AgentUpdateRolloutTargetView],
) -> &'static str {
    if targets
        .iter()
        .all(|target| target.status == "heartbeat_verified")
    {
        "heartbeat_verified"
    } else {
        "partially_heartbeat_verified"
    }
}

pub(crate) fn rollout_status_for_timeout_targets(
    targets: &[AgentUpdateRolloutTargetView],
) -> &'static str {
    if targets
        .iter()
        .all(|target| target.status == "heartbeat_timeout")
    {
        "heartbeat_timeout"
    } else {
        "partially_heartbeat_timeout"
    }
}

pub(crate) fn rollout_status_for_rollback_targets(
    targets: &[AgentUpdateRolloutTargetView],
) -> &'static str {
    if targets.iter().all(|target| target.status == "rolled_back") {
        "rolled_back"
    } else {
        "partially_rolled_back"
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_memory_agent_update_rollout(
    memory: &MemoryState,
    job_id: Uuid,
    operator: &AuthContext,
    command_hash: &str,
    resolved_targets: &[String],
    sha256_hex: &str,
    artifact_signature_provided: bool,
    artifact_signing_key_hex: Option<&str>,
    canary_count: i32,
    rollout_policy: &ResolvedAgentUpdateRolloutPolicy,
    created_at: &str,
) {
    let targets = resolved_targets
        .iter()
        .cloned()
        .map(|client_id| AgentUpdateRolloutTargetView {
            client_id,
            status: "queued".to_string(),
            exit_code: None,
            updated_at: created_at.to_string(),
        })
        .collect::<Vec<_>>();
    let (completed_count, failed_count, pending_count) = rollout_target_summary(&targets);
    memory
        .agent_update_rollouts
        .write()
        .await
        .push(AgentUpdateRolloutView {
            id: Uuid::new_v4(),
            job_id,
            actor_id: Some(operator.operator.id),
            status: "staging_requested".to_string(),
            artifact_sha256_hex: sha256_hex.to_ascii_lowercase(),
            artifact_signature_provided,
            artifact_signing_key_sha256_hex: update_signing_key_sha256(artifact_signing_key_hex),
            target_count: resolved_targets.len() as i32,
            completed_count,
            failed_count,
            pending_count,
            activation_policy: AGENT_UPDATE_ROLLOUT_ACTIVATION_POLICY.to_string(),
            canary_count,
            rollout_policy_id: rollout_policy.policy_id,
            rollout_policy_name: rollout_policy.policy_name.clone(),
            heartbeat_timeout_secs: Some(DEFAULT_AGENT_UPDATE_HEARTBEAT_TIMEOUT_SECS),
            automation_paused: false,
            automation_pause_reason: None,
            automation_health_gate: rollout_policy
                .automation_health_gate
                .clone()
                .unwrap_or_else(|| ROLLOUT_HEALTH_GATE_HEARTBEAT_VERIFIED.to_string()),
            automation_lease_owner: None,
            automation_lease_expires_at: None,
            automation_status: "unreconciled".to_string(),
            automation_next_action: None,
            automation_blocker: None,
            automation_targets: Vec::new(),
            automation_updated_at: None,
            activation_delegations: Vec::new(),
            rollback_delegations: Vec::new(),
            targets,
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
        });
    memory.audits.write().await.push(AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "agent_update.rollout_recorded".to_string(),
        target: format!("job:{job_id}"),
        command_hash: Some(command_hash.to_string()),
        metadata: agent_update_rollout_metadata(
            job_id,
            resolved_targets,
            sha256_hex,
            artifact_signature_provided,
            artifact_signing_key_hex,
            canary_count,
            rollout_policy,
        ),
        created_at: created_at.to_string(),
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_postgres_agent_update_rollout(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    operator: &AuthContext,
    command_hash: &str,
    resolved_targets: &[String],
    sha256_hex: &str,
    artifact_signature_provided: bool,
    artifact_signing_key_hex: Option<&str>,
    canary_count: i32,
    rollout_policy: &ResolvedAgentUpdateRolloutPolicy,
) -> Result<()> {
    let rollout_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO agent_update_rollouts (
            id,
            job_id,
            actor_id,
            status,
            artifact_sha256_hex,
            artifact_signature_provided,
            artifact_signing_key_sha256_hex,
            target_count,
            canary_count,
            rollout_policy_id,
            rollout_policy_name,
            activation_policy,
            heartbeat_timeout_secs,
            automation_health_gate
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
    )
    .bind(rollout_id)
    .bind(job_id)
    .bind(operator.operator.id)
    .bind("staging_requested")
    .bind(sha256_hex.to_ascii_lowercase())
    .bind(artifact_signature_provided)
    .bind(update_signing_key_sha256(artifact_signing_key_hex))
    .bind(resolved_targets.len() as i32)
    .bind(canary_count)
    .bind(rollout_policy.policy_id)
    .bind(&rollout_policy.policy_name)
    .bind(AGENT_UPDATE_ROLLOUT_ACTIVATION_POLICY)
    .bind(Some(DEFAULT_AGENT_UPDATE_HEARTBEAT_TIMEOUT_SECS))
    .bind(
        rollout_policy
            .automation_health_gate
            .as_deref()
            .unwrap_or(ROLLOUT_HEALTH_GATE_HEARTBEAT_VERIFIED),
    )
    .execute(&mut **tx)
    .await?;
    for client_id in resolved_targets {
        sqlx::query(
            r#"
            INSERT INTO agent_update_rollout_targets (
                rollout_id, client_id, status
            )
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(rollout_id)
        .bind(client_id)
        .bind("queued")
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(operator.operator.id)
    .bind("agent_update.rollout_recorded")
    .bind(format!("agent_update_rollout:{rollout_id}"))
    .bind(command_hash)
    .bind(agent_update_rollout_metadata(
        job_id,
        resolved_targets,
        sha256_hex,
        artifact_signature_provided,
        artifact_signing_key_hex,
        canary_count,
        rollout_policy,
    ))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn agent_update_rollout_metadata(
    job_id: Uuid,
    resolved_targets: &[String],
    sha256_hex: &str,
    artifact_signature_provided: bool,
    artifact_signing_key_hex: Option<&str>,
    canary_count: i32,
    rollout_policy: &ResolvedAgentUpdateRolloutPolicy,
) -> serde_json::Value {
    json!({
        "job_id": job_id,
        "target_count": resolved_targets.len(),
        "artifact_sha256_hex": sha256_hex.to_ascii_lowercase(),
        "artifact_signature_provided": artifact_signature_provided,
        "artifact_signing_key_sha256_hex": update_signing_key_sha256(
            artifact_signing_key_hex
        ),
        "canary_count": canary_count,
        "rollout_policy_id": rollout_policy.policy_id,
        "rollout_policy_name": &rollout_policy.policy_name,
        "rollout_policy_canary_default": rollout_policy.canary_count,
        "rollout_policy_health_gate_default": &rollout_policy.automation_health_gate,
        "activation_policy": AGENT_UPDATE_ROLLOUT_ACTIVATION_POLICY,
    })
}

fn rollout_control_metadata(
    rollout: &AgentUpdateRolloutView,
    request: &AgentUpdateRolloutControlRequest,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "rollout_id": rollout.id,
        "operator_id": operator.operator.id,
        "paused": rollout.automation_paused,
        "pause_reason": rollout.automation_pause_reason,
        "automation_health_gate": rollout.automation_health_gate,
        "requested_pause_change": request.paused,
        "requested_health_gate": request.automation_health_gate,
    })
}

impl Repository {
    pub(crate) async fn list_agent_update_rollouts(
        &self,
        limit: i64,
    ) -> Result<Vec<AgentUpdateRolloutView>> {
        match self {
            Self::Memory(memory) => {
                let mut rollouts = memory.agent_update_rollouts.read().await.clone();
                rollouts.sort_by(|left, right| {
                    right
                        .created_at
                        .cmp(&left.created_at)
                        .then_with(|| right.id.cmp(&left.id))
                });
                let mut rollouts = rollouts
                    .into_iter()
                    .take(limit as usize)
                    .collect::<Vec<_>>();
                for rollout in &mut rollouts {
                    self.attach_agent_update_delegation_summaries(rollout)
                        .await?;
                }
                Ok(rollouts)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        job_id,
                        actor_id,
                        status,
                        artifact_sha256_hex,
                        artifact_signature_provided,
                        artifact_signing_key_sha256_hex,
                        target_count,
                        canary_count,
                        rollout_policy_id,
                        rollout_policy_name,
                        activation_policy,
                        heartbeat_timeout_secs,
                        automation_paused,
                        automation_pause_reason,
                        automation_health_gate,
                        automation_lease_owner,
                        automation_lease_expires_at::text AS automation_lease_expires_at,
                        automation_status,
                        automation_next_action,
                        automation_blocker,
                        automation_targets,
                        automation_updated_at::text AS automation_updated_at,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM agent_update_rollouts
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                let mut rollouts = Vec::with_capacity(rows.len());
                for row in rows {
                    let rollout_id: Uuid = row.try_get("id")?;
                    let targets = self.list_agent_update_rollout_targets(rollout_id).await?;
                    let (completed_count, failed_count, pending_count) =
                        rollout_target_summary(&targets);
                    let mut rollout = AgentUpdateRolloutView {
                        id: rollout_id,
                        job_id: row.try_get("job_id")?,
                        actor_id: row.try_get("actor_id")?,
                        status: row.try_get("status")?,
                        artifact_sha256_hex: row.try_get("artifact_sha256_hex")?,
                        artifact_signature_provided: row.try_get("artifact_signature_provided")?,
                        artifact_signing_key_sha256_hex: row
                            .try_get("artifact_signing_key_sha256_hex")?,
                        target_count: row.try_get("target_count")?,
                        completed_count,
                        failed_count,
                        pending_count,
                        activation_policy: row.try_get("activation_policy")?,
                        canary_count: row.try_get("canary_count")?,
                        rollout_policy_id: row.try_get("rollout_policy_id")?,
                        rollout_policy_name: row.try_get("rollout_policy_name")?,
                        heartbeat_timeout_secs: row.try_get("heartbeat_timeout_secs")?,
                        automation_paused: row.try_get("automation_paused")?,
                        automation_pause_reason: row.try_get("automation_pause_reason")?,
                        automation_health_gate: row.try_get("automation_health_gate")?,
                        automation_lease_owner: row.try_get("automation_lease_owner")?,
                        automation_lease_expires_at: row.try_get("automation_lease_expires_at")?,
                        automation_status: row.try_get("automation_status")?,
                        automation_next_action: row.try_get("automation_next_action")?,
                        automation_blocker: row.try_get("automation_blocker")?,
                        automation_targets: row.try_get("automation_targets")?,
                        automation_updated_at: row.try_get("automation_updated_at")?,
                        activation_delegations: Vec::new(),
                        rollback_delegations: Vec::new(),
                        targets,
                        created_at: row.try_get("created_at")?,
                        updated_at: row.try_get("updated_at")?,
                    };
                    self.attach_agent_update_delegation_summaries(&mut rollout)
                        .await?;
                    rollouts.push(rollout);
                }
                Ok(rollouts)
            }
        }
    }

    pub(crate) async fn update_agent_update_rollout_control(
        &self,
        rollout_id: Uuid,
        request: &AgentUpdateRolloutControlRequest,
        operator: &AuthContext,
    ) -> Result<AgentUpdateRolloutView> {
        let pause_reason = request
            .pause_reason
            .as_ref()
            .map(|reason| reason.trim())
            .filter(|reason| !reason.is_empty());
        match self {
            Self::Memory(memory) => {
                let now = crate::unix_now().to_string();
                let mut rollouts = memory.agent_update_rollouts.write().await;
                let rollout = rollouts
                    .iter_mut()
                    .find(|rollout| rollout.id == rollout_id)
                    .ok_or_else(|| anyhow::anyhow!("agent_update_rollout_not_found"))?;
                if let Some(paused) = request.paused {
                    rollout.automation_paused = paused;
                    rollout.automation_pause_reason = if paused {
                        Some(
                            pause_reason
                                .unwrap_or("operator paused rollout automation")
                                .to_string(),
                        )
                    } else {
                        None
                    };
                }
                if let Some(health_gate) = request.automation_health_gate.as_ref() {
                    rollout.automation_health_gate = health_gate.clone();
                }
                if rollout.automation_paused {
                    rollout.automation_status = "paused".to_string();
                    rollout.automation_next_action = None;
                    rollout.automation_blocker = rollout.automation_pause_reason.clone();
                    rollout.automation_targets.clear();
                } else {
                    rollout.automation_status = "unreconciled".to_string();
                    rollout.automation_next_action = None;
                    rollout.automation_blocker = None;
                    rollout.automation_targets.clear();
                }
                rollout.automation_updated_at = Some(now.clone());
                rollout.updated_at = now.clone();
                let updated = rollout.clone();
                drop(rollouts);
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "agent_update.rollout_control_updated".to_string(),
                    target: format!("agent_update_rollout:{rollout_id}"),
                    command_hash: None,
                    metadata: rollout_control_metadata(&updated, request, operator),
                    created_at: now,
                });
                Ok(updated)
            }
            Self::Postgres(pool) => {
                let updated = sqlx::query(
                    r#"
                    UPDATE agent_update_rollouts
                    SET automation_paused = COALESCE($2, automation_paused),
                        automation_pause_reason = CASE
                            WHEN COALESCE($2, automation_paused)
                            THEN COALESCE(NULLIF(BTRIM($3::text), ''), automation_pause_reason, 'operator paused rollout automation')
                            ELSE NULL
                        END,
                        automation_health_gate = COALESCE($4::text, automation_health_gate),
                        automation_status = CASE
                            WHEN COALESCE($2, automation_paused) THEN 'paused'
                            ELSE 'unreconciled'
                        END,
                        automation_next_action = NULL,
                        automation_blocker = CASE
                            WHEN COALESCE($2, automation_paused)
                            THEN COALESCE(NULLIF(BTRIM($3::text), ''), automation_pause_reason, 'operator paused rollout automation')
                            ELSE NULL
                        END,
                        automation_targets = '{}',
                        automation_updated_at = now(),
                        updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(rollout_id)
                .bind(request.paused)
                .bind(pause_reason)
                .bind(request.automation_health_gate.as_deref())
                .execute(pool)
                .await?;
                anyhow::ensure!(
                    updated.rows_affected() == 1,
                    "agent_update_rollout_not_found"
                );
                let rollout = self
                    .find_agent_update_rollout(rollout_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("agent_update_rollout_not_found"))?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("agent_update.rollout_control_updated")
                .bind(format!("agent_update_rollout:{rollout_id}"))
                .bind(rollout_control_metadata(&rollout, request, operator))
                .execute(pool)
                .await?;
                Ok(rollout)
            }
        }
    }

    pub(crate) async fn find_agent_update_rollout(
        &self,
        rollout_id: Uuid,
    ) -> Result<Option<AgentUpdateRolloutView>> {
        match self {
            Self::Memory(memory) => {
                let mut rollout = memory
                    .agent_update_rollouts
                    .read()
                    .await
                    .iter()
                    .find(|rollout| rollout.id == rollout_id)
                    .cloned();
                if let Some(rollout) = &mut rollout {
                    self.attach_agent_update_delegation_summaries(rollout)
                        .await?;
                }
                Ok(rollout)
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        job_id,
                        actor_id,
                        status,
                        artifact_sha256_hex,
                        artifact_signature_provided,
                        artifact_signing_key_sha256_hex,
                        target_count,
                        canary_count,
                        rollout_policy_id,
                        rollout_policy_name,
                        activation_policy,
                        heartbeat_timeout_secs,
                        automation_paused,
                        automation_pause_reason,
                        automation_health_gate,
                        automation_lease_owner,
                        automation_lease_expires_at::text AS automation_lease_expires_at,
                        automation_status,
                        automation_next_action,
                        automation_blocker,
                        automation_targets,
                        automation_updated_at::text AS automation_updated_at,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM agent_update_rollouts
                    WHERE id = $1
                    "#,
                )
                .bind(rollout_id)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                let targets = self.list_agent_update_rollout_targets(rollout_id).await?;
                let (completed_count, failed_count, pending_count) =
                    rollout_target_summary(&targets);
                let mut rollout = AgentUpdateRolloutView {
                    id: rollout_id,
                    job_id: row.try_get("job_id")?,
                    actor_id: row.try_get("actor_id")?,
                    status: row.try_get("status")?,
                    artifact_sha256_hex: row.try_get("artifact_sha256_hex")?,
                    artifact_signature_provided: row.try_get("artifact_signature_provided")?,
                    artifact_signing_key_sha256_hex: row
                        .try_get("artifact_signing_key_sha256_hex")?,
                    target_count: row.try_get("target_count")?,
                    completed_count,
                    failed_count,
                    pending_count,
                    activation_policy: row.try_get("activation_policy")?,
                    canary_count: row.try_get("canary_count")?,
                    rollout_policy_id: row.try_get("rollout_policy_id")?,
                    rollout_policy_name: row.try_get("rollout_policy_name")?,
                    heartbeat_timeout_secs: row.try_get("heartbeat_timeout_secs")?,
                    automation_paused: row.try_get("automation_paused")?,
                    automation_pause_reason: row.try_get("automation_pause_reason")?,
                    automation_health_gate: row.try_get("automation_health_gate")?,
                    automation_lease_owner: row.try_get("automation_lease_owner")?,
                    automation_lease_expires_at: row.try_get("automation_lease_expires_at")?,
                    automation_status: row.try_get("automation_status")?,
                    automation_next_action: row.try_get("automation_next_action")?,
                    automation_blocker: row.try_get("automation_blocker")?,
                    automation_targets: row.try_get("automation_targets")?,
                    automation_updated_at: row.try_get("automation_updated_at")?,
                    activation_delegations: Vec::new(),
                    rollback_delegations: Vec::new(),
                    targets,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                };
                self.attach_agent_update_delegation_summaries(&mut rollout)
                    .await?;
                Ok(Some(rollout))
            }
        }
    }

    pub(crate) async fn list_agent_update_rollout_targets(
        &self,
        rollout_id: Uuid,
    ) -> Result<Vec<AgentUpdateRolloutTargetView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .agent_update_rollouts
                .read()
                .await
                .iter()
                .find(|rollout| rollout.id == rollout_id)
                .map(|rollout| rollout.targets.clone())
                .unwrap_or_default()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        status,
                        exit_code,
                        updated_at::text AS updated_at
                    FROM agent_update_rollout_targets
                    WHERE rollout_id = $1
                    ORDER BY client_id
                    "#,
                )
                .bind(rollout_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(AgentUpdateRolloutTargetView {
                            client_id: row.try_get("client_id")?,
                            status: row.try_get("status")?,
                            exit_code: row.try_get("exit_code")?,
                            updated_at: row.try_get("updated_at")?,
                        })
                    })
                    .collect()
            }
        }
    }
}
