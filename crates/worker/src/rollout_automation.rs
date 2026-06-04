use anyhow::Result;
use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const PROOF_DELEGATION_BLOCKER: &str =
    "privileged rollout dispatch requires fresh per-target proof; use panel or CLI until proof delegation is configured";

#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct RolloutAutomationRun {
    pub(crate) expired_heartbeats: i64,
    pub(crate) reconciled_rollouts: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RolloutTargetState {
    client_id: String,
    status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RolloutAutomationDecision {
    status: String,
    next_action: Option<String>,
    blocker: Option<String>,
    targets: Vec<String>,
}

pub(crate) async fn process_rollout_automation(
    pool: &PgPool,
    rollout_limit: i64,
    default_heartbeat_timeout_secs: i32,
    worker_id: &str,
    lease_secs: i32,
) -> Result<RolloutAutomationRun> {
    let expired_heartbeats =
        expire_agent_update_heartbeat_timeouts(pool, default_heartbeat_timeout_secs).await?;
    let reconciled_rollouts =
        refresh_rollout_automation(pool, rollout_limit, worker_id, lease_secs).await?;
    Ok(RolloutAutomationRun {
        expired_heartbeats,
        reconciled_rollouts,
    })
}

async fn expire_agent_update_heartbeat_timeouts(
    pool: &PgPool,
    default_timeout_secs: i32,
) -> Result<i64> {
    let default_timeout_secs = default_timeout_secs.clamp(1, 86_400);
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
        ORDER BY target.updated_at, rollout.id, target.client_id
        "#,
    )
    .bind(default_timeout_secs)
    .fetch_all(pool)
    .await?;

    let mut expired_count = 0_i64;
    let mut affected_rollouts = Vec::new();
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
        affected_rollouts.push(rollout_id);
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
        .bind(json!({
            "rollout_id": rollout_id,
            "rollout_job_id": rollout_job_id,
            "client_id": client_id,
            "artifact_sha256_hex": artifact_sha256_hex.to_ascii_lowercase(),
            "heartbeat_timeout_secs": heartbeat_timeout_secs,
            "target_updated_at": target_updated_at,
            "status": "heartbeat_timeout",
            "worker": "rollout_automation",
        }))
        .execute(pool)
        .await?;
    }

    affected_rollouts.sort();
    affected_rollouts.dedup();
    for rollout_id in affected_rollouts {
        update_rollout_timeout_status(pool, rollout_id).await?;
    }
    Ok(expired_count)
}

async fn update_rollout_timeout_status(pool: &PgPool, rollout_id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE agent_update_rollouts
        SET status = CASE
                WHEN NOT EXISTS (
                    SELECT 1
                    FROM agent_update_rollout_targets
                    WHERE rollout_id = $1 AND status <> 'heartbeat_timeout'
                )
                THEN 'heartbeat_timeout'
                ELSE 'partially_heartbeat_timeout'
            END,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(rollout_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn refresh_rollout_automation(
    pool: &PgPool,
    limit: i64,
    worker_id: &str,
    lease_secs: i32,
) -> Result<usize> {
    let mut tx = pool.begin().await?;
    let lease_secs = lease_secs.clamp(1, 3600);
    let rollout_rows = sqlx::query(
        r#"
        SELECT
            id,
            job_id,
            canary_count,
            automation_paused,
            automation_pause_reason,
            automation_health_gate,
            automation_status,
            automation_next_action,
            automation_blocker,
            automation_targets
        FROM agent_update_rollouts
        WHERE automation_lease_expires_at IS NULL
           OR automation_lease_expires_at <= now()
           OR automation_lease_owner = $2
        ORDER BY automation_updated_at NULLS FIRST, updated_at, id
        LIMIT $1
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(limit.clamp(1, 200))
    .bind(worker_id)
    .fetch_all(&mut *tx)
    .await?;

    let mut changed_count = 0_usize;
    for row in rollout_rows {
        let rollout_id: Uuid = row.try_get("id")?;
        let rollout_job_id: Uuid = row.try_get("job_id")?;
        let canary_count: i32 = row.try_get("canary_count")?;
        let paused: bool = row.try_get("automation_paused")?;
        let pause_reason: Option<String> = row.try_get("automation_pause_reason")?;
        let health_gate: String = row.try_get("automation_health_gate")?;
        let current = RolloutAutomationDecision {
            status: row.try_get("automation_status")?,
            next_action: row.try_get("automation_next_action")?,
            blocker: row.try_get("automation_blocker")?,
            targets: row.try_get("automation_targets")?,
        };
        let targets = load_rollout_targets(&mut tx, rollout_id).await?;
        let control = RolloutControlState {
            canary_count,
            paused,
            pause_reason,
            health_gate,
        };
        let decision = plan_rollout_automation(&control, &targets);
        if decision == current {
            sqlx::query(
                r#"
                UPDATE agent_update_rollouts
                SET automation_lease_owner = $2,
                    automation_lease_expires_at = now() + ($3::text || ' seconds')::interval
                WHERE id = $1
                "#,
            )
            .bind(rollout_id)
            .bind(worker_id)
            .bind(lease_secs)
            .execute(&mut *tx)
            .await?;
            continue;
        }
        sqlx::query(
            r#"
            UPDATE agent_update_rollouts
            SET automation_status = $2,
                automation_next_action = $3,
                automation_blocker = $4,
                automation_targets = $5,
                automation_updated_at = now(),
                automation_lease_owner = $6,
                automation_lease_expires_at = now() + ($7::text || ' seconds')::interval
            WHERE id = $1
            "#,
        )
        .bind(rollout_id)
        .bind(&decision.status)
        .bind(&decision.next_action)
        .bind(&decision.blocker)
        .bind(&decision.targets)
        .bind(worker_id)
        .bind(lease_secs)
        .execute(&mut *tx)
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
        .bind("agent_update.rollout_automation_reconciled")
        .bind(format!("agent_update_rollout:{rollout_id}"))
        .bind(json!({
            "rollout_id": rollout_id,
            "rollout_job_id": rollout_job_id,
            "previous_status": current.status,
            "automation_status": decision.status,
            "automation_next_action": decision.next_action,
            "automation_blocker": decision.blocker,
            "automation_targets": decision.targets,
            "automation_health_gate": control.health_gate,
            "automation_paused": control.paused,
            "worker_id": worker_id,
            "lease_secs": lease_secs,
        }))
        .execute(&mut *tx)
        .await?;
        changed_count += 1;
    }
    tx.commit().await?;
    Ok(changed_count)
}

async fn load_rollout_targets(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rollout_id: Uuid,
) -> Result<Vec<RolloutTargetState>> {
    let rows = sqlx::query(
        r#"
        SELECT client_id, status
        FROM agent_update_rollout_targets
        WHERE rollout_id = $1
        ORDER BY client_id
        "#,
    )
    .bind(rollout_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(RolloutTargetState {
                client_id: row.try_get("client_id")?,
                status: row.try_get("status")?,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RolloutControlState {
    canary_count: i32,
    paused: bool,
    pause_reason: Option<String>,
    health_gate: String,
}

fn plan_rollout_automation(
    control: &RolloutControlState,
    targets: &[RolloutTargetState],
) -> RolloutAutomationDecision {
    if control.paused {
        return decision(
            "paused",
            None,
            Some(
                control
                    .pause_reason
                    .as_deref()
                    .unwrap_or("rollout automation is paused"),
            ),
            Vec::new(),
        );
    }
    if targets.is_empty() {
        return decision(
            "manual_review",
            None,
            Some("rollout has no targets"),
            Vec::new(),
        );
    }
    if targets.iter().all(|target| target.status == "rolled_back") {
        return decision("rolled_back", None, None, Vec::new());
    }
    if targets
        .iter()
        .all(|target| target.status == "heartbeat_verified")
    {
        return decision("complete", None, None, Vec::new());
    }
    if control.health_gate == "manual_only" {
        return decision(
            "manual_review",
            None,
            Some("automation health gate manual_only requires operator review"),
            Vec::new(),
        );
    }

    let rollback_targets =
        targets_with_status(targets, &["heartbeat_timeout", "activation_failed"]);
    if !rollback_targets.is_empty() {
        return decision(
            "rollback_required",
            Some("operator_rollback_targets"),
            Some(PROOF_DELEGATION_BLOCKER),
            rollback_targets,
        );
    }
    if targets
        .iter()
        .any(|target| target.status == "activation_pending_restart")
    {
        return decision(
            "waiting_for_heartbeat",
            None,
            Some("waiting for post-restart agent update heartbeat"),
            Vec::new(),
        );
    }
    if targets.iter().any(|target| {
        matches!(
            target.status.as_str(),
            "failed" | "dispatch_failed" | "rejected_by_agent" | "timed_out"
        )
    }) {
        return decision(
            "staging_failed",
            None,
            Some("staging failed before activation; inspect target job output"),
            Vec::new(),
        );
    }

    let staged_targets = targets_with_status(targets, &["completed"]);
    if !staged_targets.is_empty() {
        let has_verified = targets
            .iter()
            .any(|target| target.status == "heartbeat_verified");
        if control.health_gate == "manual_after_canary" && has_verified {
            return decision(
                "manual_review",
                None,
                Some("automation health gate manual_after_canary requires operator review before the next batch"),
                Vec::new(),
            );
        }
        let batch_size = rollout_batch_size(control.canary_count, staged_targets.len());
        let action = if has_verified {
            "ready_activate_next_batch"
        } else {
            "ready_activate_canary"
        };
        return decision(
            action,
            Some("operator_activate_batch"),
            Some(PROOF_DELEGATION_BLOCKER),
            staged_targets.into_iter().take(batch_size).collect(),
        );
    }

    if targets.iter().any(|target| {
        matches!(
            target.status.as_str(),
            "queued" | "accepted" | "dispatching" | "staging_requested"
        )
    }) {
        return decision(
            "waiting_for_staging",
            None,
            Some("waiting for artifact staging job results"),
            Vec::new(),
        );
    }
    decision(
        "manual_review",
        None,
        Some("rollout target statuses do not match an automated transition"),
        Vec::new(),
    )
}

fn targets_with_status(targets: &[RolloutTargetState], statuses: &[&str]) -> Vec<String> {
    let mut selected = targets
        .iter()
        .filter(|target| statuses.contains(&target.status.as_str()))
        .map(|target| target.client_id.clone())
        .collect::<Vec<_>>();
    selected.sort();
    selected.dedup();
    selected
}

fn rollout_batch_size(canary_count: i32, eligible_count: usize) -> usize {
    if canary_count > 0 {
        (canary_count as usize).clamp(1, eligible_count)
    } else {
        eligible_count
    }
}

fn decision(
    status: &str,
    next_action: Option<&str>,
    blocker: Option<&str>,
    targets: Vec<String>,
) -> RolloutAutomationDecision {
    RolloutAutomationDecision {
        status: status.to_string(),
        next_action: next_action.map(str::to_string),
        blocker: blocker.map(str::to_string),
        targets,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        plan_rollout_automation, RolloutControlState, RolloutTargetState, PROOF_DELEGATION_BLOCKER,
    };

    fn target(client_id: &str, status: &str) -> RolloutTargetState {
        RolloutTargetState {
            client_id: client_id.to_string(),
            status: status.to_string(),
        }
    }

    fn control(canary_count: i32) -> RolloutControlState {
        RolloutControlState {
            canary_count,
            paused: false,
            pause_reason: None,
            health_gate: "heartbeat_verified".to_string(),
        }
    }

    #[test]
    fn staged_rollout_recommends_canary_batch() {
        let decision = plan_rollout_automation(
            &control(2),
            &[
                target("client-c", "completed"),
                target("client-a", "completed"),
                target("client-b", "completed"),
            ],
        );

        assert_eq!(decision.status, "ready_activate_canary");
        assert_eq!(
            decision.next_action.as_deref(),
            Some("operator_activate_batch")
        );
        assert_eq!(decision.blocker.as_deref(), Some(PROOF_DELEGATION_BLOCKER));
        assert_eq!(decision.targets, ["client-a", "client-b"]);
    }

    #[test]
    fn verified_canary_recommends_next_staged_batch() {
        let decision = plan_rollout_automation(
            &control(1),
            &[
                target("client-a", "heartbeat_verified"),
                target("client-b", "completed"),
                target("client-c", "completed"),
            ],
        );

        assert_eq!(decision.status, "ready_activate_next_batch");
        assert_eq!(decision.targets, ["client-b"]);
    }

    #[test]
    fn heartbeat_timeout_recommends_rollback_only_for_timed_out_targets() {
        let decision = plan_rollout_automation(
            &control(1),
            &[
                target("client-a", "heartbeat_timeout"),
                target("client-b", "heartbeat_verified"),
                target("client-c", "completed"),
            ],
        );

        assert_eq!(decision.status, "rollback_required");
        assert_eq!(
            decision.next_action.as_deref(),
            Some("operator_rollback_targets")
        );
        assert_eq!(decision.targets, ["client-a"]);
    }

    #[test]
    fn activation_failure_recommends_rollback_without_unlocking_staged_targets() {
        let decision = plan_rollout_automation(
            &control(2),
            &[
                target("client-a", "activation_failed"),
                target("client-b", "heartbeat_timeout"),
                target("client-c", "completed"),
            ],
        );

        assert_eq!(decision.status, "rollback_required");
        assert_eq!(
            decision.next_action.as_deref(),
            Some("operator_rollback_targets")
        );
        assert_eq!(decision.blocker.as_deref(), Some(PROOF_DELEGATION_BLOCKER));
        assert_eq!(decision.targets, ["client-a", "client-b"]);
    }

    #[test]
    fn staging_failure_blocks_activation_even_when_other_targets_completed() {
        let decision = plan_rollout_automation(
            &control(2),
            &[
                target("client-a", "completed"),
                target("client-b", "dispatch_failed"),
                target("client-c", "completed"),
            ],
        );

        assert_eq!(decision.status, "staging_failed");
        assert_eq!(decision.next_action, None);
        assert_eq!(
            decision.blocker.as_deref(),
            Some("staging failed before activation; inspect target job output")
        );
        assert!(decision.targets.is_empty());
    }

    #[test]
    fn pending_restart_waits_for_heartbeat() {
        let decision = plan_rollout_automation(
            &control(1),
            &[
                target("client-a", "activation_pending_restart"),
                target("client-b", "completed"),
            ],
        );

        assert_eq!(decision.status, "waiting_for_heartbeat");
        assert_eq!(decision.next_action, None);
        assert!(decision.targets.is_empty());
    }

    #[test]
    fn all_verified_is_complete() {
        let decision = plan_rollout_automation(
            &control(1),
            &[
                target("client-a", "heartbeat_verified"),
                target("client-b", "heartbeat_verified"),
            ],
        );

        assert_eq!(decision.status, "complete");
        assert_eq!(decision.next_action, None);
        assert!(decision.targets.is_empty());
    }

    #[test]
    fn paused_rollout_stops_recommendations_with_reason() {
        let decision = plan_rollout_automation(
            &RolloutControlState {
                canary_count: 1,
                paused: true,
                pause_reason: Some("operator maintenance window".to_string()),
                health_gate: "heartbeat_verified".to_string(),
            },
            &[target("client-a", "completed")],
        );

        assert_eq!(decision.status, "paused");
        assert_eq!(decision.next_action, None);
        assert_eq!(
            decision.blocker.as_deref(),
            Some("operator maintenance window")
        );
        assert!(decision.targets.is_empty());
    }

    #[test]
    fn manual_after_canary_gate_blocks_next_batch_recommendation() {
        let decision = plan_rollout_automation(
            &RolloutControlState {
                canary_count: 1,
                paused: false,
                pause_reason: None,
                health_gate: "manual_after_canary".to_string(),
            },
            &[
                target("client-a", "heartbeat_verified"),
                target("client-b", "completed"),
            ],
        );

        assert_eq!(decision.status, "manual_review");
        assert_eq!(decision.next_action, None);
        assert!(decision
            .blocker
            .as_deref()
            .unwrap()
            .contains("manual_after_canary"));
        assert!(decision.targets.is_empty());
    }

    #[test]
    fn manual_only_gate_blocks_all_recommendations() {
        let decision = plan_rollout_automation(
            &RolloutControlState {
                canary_count: 1,
                paused: false,
                pause_reason: None,
                health_gate: "manual_only".to_string(),
            },
            &[target("client-a", "completed")],
        );

        assert_eq!(decision.status, "manual_review");
        assert_eq!(decision.next_action, None);
        assert!(decision.blocker.as_deref().unwrap().contains("manual_only"));
        assert!(decision.targets.is_empty());
    }
}
