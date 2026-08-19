use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgRow, types::Json as SqlJson, PgPool, Postgres, Row, Transaction};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashSet},
};
use uuid::Uuid;
use vpsman_common::{
    is_terminal_command_type, is_terminal_session_event, payload_hash, terminal_session_state,
    TerminalControlAck, TerminalControlAction, TerminalStreamOutput,
    MAX_TERMINAL_FLOW_WINDOW_BYTES,
};
use vpsman_server_core::target_status_is_active;

use crate::{
    auth_model::AuthContext,
    model::{AuditLogView, JobOutputView},
    model_terminal::{
        TerminalOutputChunkRecord, TerminalReplayChunkView, TerminalReplayView, TerminalSessionView,
    },
    repository::Repository,
    repository_job_outputs::JobOutputWriteResult,
    repository_jobs::{
        enqueue_target_terminal_event_in_tx, finish_jobs_in_tx_and_reconcile_event_sources,
    },
    ApiError, TargetDispatchOutcome,
};

fn terminal_session_target_outcome(
    session_state: &str,
    target_status: &str,
    exit_code: Option<i32>,
    received_at: Option<String>,
) -> TargetDispatchOutcome {
    TargetDispatchOutcome {
        status: target_status.to_string(),
        exit_code,
        #[cfg(test)]
        command_version: None,
        accepted: target_status == "completed",
        message: format!("terminal session {session_state}"),
        received_at,
        outputs: Vec::new(),
    }
}

fn terminal_job_statuses(session_state: &str) -> Result<(&'static str, &'static str, bool)> {
    match session_state {
        "opening" | "open" => Ok(("running", "running", false)),
        "closed" | "exited" => Ok(("completed", "completed", true)),
        "rejected" => Ok(("rejected", "rejected", true)),
        "missing" | "failed" => Ok(("failed", "failed", true)),
        _ => anyhow::bail!("terminal_session_state_invalid:{session_state}"),
    }
}

#[derive(Debug)]
enum TerminalReconcileDisposition {
    Applied { terminal_transitioned: bool },
    StaleNonterminal { authoritative_status: String },
}

fn terminal_session_state_for_terminal_status(status: &str) -> (&'static str, &'static str) {
    match status {
        "completed" | "partial_success" => ("closed", "closed"),
        "rejected" | "skipped" => ("rejected", "rejected"),
        _ => ("failed", "failed"),
    }
}

fn memory_terminal_reconcile_disposition(
    job: &crate::model::JobHistoryView,
    target: &crate::model::JobTargetView,
    job_status: &str,
    target_status: &str,
    terminal: bool,
) -> Result<Option<String>> {
    let target_terminal = target.completed_at.is_some() || !target_status_is_active(&target.status);
    let job_terminal =
        job.completed_at.is_some() || !matches!(job.status.as_str(), "queued" | "running");
    if !terminal && (target_terminal || job_terminal) {
        return Ok(Some(if target_terminal {
            target.status.clone()
        } else {
            job.status.clone()
        }));
    }
    if terminal && target_terminal {
        anyhow::ensure!(
            target.status == target_status,
            "terminal_open_target_terminal_state_conflict"
        );
    }
    if terminal && job_terminal {
        anyhow::ensure!(
            job.status == job_status,
            "terminal_open_job_terminal_state_conflict"
        );
    }
    Ok(None)
}

fn reconcile_memory_terminal_job_locked(
    jobs: &mut [crate::model::JobHistoryView],
    targets: &mut [crate::model::JobTargetView],
    job_id: Uuid,
    client_id: &str,
    session_state: &str,
    now: &str,
) -> Result<TerminalReconcileDisposition> {
    let (job_status, target_status, terminal) = terminal_job_statuses(session_state)?;
    let job_index = jobs
        .iter()
        .position(|job| job.id == job_id && job.command_type == "terminal_open")
        .context("terminal_open_job_not_found")?;
    let target_index = targets
        .iter()
        .position(|target| target.job_id == job_id && target.client_id == client_id)
        .context("terminal_open_target_not_found")?;
    if let Some(authoritative_status) = memory_terminal_reconcile_disposition(
        &jobs[job_index],
        &targets[target_index],
        job_status,
        target_status,
        terminal,
    )? {
        return Ok(TerminalReconcileDisposition::StaleNonterminal {
            authoritative_status,
        });
    }

    let terminal_transitioned = terminal && jobs[job_index].completed_at.is_none();
    let target = &mut targets[target_index];
    target.status = target_status.to_string();
    if terminal {
        if target.completed_at.is_none() {
            target.completed_at = Some(now.to_string());
        }
    } else {
        target.completed_at = None;
        target.exit_code = None;
        target.deadline_at = None;
    }

    let job = &mut jobs[job_index];
    job.status = job_status.to_string();
    if terminal {
        if job.completed_at.is_none() {
            job.completed_at = Some(now.to_string());
        }
    } else {
        job.completed_at = None;
    }
    Ok(TerminalReconcileDisposition::Applied {
        terminal_transitioned,
    })
}

fn normalize_memory_stale_terminal_session(
    sessions: &mut [TerminalSessionView],
    job_id: Uuid,
    client_id: &str,
    authoritative_status: &str,
    observed_at: &str,
) {
    let (state, last_status) = terminal_session_state_for_terminal_status(authoritative_status);
    if let Some(session) = sessions.iter_mut().find(|session| {
        session.job_id == job_id
            && session.client_id == client_id
            && matches!(session.state.as_str(), "opening" | "open")
    }) {
        session.state = state.to_string();
        session.last_status = last_status.to_string();
        session.last_event = "terminal_stream".to_string();
        session.close_reason = session
            .close_reason
            .take()
            .or_else(|| Some(format!("job_target_{authoritative_status}")));
        session.observed_at = observed_at.to_string();
    }
}

#[derive(Debug)]
struct LockedPostgresTerminalJob {
    target_status: String,
    target_completed_at: Option<String>,
    exit_code: Option<i32>,
    result_received_at: Option<String>,
    job_status: String,
    job_completed_at: Option<String>,
}

async fn lock_postgres_terminal_job_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
) -> Result<LockedPostgresTerminalJob> {
    let target_row = sqlx::query(
        r#"
        SELECT
            status,
            completed_at::text AS completed_at,
            exit_code,
            result_received_at::text AS result_received_at
        FROM job_targets
        WHERE job_id = $1 AND client_id = $2
        FOR UPDATE
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_optional(&mut **tx)
    .await?
    .context("terminal_open_target_not_found")?;
    let job_row = sqlx::query(
        r#"
        SELECT status, completed_at::text AS completed_at
        FROM jobs
        WHERE id = $1 AND command_type = 'terminal_open'
        FOR NO KEY UPDATE
        "#,
    )
    .bind(job_id)
    .fetch_optional(&mut **tx)
    .await?
    .context("terminal_open_job_not_found")?;
    Ok(LockedPostgresTerminalJob {
        target_status: target_row.try_get("status")?,
        target_completed_at: target_row.try_get("completed_at")?,
        exit_code: target_row.try_get("exit_code")?,
        result_received_at: target_row.try_get("result_received_at")?,
        job_status: job_row.try_get("status")?,
        job_completed_at: job_row.try_get("completed_at")?,
    })
}

fn postgres_terminal_reconcile_disposition(
    locked: &LockedPostgresTerminalJob,
    session_state: &str,
) -> Result<Option<String>> {
    let (job_status, target_status, terminal) = terminal_job_statuses(session_state)?;
    let target_terminal =
        locked.target_completed_at.is_some() || !target_status_is_active(&locked.target_status);
    let job_terminal = locked.job_completed_at.is_some()
        || !matches!(locked.job_status.as_str(), "queued" | "running");
    if !terminal && (target_terminal || job_terminal) {
        return Ok(Some(if target_terminal {
            locked.target_status.clone()
        } else {
            locked.job_status.clone()
        }));
    }
    if terminal && target_terminal {
        anyhow::ensure!(
            locked.target_status == target_status,
            "terminal_open_target_terminal_state_conflict"
        );
    }
    if terminal && job_terminal {
        anyhow::ensure!(
            locked.job_status == job_status,
            "terminal_open_job_terminal_state_conflict"
        );
    }
    Ok(None)
}

async fn normalize_postgres_stale_terminal_session_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
    authoritative_status: &str,
) -> Result<()> {
    let (state, last_status) = terminal_session_state_for_terminal_status(authoritative_status);
    sqlx::query(
        r#"
        UPDATE terminal_sessions
        SET state = $3,
            last_status = $4,
            last_event = 'terminal_stream',
            close_reason = COALESCE(close_reason, $5),
            observed_at = now()
        WHERE job_id = $1
          AND client_id = $2
          AND state IN ('opening', 'open')
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(state)
    .bind(last_status)
    .bind(format!("job_target_{authoritative_status}"))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn terminal_control_audit_id(
    client_id: &str,
    job_id: Uuid,
    session_id: Uuid,
    request_id: Uuid,
) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(b"vpsman:terminal-control-audit:v1\0");
    digest.update(client_id.as_bytes());
    digest.update(b"\0");
    digest.update(job_id.as_bytes());
    digest.update(session_id.as_bytes());
    digest.update(request_id.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn terminal_control_audit(
    operator: &AuthContext,
    client_id: &str,
    job_id: Uuid,
    action: &TerminalControlAction,
    action_hash: &str,
    ack: &TerminalControlAck,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: terminal_control_audit_id(client_id, job_id, ack.session_id, ack.request_id),
        actor_id: Some(operator.operator.id),
        action: format!("terminal.{}", action.kind()),
        target: format!("terminal:{client_id}:{}", ack.session_id),
        command_hash: Some(action_hash.to_string()),
        metadata: serde_json::json!({
            "request_id": ack.request_id,
            "terminal_session_id": ack.session_id,
            "job_id": job_id,
            "client_id": client_id,
            "action": action.kind(),
            "accepted": ack.accepted,
            "status": ack.status,
            "result": if ack.accepted { "accepted" } else { "rejected" },
            "message": ack.message,
            "input_seq": ack.input_seq,
            "written_bytes": ack.written_bytes,
            "cols": ack.cols,
            "rows": ack.rows,
            "operator_id": operator.operator.id,
            "operator_username": &operator.operator.username,
            "operator_role": &operator.operator.role,
            "operator_session_id": operator.audit_session_id(),
            "origin_kind": "operator_request",
            "component": "terminal-controller",
        }),
        created_at,
    }
}

fn terminal_control_audits_match(left: &AuditLogView, right: &AuditLogView) -> bool {
    left.actor_id == right.actor_id
        && left.action == right.action
        && left.target == right.target
        && left.command_hash == right.command_hash
        && left.metadata == right.metadata
}

async fn insert_postgres_terminal_control_audit_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    audit: &AuditLogView,
) -> Result<bool> {
    let inserted = sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata, created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7::timestamptz)
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(audit.id)
    .bind(audit.actor_id)
    .bind(&audit.action)
    .bind(&audit.target)
    .bind(&audit.command_hash)
    .bind(SqlJson(&audit.metadata))
    .bind(&audit.created_at)
    .execute(&mut **tx)
    .await?;
    if inserted.rows_affected() == 0 {
        let row = sqlx::query(
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
            actor_id: row.try_get("actor_id")?,
            action: row.try_get("action")?,
            target: row.try_get("target")?,
            command_hash: row.try_get("command_hash")?,
            metadata: row.try_get("metadata")?,
            created_at: audit.created_at.clone(),
        };
        anyhow::ensure!(
            terminal_control_audits_match(&existing, audit),
            "terminal_control_audit_identity_conflict"
        );
        return Ok(false);
    }
    Ok(true)
}

async fn apply_postgres_terminal_control_ack_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    job_id: Uuid,
    action: &TerminalControlAction,
    ack: &TerminalControlAck,
    input_seq: Option<u64>,
) -> Result<()> {
    let result = if ack.accepted {
        match action {
            TerminalControlAction::Input { .. } => {
                let input_seq = input_seq.expect("accepted input acknowledgement was validated");
                sqlx::query(
                    r#"
                    UPDATE terminal_sessions
                    SET last_input_seq = GREATEST(last_input_seq, $4),
                        last_status = $5,
                        last_event = 'terminal_input',
                        observed_at = now()
                    WHERE client_id = $1 AND session_id = $2 AND job_id = $3
                    "#,
                )
                .bind(client_id)
                .bind(ack.session_id)
                .bind(job_id)
                .bind(i64::try_from(input_seq).unwrap_or(i64::MAX))
                .bind(&ack.status)
                .execute(&mut **tx)
                .await?
            }
            TerminalControlAction::Resize { cols, rows } => {
                sqlx::query(
                    r#"
                UPDATE terminal_sessions
                SET cols = $4,
                    rows = $5,
                    last_status = $6,
                    last_event = 'terminal_resize',
                    observed_at = now()
                WHERE client_id = $1 AND session_id = $2 AND job_id = $3
                "#,
                )
                .bind(client_id)
                .bind(ack.session_id)
                .bind(job_id)
                .bind(i64::from(*cols))
                .bind(i64::from(*rows))
                .bind(&ack.status)
                .execute(&mut **tx)
                .await?
            }
            TerminalControlAction::Close { reason } => {
                sqlx::query(
                    r#"
                UPDATE terminal_sessions
                SET state = 'closed',
                    last_status = $4,
                    last_event = 'terminal_close',
                    close_reason = COALESCE($5, 'operator'),
                    observed_at = now()
                WHERE client_id = $1 AND session_id = $2 AND job_id = $3
                "#,
                )
                .bind(client_id)
                .bind(ack.session_id)
                .bind(job_id)
                .bind(&ack.status)
                .bind(reason)
                .execute(&mut **tx)
                .await?
            }
        }
    } else if matches!(ack.status.as_str(), "missing" | "failed" | "exited") {
        sqlx::query(
            r#"
            UPDATE terminal_sessions
            SET state = $4,
                last_status = $4,
                last_event = $5,
                close_reason = COALESCE(close_reason, $6),
                observed_at = now()
            WHERE client_id = $1 AND session_id = $2 AND job_id = $3
            "#,
        )
        .bind(client_id)
        .bind(ack.session_id)
        .bind(job_id)
        .bind(&ack.status)
        .bind(format!("terminal_{}", action.kind()))
        .bind(&ack.message)
        .execute(&mut **tx)
        .await?
    } else {
        return Ok(());
    };
    anyhow::ensure!(result.rows_affected() == 1, "terminal_session_not_found");
    Ok(())
}

fn apply_memory_terminal_control_ack(
    session: &mut TerminalSessionView,
    action: &TerminalControlAction,
    ack: &TerminalControlAck,
    input_seq: Option<u64>,
    observed_at: &str,
) {
    if ack.accepted {
        session.last_status = ack.status.clone();
        session.last_event = format!("terminal_{}", action.kind());
        session.observed_at = observed_at.to_string();
        match action {
            TerminalControlAction::Input { .. } => {
                let input_seq = input_seq.expect("accepted input acknowledgement was validated");
                session.last_input_seq = session
                    .last_input_seq
                    .max(i64::try_from(input_seq).unwrap_or(i64::MAX));
            }
            TerminalControlAction::Resize { cols, rows } => {
                session.cols = Some(i64::from(*cols));
                session.rows = Some(i64::from(*rows));
            }
            TerminalControlAction::Close { reason } => {
                session.state = "closed".to_string();
                session.close_reason = reason.clone().or_else(|| Some("operator".to_string()));
            }
        }
    } else if matches!(ack.status.as_str(), "missing" | "failed" | "exited") {
        session.state = ack.status.clone();
        session.last_status = ack.status.clone();
        session.last_event = format!("terminal_{}", action.kind());
        session.close_reason = Some(ack.message.clone());
        session.observed_at = observed_at.to_string();
    }
}

async fn validate_postgres_terminal_control_target_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
    terminal_state: Option<&str>,
) -> Result<()> {
    let locked = lock_postgres_terminal_job_in_tx(tx, job_id, client_id).await?;
    if let Some(terminal_state) = terminal_state {
        anyhow::ensure!(
            postgres_terminal_reconcile_disposition(&locked, terminal_state)?.is_none(),
            "terminal_session_job_inactive"
        );
    } else {
        anyhow::ensure!(
            locked.target_completed_at.is_none()
                && target_status_is_active(&locked.target_status)
                && locked.job_completed_at.is_none()
                && matches!(locked.job_status.as_str(), "queued" | "running"),
            "terminal_session_job_inactive"
        );
    }
    Ok(())
}

async fn reconcile_postgres_terminal_job_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
    session_state: &str,
) -> Result<TerminalReconcileDisposition> {
    let (job_status, target_status, terminal) = terminal_job_statuses(session_state)?;
    let locked = lock_postgres_terminal_job_in_tx(tx, job_id, client_id).await?;
    if let Some(authoritative_status) =
        postgres_terminal_reconcile_disposition(&locked, session_state)?
    {
        normalize_postgres_stale_terminal_session_in_tx(
            tx,
            job_id,
            client_id,
            &authoritative_status,
        )
        .await?;
        return Ok(TerminalReconcileDisposition::StaleNonterminal {
            authoritative_status,
        });
    }

    let target_result = sqlx::query(
        r#"
        UPDATE job_targets
        SET status = $3,
            completed_at = CASE WHEN $4 THEN COALESCE(completed_at, now()) ELSE NULL END,
            exit_code = CASE WHEN $4 THEN exit_code ELSE NULL END,
            deadline_at = CASE WHEN $4 THEN deadline_at ELSE NULL END,
            dispatch_lease_until = NULL
        WHERE job_id = $1 AND client_id = $2
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(target_status)
    .bind(terminal)
    .execute(&mut **tx)
    .await?;
    anyhow::ensure!(
        target_result.rows_affected() == 1,
        "terminal_open_target_not_found"
    );
    if terminal {
        let outcome = terminal_session_target_outcome(
            session_state,
            target_status,
            locked.exit_code,
            locked.result_received_at,
        );
        enqueue_target_terminal_event_in_tx(tx, job_id, client_id, &outcome).await?;
        finish_jobs_in_tx_and_reconcile_event_sources(tx, &[job_id]).await?;
    } else {
        let job_result = sqlx::query(
            r#"
            UPDATE jobs
            SET status = $2,
                completed_at = NULL
            WHERE id = $1 AND command_type = 'terminal_open'
            "#,
        )
        .bind(job_id)
        .bind(job_status)
        .execute(&mut **tx)
        .await?;
        anyhow::ensure!(
            job_result.rows_affected() == 1,
            "terminal_open_job_not_found"
        );
    }
    Ok(TerminalReconcileDisposition::Applied {
        terminal_transitioned: terminal && locked.job_completed_at.is_none(),
    })
}

impl Repository {
    pub(crate) async fn list_terminal_sessions(
        &self,
        limit: i64,
        client_id: Option<&str>,
        session_id: Option<Uuid>,
    ) -> Result<Vec<TerminalSessionView>> {
        let limit = limit.clamp(1, 200);
        match self {
            Self::Memory(memory) => {
                let command_types = memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .map(|job| (job.id, job.command_type.clone()))
                    .collect::<BTreeMap<_, _>>();
                let mut outputs = memory
                    .job_outputs
                    .read()
                    .await
                    .iter()
                    .filter_map(|output| {
                        if output.stream != "status" {
                            return None;
                        }
                        if let Some(client_id) = client_id {
                            if output.client_id != client_id {
                                return None;
                            }
                        }
                        let command_type = command_types.get(&output.job_id)?;
                        if !is_terminal_command(command_type) {
                            return None;
                        }
                        Some(TerminalStatusOutput {
                            job_id: output.job_id,
                            client_id: output.client_id.clone(),
                            seq: output.seq,
                            data: BASE64.decode(&output.data_base64).ok()?,
                            created_at: output.created_at.clone(),
                        })
                    })
                    .collect::<Vec<_>>();
                sort_terminal_outputs_newest(&mut outputs)?;
                let mut sessions = build_terminal_sessions(outputs, limit, session_id)?;
                sessions.extend(
                    memory
                        .terminal_sessions
                        .read()
                        .await
                        .iter()
                        .filter(|session| {
                            client_id.is_none_or(|client_id| session.client_id == client_id)
                                && session_id
                                    .is_none_or(|session_id| session.session_id == session_id)
                        })
                        .cloned(),
                );
                deduplicate_terminal_sessions(sessions, limit)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        session_id,
                        client_id,
                        job_id,
                        state,
                        last_status,
                        argv,
                        cwd,
                        cols,
                        rows,
                        idle_timeout_secs,
                        flow_window_bytes,
                        output_first_seq,
                        output_next_seq,
                        output_retained_first_seq,
                        output_retained_bytes,
                        output_dropped_bytes,
                        output_dropped_chunks,
                        output_replay_truncated,
                        last_input_seq,
                        close_reason,
                        last_event,
                        opened_at::text AS opened_at,
                        observed_at::text AS observed_at
                    FROM terminal_sessions
                    WHERE ($2::text IS NULL OR client_id = $2)
                      AND ($3::uuid IS NULL OR session_id = $3)
                    ORDER BY observed_at DESC, client_id ASC, session_id ASC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .bind(client_id)
                .bind(session_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(terminal_session_from_row)
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }

    pub(crate) async fn authorize_terminal_control(
        &self,
        client_id: &str,
        session_id: Uuid,
        operator: &AuthContext,
    ) -> std::result::Result<Uuid, ApiError> {
        match self {
            Self::Memory(memory) => {
                let sessions = memory.terminal_sessions.read().await;
                let session = sessions
                    .iter()
                    .find(|session| {
                        session.client_id == client_id && session.session_id == session_id
                    })
                    .ok_or_else(|| ApiError::not_found("terminal_session_not_found"))?;
                if session.state != "open" {
                    return Err(ApiError::conflict("terminal_session_not_open"));
                }
                let job_id = session.job_id;
                drop(sessions);
                let jobs = memory.jobs.read().await;
                let job = jobs
                    .iter()
                    .find(|job| job.id == job_id && job.command_type == "terminal_open")
                    .ok_or_else(|| ApiError::conflict("terminal_session_job_invalid"))?;
                if job.actor_id != Some(operator.operator.id) {
                    return Err(ApiError::forbidden("terminal_session_not_owned"));
                }
                drop(jobs);
                let target_active = memory.job_targets.read().await.iter().any(|target| {
                    target.job_id == job_id
                        && target.client_id == client_id
                        && target.completed_at.is_none()
                        && target_status_is_active(&target.status)
                });
                if !target_active {
                    return Err(ApiError::conflict("terminal_session_job_inactive"));
                }
                Ok(job_id)
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        session.state,
                        session.job_id,
                        job.actor_id,
                        target.status AS target_status,
                        target.completed_at::text AS target_completed_at
                    FROM terminal_sessions session
                    JOIN jobs job ON job.id = session.job_id
                    LEFT JOIN job_targets target
                      ON target.job_id = session.job_id
                     AND target.client_id = session.client_id
                    WHERE session.client_id = $1
                      AND session.session_id = $2
                      AND job.command_type = 'terminal_open'
                    "#,
                )
                .bind(client_id)
                .bind(session_id)
                .fetch_optional(pool)
                .await
                .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                let Some(row) = row else {
                    return Err(ApiError::not_found("terminal_session_not_found"));
                };
                let state: String = row
                    .try_get("state")
                    .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                if state != "open" {
                    return Err(ApiError::conflict("terminal_session_not_open"));
                }
                let actor_id: Option<Uuid> = row
                    .try_get("actor_id")
                    .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                if actor_id != Some(operator.operator.id) {
                    return Err(ApiError::forbidden("terminal_session_not_owned"));
                }
                let target_status: Option<String> = row
                    .try_get("target_status")
                    .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                let target_completed_at: Option<String> = row
                    .try_get("target_completed_at")
                    .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
                if target_completed_at.is_some()
                    || target_status
                        .as_deref()
                        .is_none_or(|status| !target_status_is_active(status))
                {
                    return Err(ApiError::conflict("terminal_session_job_inactive"));
                }
                row.try_get("job_id")
                    .map_err(|error| ApiError::from(anyhow::Error::from(error)))
            }
        }
    }

    pub(crate) async fn record_terminal_control_ack(
        &self,
        operator: &AuthContext,
        client_id: &str,
        job_id: Uuid,
        action: &TerminalControlAction,
        action_hash: &str,
        ack: &TerminalControlAck,
    ) -> Result<()> {
        let now = now_rfc3339();
        let input_seq = if ack.accepted && matches!(action, TerminalControlAction::Input { .. }) {
            Some(ack.input_seq.context("terminal_input_ack_missing_seq")?)
        } else {
            None
        };
        let lifecycle_event = matches!(action, TerminalControlAction::Close { .. })
            || (!ack.accepted && matches!(ack.status.as_str(), "missing" | "failed" | "exited"));
        let state = if ack.accepted && matches!(action, TerminalControlAction::Close { .. }) {
            Some("closed")
        } else if !ack.accepted && matches!(ack.status.as_str(), "missing" | "failed" | "exited") {
            Some(ack.status.as_str())
        } else {
            None
        };
        let audit = lifecycle_event.then(|| {
            terminal_control_audit(
                operator,
                client_id,
                job_id,
                action,
                action_hash,
                ack,
                now.clone(),
            )
        });

        match self {
            Self::Memory(memory) => {
                if !ack.accepted && audit.is_none() && state.is_none() {
                    return Ok(());
                }
                let terminal_transitioned = {
                    let _terminal_guard = memory.job_terminal_side_effects.lock().await;
                    // Memory's established overlapping core order is jobs -> targets. Keep the
                    // terminal session and audit guards after those two so output completion and
                    // control acknowledgement cannot form an AB/BA cycle.
                    let mut jobs = memory.jobs.write().await;
                    let mut targets = memory.job_targets.write().await;
                    let mut sessions = memory.terminal_sessions.write().await;
                    let mut audits = memory.audits.write().await;

                    if let Some(audit) = audit.as_ref() {
                        if let Some(existing) =
                            audits.iter().find(|existing| existing.id == audit.id)
                        {
                            anyhow::ensure!(
                                terminal_control_audits_match(existing, audit),
                                "terminal_control_audit_identity_conflict"
                            );
                            return Ok(());
                        }
                    }

                    let job = jobs
                        .iter()
                        .find(|job| job.id == job_id && job.command_type == "terminal_open")
                        .context("terminal_open_job_not_found")?;
                    let target = targets
                        .iter()
                        .find(|target| target.job_id == job_id && target.client_id == client_id)
                        .context("terminal_open_target_not_found")?;
                    let session_index = sessions
                        .iter()
                        .position(|session| {
                            session.client_id == client_id
                                && session.session_id == ack.session_id
                                && session.job_id == job_id
                        })
                        .context("terminal_session_not_found")?;

                    if let Some(state) = state {
                        let (job_status, target_status, terminal) = terminal_job_statuses(state)?;
                        anyhow::ensure!(
                            memory_terminal_reconcile_disposition(
                                job,
                                target,
                                job_status,
                                target_status,
                                terminal,
                            )?
                            .is_none(),
                            "terminal_session_job_inactive"
                        );
                    } else {
                        anyhow::ensure!(
                            job.completed_at.is_none()
                                && matches!(job.status.as_str(), "queued" | "running")
                                && target.completed_at.is_none()
                                && target_status_is_active(&target.status),
                            "terminal_session_job_inactive"
                        );
                    }

                    let mut next_session = sessions[session_index].clone();
                    apply_memory_terminal_control_ack(
                        &mut next_session,
                        action,
                        ack,
                        input_seq,
                        &now,
                    );
                    let terminal_transitioned = if let Some(state) = state {
                        match reconcile_memory_terminal_job_locked(
                            &mut jobs,
                            &mut targets,
                            job_id,
                            client_id,
                            state,
                            &now,
                        )? {
                            TerminalReconcileDisposition::Applied {
                                terminal_transitioned,
                            } => terminal_transitioned,
                            TerminalReconcileDisposition::StaleNonterminal { .. } => false,
                        }
                    } else {
                        false
                    };
                    sessions[session_index] = next_session;
                    if let Some(audit) = audit {
                        audits.push(audit);
                    }
                    terminal_transitioned
                };
                if terminal_transitioned {
                    self.reconcile_memory_job_event_sources(job_id).await?;
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                if let Some(audit) = audit.as_ref() {
                    if !insert_postgres_terminal_control_audit_in_tx(&mut tx, audit).await? {
                        tx.commit().await?;
                        return Ok(());
                    }
                }
                if ack.accepted || audit.is_some() || state.is_some() {
                    validate_postgres_terminal_control_target_in_tx(
                        &mut tx, job_id, client_id, state,
                    )
                    .await?;
                }
                apply_postgres_terminal_control_ack_in_tx(
                    &mut tx, client_id, job_id, action, ack, input_seq,
                )
                .await?;
                if let Some(state) = state {
                    reconcile_postgres_terminal_job_in_tx(&mut tx, job_id, client_id, state)
                        .await?;
                }
                tx.commit().await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn reconcile_terminal_job(
        &self,
        job_id: Uuid,
        client_id: &str,
        session_state: &str,
    ) -> Result<()> {
        let memory_terminal_transitioned = match self {
            Self::Memory(memory) => {
                let now = now_rfc3339();
                let _terminal_guard = memory.job_terminal_side_effects.lock().await;
                let mut jobs = memory.jobs.write().await;
                let mut targets = memory.job_targets.write().await;
                match reconcile_memory_terminal_job_locked(
                    &mut jobs,
                    &mut targets,
                    job_id,
                    client_id,
                    session_state,
                    &now,
                )? {
                    TerminalReconcileDisposition::Applied {
                        terminal_transitioned,
                    } => terminal_transitioned,
                    TerminalReconcileDisposition::StaleNonterminal {
                        authoritative_status,
                    } => {
                        let mut sessions = memory.terminal_sessions.write().await;
                        normalize_memory_stale_terminal_session(
                            &mut sessions,
                            job_id,
                            client_id,
                            &authoritative_status,
                            &now,
                        );
                        false
                    }
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                reconcile_postgres_terminal_job_in_tx(&mut tx, job_id, client_id, session_state)
                    .await?;
                tx.commit().await?;
                false
            }
        };
        if memory_terminal_transitioned {
            self.reconcile_memory_job_event_sources(job_id).await?;
        }
        Ok(())
    }

    pub(crate) async fn reconcile_terminal_job_by_id(&self, job_id: Uuid) -> Result<()> {
        let mut session = self.terminal_job_session_state(job_id).await?;
        let mut core_already_reconciled = false;
        if session.is_none() {
            match self {
                Self::Memory(memory) => {
                    if let Some(view) = self
                        .list_terminal_sessions(200, None, None)
                        .await?
                        .into_iter()
                        .find(|session| session.job_id == job_id)
                    {
                        let now = now_rfc3339();
                        let (effective_state, terminal_transitioned) = {
                            let _terminal_guard = memory.job_terminal_side_effects.lock().await;
                            let mut jobs = memory.jobs.write().await;
                            let mut targets = memory.job_targets.write().await;
                            let mut sessions = memory.terminal_sessions.write().await;
                            let mut next_sessions = sessions.clone();
                            upsert_memory_terminal_session(&mut next_sessions, view.clone())?;
                            let session_index = next_sessions
                                .iter()
                                .position(|session| {
                                    session.client_id == view.client_id
                                        && session.session_id == view.session_id
                                })
                                .context("terminal_session_missing_after_upsert")?;
                            let effective_state = next_sessions[session_index].state.clone();
                            let terminal_transitioned = match reconcile_memory_terminal_job_locked(
                                &mut jobs,
                                &mut targets,
                                job_id,
                                &view.client_id,
                                &effective_state,
                                &now,
                            )? {
                                TerminalReconcileDisposition::Applied {
                                    terminal_transitioned,
                                } => terminal_transitioned,
                                TerminalReconcileDisposition::StaleNonterminal {
                                    authoritative_status,
                                } => {
                                    normalize_memory_stale_terminal_session(
                                        &mut next_sessions,
                                        job_id,
                                        &view.client_id,
                                        &authoritative_status,
                                        &now,
                                    );
                                    false
                                }
                            };
                            let effective_state = next_sessions[session_index].state.clone();
                            *sessions = next_sessions;
                            (effective_state, terminal_transitioned)
                        };
                        if terminal_transitioned {
                            self.reconcile_memory_job_event_sources(job_id).await?;
                        }
                        session = Some((view.client_id, effective_state));
                        core_already_reconciled = true;
                    }
                }
                Self::Postgres(pool) => {
                    let client_id: Option<String> = sqlx::query_scalar(
                        r#"
                        SELECT client_id
                        FROM job_targets
                        WHERE job_id = $1
                        ORDER BY client_id
                        LIMIT 1
                        "#,
                    )
                    .bind(job_id)
                    .fetch_optional(pool)
                    .await?;
                    if let Some(client_id) = client_id {
                        self.refresh_terminal_sessions_for_client(&client_id)
                            .await?;
                        session = self.terminal_job_session_state(job_id).await?;
                        core_already_reconciled = true;
                    }
                }
            }
        }
        if let Some((client_id, state)) = session {
            if !core_already_reconciled {
                self.reconcile_terminal_job(job_id, &client_id, &state)
                    .await?;
            }
            let effective_state = self
                .terminal_job_session_state(job_id)
                .await?
                .map(|(_, state)| state);
            if effective_state
                .as_deref()
                .is_some_and(|state| matches!(state, "opening" | "open"))
                && self
                    .terminal_job_agent_incarnation_changed(job_id, &client_id)
                    .await?
            {
                self.mark_terminal_session_missing_and_reconcile(
                    job_id,
                    &client_id,
                    "agent_process_restarted",
                )
                .await?;
                return Ok(());
            }
        }
        Ok(())
    }

    async fn terminal_job_agent_incarnation_changed(
        &self,
        job_id: Uuid,
        client_id: &str,
    ) -> Result<bool> {
        match self {
            Self::Memory(memory) => {
                let expected = memory
                    .job_targets
                    .read()
                    .await
                    .iter()
                    .find(|target| target.job_id == job_id && target.client_id == client_id)
                    .and_then(|target| target.process_incarnation_id);
                let actual = memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .find(|agent| agent.id == client_id)
                    .and_then(|agent| agent.process_incarnation_id);
                Ok(
                    matches!((expected, actual), (Some(expected), Some(actual)) if expected != actual),
                )
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        target.process_incarnation_id AS expected_process_incarnation_id,
                        client.process_incarnation_id AS actual_process_incarnation_id
                    FROM job_targets target
                    JOIN clients client ON client.id = target.client_id
                    WHERE target.job_id = $1 AND target.client_id = $2
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(false);
                };
                let expected: Option<Uuid> = row.try_get("expected_process_incarnation_id")?;
                let actual: Option<Uuid> = row.try_get("actual_process_incarnation_id")?;
                Ok(
                    matches!((expected, actual), (Some(expected), Some(actual)) if expected != actual),
                )
            }
        }
    }

    async fn mark_terminal_session_missing_and_reconcile(
        &self,
        job_id: Uuid,
        client_id: &str,
        reason: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let now = now_rfc3339();
                let terminal_transitioned = {
                    let _terminal_guard = memory.job_terminal_side_effects.lock().await;
                    let mut jobs = memory.jobs.write().await;
                    let mut targets = memory.job_targets.write().await;
                    let mut sessions = memory.terminal_sessions.write().await;
                    let session_index = sessions
                        .iter()
                        .position(|session| {
                            session.job_id == job_id && session.client_id == client_id
                        })
                        .context("terminal_session_not_found")?;
                    let job = jobs
                        .iter()
                        .find(|job| job.id == job_id && job.command_type == "terminal_open")
                        .context("terminal_open_job_not_found")?;
                    let target = targets
                        .iter()
                        .find(|target| target.job_id == job_id && target.client_id == client_id)
                        .context("terminal_open_target_not_found")?;
                    if matches!(sessions[session_index].state.as_str(), "opening" | "open") {
                        if let Some(authoritative_status) = memory_terminal_reconcile_disposition(
                            job, target, "running", "running", false,
                        )? {
                            normalize_memory_stale_terminal_session(
                                &mut sessions,
                                job_id,
                                client_id,
                                &authoritative_status,
                                &now,
                            );
                            false
                        } else {
                            let mut next_session = sessions[session_index].clone();
                            next_session.state = "missing".to_string();
                            next_session.last_status = "missing".to_string();
                            next_session.last_event = "terminal_stream".to_string();
                            next_session.close_reason = Some(reason.to_string());
                            next_session.observed_at = now.clone();
                            let transitioned = match reconcile_memory_terminal_job_locked(
                                &mut jobs,
                                &mut targets,
                                job_id,
                                client_id,
                                "missing",
                                &now,
                            )? {
                                TerminalReconcileDisposition::Applied {
                                    terminal_transitioned,
                                } => terminal_transitioned,
                                TerminalReconcileDisposition::StaleNonterminal { .. } => false,
                            };
                            sessions[session_index] = next_session;
                            transitioned
                        }
                    } else {
                        match reconcile_memory_terminal_job_locked(
                            &mut jobs,
                            &mut targets,
                            job_id,
                            client_id,
                            &sessions[session_index].state,
                            &now,
                        )? {
                            TerminalReconcileDisposition::Applied {
                                terminal_transitioned,
                            } => terminal_transitioned,
                            TerminalReconcileDisposition::StaleNonterminal {
                                authoritative_status,
                            } => {
                                normalize_memory_stale_terminal_session(
                                    &mut sessions,
                                    job_id,
                                    client_id,
                                    &authoritative_status,
                                    &now,
                                );
                                false
                            }
                        }
                    }
                };
                if terminal_transitioned {
                    self.reconcile_memory_job_event_sources(job_id).await?;
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let locked = lock_postgres_terminal_job_in_tx(&mut tx, job_id, client_id).await?;
                if let Some(authoritative_status) =
                    postgres_terminal_reconcile_disposition(&locked, "open")?
                {
                    normalize_postgres_stale_terminal_session_in_tx(
                        &mut tx,
                        job_id,
                        client_id,
                        &authoritative_status,
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(());
                }
                let state: Option<String> = sqlx::query_scalar(
                    r#"
                    UPDATE terminal_sessions
                    SET state = 'missing',
                        last_status = 'missing',
                        last_event = 'terminal_stream',
                        close_reason = $3,
                        observed_at = now()
                    WHERE job_id = $1
                      AND client_id = $2
                      AND state IN ('opening', 'open')
                    RETURNING state
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(reason)
                .fetch_optional(&mut *tx)
                .await?;
                let effective_state = match state {
                    Some(state) => state,
                    None => sqlx::query_scalar(
                        r#"
                        SELECT state
                        FROM terminal_sessions
                        WHERE job_id = $1 AND client_id = $2
                        FOR UPDATE
                        "#,
                    )
                    .bind(job_id)
                    .bind(client_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .context("terminal_session_not_found")?,
                };
                reconcile_postgres_terminal_job_in_tx(&mut tx, job_id, client_id, &effective_state)
                    .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }

    async fn terminal_job_session_state(&self, job_id: Uuid) -> Result<Option<(String, String)>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .terminal_sessions
                .read()
                .await
                .iter()
                .find(|session| session.job_id == job_id)
                .map(|session| (session.client_id.clone(), session.state.clone()))),
            Self::Postgres(pool) => Ok(sqlx::query(
                r#"
                SELECT client_id, state
                FROM terminal_sessions
                WHERE job_id = $1
                "#,
            )
            .bind(job_id)
            .fetch_optional(pool)
            .await?
            .map(|row| Ok::<_, sqlx::Error>((row.try_get("client_id")?, row.try_get("state")?)))
            .transpose()?),
        }
    }

    pub(crate) async fn terminal_session_replay(
        &self,
        client_id: &str,
        session_id: Uuid,
        from_seq: Option<i64>,
        limit: i64,
        max_bytes: i64,
        include_data: bool,
    ) -> Result<TerminalReplayView> {
        let from_seq = from_seq.unwrap_or(1).max(1);
        let limit = limit.clamp(1, 1000);
        let max_bytes = max_bytes.max(1);
        match self {
            Self::Memory(memory) => {
                let mut chunks = memory
                    .terminal_output_chunks
                    .read()
                    .await
                    .iter()
                    .filter(|chunk| {
                        chunk.client_id == client_id
                            && chunk.session_id == session_id
                            && chunk.terminal_seq >= from_seq
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                chunks.sort_by_key(|chunk| chunk.terminal_seq);
                Ok(build_terminal_replay_from_chunks(
                    client_id,
                    session_id,
                    chunks,
                    from_seq,
                    limit,
                    max_bytes,
                    include_data,
                    memory_terminal_next_seq(memory, client_id, session_id).await,
                ))
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        session_id,
                        terminal_seq,
                        job_id,
                        data,
                        size_bytes,
                        sha256_hex,
                        created_at::text AS created_at
                    FROM terminal_output_chunks
                    WHERE client_id = $1
                      AND session_id = $2
                      AND terminal_seq >= $3
                    ORDER BY terminal_seq ASC
                    LIMIT $4
                    "#,
                )
                .bind(client_id)
                .bind(session_id)
                .bind(from_seq)
                .bind(limit.saturating_add(1))
                .fetch_all(pool)
                .await?;
                let chunks = rows
                    .into_iter()
                    .map(terminal_output_chunk_from_row)
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
                let next_seq =
                    postgres_terminal_next_seq(pool, client_id, session_id, from_seq).await?;
                Ok(build_terminal_replay_from_chunks(
                    client_id,
                    session_id,
                    chunks,
                    from_seq,
                    limit,
                    max_bytes,
                    include_data,
                    next_seq,
                ))
            }
        }
    }

    pub(crate) async fn record_terminal_stream_chunk(
        &self,
        client_id: &str,
        event: &TerminalStreamOutput,
    ) -> Result<JobOutputWriteResult> {
        if self
            .terminal_job_command_type(event.job_id)
            .await?
            .as_deref()
            != Some("terminal_open")
        {
            anyhow::bail!("terminal_stream_job_not_found");
        }
        let terminal_seq = terminal_seq_i64(
            event
                .terminal_seq
                .context("terminal stream chunk missing terminal_seq")?,
        )?;
        let record = terminal_output_chunk_record(
            client_id,
            event.session_id,
            terminal_seq,
            event.job_id,
            event.output.data.clone(),
            None,
        );
        let retention = TerminalRetentionBounds::from_stream(event)?;
        self.record_terminal_output_chunk_record(record, retention)
            .await
    }

    pub(crate) async fn record_terminal_stream_status(
        &self,
        client_id: &str,
        event: &TerminalStreamOutput,
    ) -> Result<()> {
        let envelope_session_id = event.session_id;
        if self
            .terminal_job_command_type(event.job_id)
            .await?
            .as_deref()
            != Some("terminal_open")
        {
            anyhow::bail!("terminal_stream_job_not_found");
        }
        let output = TerminalStatusOutput {
            job_id: event.job_id,
            client_id: client_id.to_string(),
            seq: 0,
            data: event.output.data.clone(),
            created_at: now_rfc3339(),
        };
        let Some(event) = parse_terminal_event(output) else {
            anyhow::bail!("invalid_terminal_stream_status");
        };
        anyhow::ensure!(
            event.session_id == envelope_session_id && event.session_id != Uuid::nil(),
            "invalid_terminal_stream_status"
        );
        let job_id = event.job_id;
        let session_id = event.session_id;
        let incoming_state = event.state;
        terminal_job_statuses(incoming_state)?;
        match self {
            Self::Memory(memory) => {
                let session = TerminalAggregate::new(event).into_view();
                let now = now_rfc3339();
                let terminal_transitioned = {
                    let _terminal_guard = memory.job_terminal_side_effects.lock().await;
                    let mut jobs = memory.jobs.write().await;
                    let mut targets = memory.job_targets.write().await;
                    let mut sessions = memory.terminal_sessions.write().await;
                    let conflicting_binding = sessions
                        .iter()
                        .find(|session| {
                            session.client_id == client_id && session.session_id == session_id
                        })
                        .map(|session| session.job_id)
                        .is_some_and(|bound_job_id| bound_job_id != job_id);
                    if conflicting_binding {
                        if incoming_state != "rejected" {
                            anyhow::bail!("terminal_session_job_conflict");
                        }
                        match reconcile_memory_terminal_job_locked(
                            &mut jobs,
                            &mut targets,
                            job_id,
                            client_id,
                            incoming_state,
                            &now,
                        )? {
                            TerminalReconcileDisposition::Applied {
                                terminal_transitioned,
                            } => terminal_transitioned,
                            TerminalReconcileDisposition::StaleNonterminal { .. } => false,
                        }
                    } else {
                        let (job_status, target_status, terminal) =
                            terminal_job_statuses(incoming_state)?;
                        let job = jobs
                            .iter()
                            .find(|job| job.id == job_id && job.command_type == "terminal_open")
                            .context("terminal_open_job_not_found")?;
                        let target = targets
                            .iter()
                            .find(|target| target.job_id == job_id && target.client_id == client_id)
                            .context("terminal_open_target_not_found")?;
                        if let Some(authoritative_status) = memory_terminal_reconcile_disposition(
                            job,
                            target,
                            job_status,
                            target_status,
                            terminal,
                        )? {
                            normalize_memory_stale_terminal_session(
                                &mut sessions,
                                job_id,
                                client_id,
                                &authoritative_status,
                                &now,
                            );
                            false
                        } else {
                            let mut next_sessions = sessions.clone();
                            upsert_memory_terminal_session(&mut next_sessions, session)?;
                            let effective_state = next_sessions
                                .iter()
                                .find(|session| {
                                    session.client_id == client_id
                                        && session.session_id == session_id
                                })
                                .map(|session| session.state.clone())
                                .context("terminal_session_missing_after_upsert")?;
                            let transitioned = match reconcile_memory_terminal_job_locked(
                                &mut jobs,
                                &mut targets,
                                job_id,
                                client_id,
                                &effective_state,
                                &now,
                            )? {
                                TerminalReconcileDisposition::Applied {
                                    terminal_transitioned,
                                } => terminal_transitioned,
                                TerminalReconcileDisposition::StaleNonterminal {
                                    authoritative_status,
                                } => {
                                    normalize_memory_stale_terminal_session(
                                        &mut next_sessions,
                                        job_id,
                                        client_id,
                                        &authoritative_status,
                                        &now,
                                    );
                                    false
                                }
                            };
                            *sessions = next_sessions;
                            transitioned
                        }
                    }
                };
                if terminal_transitioned {
                    self.reconcile_memory_job_event_sources(job_id).await?;
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let bound_job_id: Option<Uuid> = sqlx::query_scalar(
                    r#"
                    SELECT job_id
                    FROM terminal_sessions
                    WHERE client_id = $1 AND session_id = $2
                    "#,
                )
                .bind(client_id)
                .bind(session_id)
                .fetch_optional(&mut *tx)
                .await?;
                if bound_job_id.is_some_and(|bound_job_id| bound_job_id != job_id) {
                    if incoming_state == "rejected" {
                        reconcile_postgres_terminal_job_in_tx(
                            &mut tx,
                            job_id,
                            client_id,
                            incoming_state,
                        )
                        .await?;
                        tx.commit().await?;
                        return Ok(());
                    }
                    anyhow::bail!("terminal_session_job_conflict");
                }
                let locked = lock_postgres_terminal_job_in_tx(&mut tx, job_id, client_id).await?;
                if let Some(authoritative_status) =
                    postgres_terminal_reconcile_disposition(&locked, incoming_state)?
                {
                    normalize_postgres_stale_terminal_session_in_tx(
                        &mut tx,
                        job_id,
                        client_id,
                        &authoritative_status,
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(());
                }
                let session = TerminalAggregate::new(event).into_view();
                let effective_state =
                    upsert_postgres_terminal_session_in_tx(&mut tx, &session).await?;
                reconcile_postgres_terminal_job_in_tx(&mut tx, job_id, client_id, &effective_state)
                    .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_terminal_command_replay_chunks(
        &self,
        job_id: Uuid,
        client_id: &str,
    ) -> Result<()> {
        let outputs = self
            .list_terminal_command_job_outputs(job_id, client_id)
            .await?;
        let Some(status) = terminal_replay_status_for_job_outputs(&outputs) else {
            return Ok(());
        };
        let Some(first_seq) = status.first_seq else {
            return Ok(());
        };
        let Some(session_id) = status.session_id else {
            return Ok(());
        };
        let retention = TerminalRetentionBounds {
            retained_first_seq: status.retained_first_seq.unwrap_or(first_seq).max(1),
            retained_bytes: retention_cap_i64(
                status
                    .retained_bytes
                    .unwrap_or(i64::from(MAX_TERMINAL_FLOW_WINDOW_BYTES)),
            ),
            dropped_bytes: status.dropped_bytes.unwrap_or(0),
            dropped_chunks: status.dropped_chunks.unwrap_or(0),
            replay_truncated: status.replay_truncated,
        };
        let mut pty_index = 0_i64;
        for output in outputs.into_iter().filter(|output| output.stream == "pty") {
            let terminal_seq = first_seq.saturating_add(pty_index);
            pty_index = pty_index.saturating_add(1);
            if terminal_seq < 1
                || status
                    .next_seq
                    .is_some_and(|next_seq| terminal_seq >= next_seq)
            {
                continue;
            }
            let data = BASE64
                .decode(&output.data_base64)
                .context("terminal replay job output is not valid base64")?;
            let record = terminal_output_chunk_record(
                client_id,
                session_id,
                terminal_seq,
                output.job_id,
                data,
                Some(output.created_at),
            );
            let result = self
                .record_terminal_output_chunk_record(record, retention)
                .await?;
            if result == JobOutputWriteResult::DuplicateConflict {
                anyhow::bail!("terminal_output_sequence_conflict");
            }
        }
        Ok(())
    }

    async fn record_terminal_output_chunk_record(
        &self,
        record: TerminalOutputChunkRecord,
        retention: TerminalRetentionBounds,
    ) -> Result<JobOutputWriteResult> {
        let result = match self {
            Self::Memory(memory) => {
                let mut chunks = memory.terminal_output_chunks.write().await;
                let existing = chunks.iter().position(|chunk| {
                    chunk.client_id == record.client_id
                        && chunk.session_id == record.session_id
                        && chunk.terminal_seq == record.terminal_seq
                });
                let result = match existing {
                    Some(index) if terminal_output_chunk_matches(&chunks[index], &record) => {
                        JobOutputWriteResult::DuplicateIdentical
                    }
                    Some(_) => JobOutputWriteResult::DuplicateConflict,
                    None => {
                        chunks.push(record.clone());
                        JobOutputWriteResult::Inserted
                    }
                };
                if result != JobOutputWriteResult::DuplicateConflict {
                    prune_memory_terminal_chunks(
                        &mut chunks,
                        &record.client_id,
                        record.session_id,
                        retention,
                    );
                    drop(chunks);
                    update_memory_terminal_session_range(memory, &record, retention).await;
                }
                result
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let inserted = sqlx::query_scalar::<_, Option<String>>(
                    r#"
                    INSERT INTO terminal_output_chunks (
                        client_id,
                        session_id,
                        terminal_seq,
                        job_id,
                        data,
                        size_bytes,
                        sha256_hex,
                        created_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8::timestamptz)
                    ON CONFLICT (client_id, session_id, terminal_seq)
                    DO NOTHING
                    RETURNING created_at::text
                    "#,
                )
                .bind(&record.client_id)
                .bind(record.session_id)
                .bind(record.terminal_seq)
                .bind(record.job_id)
                .bind(&record.data)
                .bind(record.size_bytes)
                .bind(&record.sha256_hex)
                .bind(&record.created_at)
                .fetch_optional(&mut *tx)
                .await?;
                let result = if inserted.flatten().is_some() {
                    JobOutputWriteResult::Inserted
                } else {
                    let existing = sqlx::query(
                        r#"
                        SELECT data, size_bytes, sha256_hex, created_at::text AS created_at
                        FROM terminal_output_chunks
                        WHERE client_id = $1 AND session_id = $2 AND terminal_seq = $3
                        "#,
                    )
                    .bind(&record.client_id)
                    .bind(record.session_id)
                    .bind(record.terminal_seq)
                    .fetch_one(&mut *tx)
                    .await?;
                    let existing = TerminalOutputChunkRecord {
                        client_id: record.client_id.clone(),
                        session_id: record.session_id,
                        terminal_seq: record.terminal_seq,
                        job_id: record.job_id,
                        data: existing.try_get("data")?,
                        size_bytes: existing.try_get("size_bytes")?,
                        sha256_hex: existing.try_get("sha256_hex")?,
                        created_at: existing.try_get("created_at")?,
                    };
                    if terminal_output_chunk_matches(&existing, &record) {
                        JobOutputWriteResult::DuplicateIdentical
                    } else {
                        JobOutputWriteResult::DuplicateConflict
                    }
                };
                if result == JobOutputWriteResult::DuplicateConflict {
                    tx.rollback().await?;
                    return Ok(JobOutputWriteResult::DuplicateConflict);
                }
                prune_postgres_terminal_chunks(
                    &mut tx,
                    &record.client_id,
                    record.session_id,
                    retention,
                )
                .await?;
                update_postgres_terminal_session_range(
                    &mut tx,
                    &record.client_id,
                    record.session_id,
                    record.terminal_seq.saturating_add(1),
                    retention,
                )
                .await?;
                tx.commit().await?;
                result
            }
        };
        Ok(result)
    }

    async fn terminal_job_command_type(&self, job_id: Uuid) -> Result<Option<String>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .jobs
                .read()
                .await
                .iter()
                .find(|job| job.id == job_id)
                .map(|job| job.command_type.clone())),
            Self::Postgres(pool) => {
                let command_type = sqlx::query_scalar(
                    r#"
                    SELECT command_type
                    FROM jobs
                    WHERE id = $1
                    "#,
                )
                .bind(job_id)
                .fetch_optional(pool)
                .await?;
                Ok(command_type)
            }
        }
    }

    async fn list_terminal_command_job_outputs(
        &self,
        job_id: Uuid,
        client_id: &str,
    ) -> Result<Vec<JobOutputView>> {
        match self {
            Self::Memory(memory) => {
                let mut outputs = memory
                    .job_outputs
                    .read()
                    .await
                    .iter()
                    .filter(|output| output.job_id == job_id && output.client_id == client_id)
                    .cloned()
                    .collect::<Vec<_>>();
                outputs.sort_by_key(|output| output.seq);
                Ok(outputs)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        job_id,
                        client_id,
                        seq,
                        stream,
                        data,
                        storage,
                        object_key,
                        data_sha256_hex,
                        data_size_bytes,
                        exit_code,
                        done,
                        created_at::text AS created_at
                    FROM job_outputs
                    WHERE job_id = $1 AND client_id = $2
                    ORDER BY seq ASC
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let data: Vec<u8> = row.try_get("data")?;
                        Ok(JobOutputView {
                            job_id: row.try_get("job_id")?,
                            client_id: row.try_get("client_id")?,
                            seq: row.try_get("seq")?,
                            stream: row.try_get("stream")?,
                            data_base64: BASE64.encode(data),
                            storage: row.try_get("storage")?,
                            artifact_object_key: row.try_get("object_key")?,
                            artifact_sha256_hex: row.try_get("data_sha256_hex")?,
                            artifact_size_bytes: row.try_get("data_size_bytes")?,
                            exit_code: row.try_get("exit_code")?,
                            done: row.try_get("done")?,
                            received_at: None,
                            created_at: row.try_get("created_at")?,
                        })
                    })
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }

    pub(crate) async fn refresh_terminal_sessions_for_client(&self, client_id: &str) -> Result<()> {
        let Self::Postgres(pool) = self else {
            return Ok(());
        };
        let sessions = terminal_sessions_from_outputs(pool, Some(client_id), None, 200).await?;
        for session in &sessions {
            let mut tx = pool.begin().await?;
            let locked =
                lock_postgres_terminal_job_in_tx(&mut tx, session.job_id, &session.client_id)
                    .await?;
            if let Some(authoritative_status) =
                postgres_terminal_reconcile_disposition(&locked, &session.state)?
            {
                normalize_postgres_stale_terminal_session_in_tx(
                    &mut tx,
                    session.job_id,
                    &session.client_id,
                    &authoritative_status,
                )
                .await?;
                tx.commit().await?;
                continue;
            }
            let effective_state = upsert_postgres_terminal_session_in_tx(&mut tx, session).await?;
            reconcile_postgres_terminal_job_in_tx(
                &mut tx,
                session.job_id,
                &session.client_id,
                &effective_state,
            )
            .await?;
            tx.commit().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) async fn upsert_postgres_terminal_session(
    pool: &PgPool,
    session: &TerminalSessionView,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    upsert_postgres_terminal_session_in_tx(&mut tx, session).await?;
    tx.commit().await?;
    Ok(())
}

async fn upsert_postgres_terminal_session_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    session: &TerminalSessionView,
) -> Result<String> {
    let row = sqlx::query(
        r#"
        INSERT INTO terminal_sessions (
            session_id, client_id, job_id, state, last_status, argv, cwd, cols, rows,
            idle_timeout_secs, flow_window_bytes, output_first_seq, output_next_seq,
            output_retained_first_seq, output_retained_bytes, output_dropped_bytes,
            output_dropped_chunks, output_replay_truncated, last_input_seq,
            close_reason, last_event, opened_at, observed_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
            $11, $12, $13, $14, $15, $16, $17, $18, $19,
            $20, $21, $22::timestamptz, $23::timestamptz
        )
        ON CONFLICT (client_id, session_id)
        DO UPDATE SET
            state = CASE
                WHEN terminal_sessions.state IN (
                    'closed', 'missing', 'rejected', 'failed', 'exited'
                )
                AND EXCLUDED.state NOT IN (
                    'closed', 'missing', 'rejected', 'failed', 'exited'
                )
                THEN terminal_sessions.state
                ELSE EXCLUDED.state
            END,
            last_status = CASE
                WHEN terminal_sessions.state IN (
                    'closed', 'missing', 'rejected', 'failed', 'exited'
                )
                AND EXCLUDED.state NOT IN (
                    'closed', 'missing', 'rejected', 'failed', 'exited'
                )
                THEN terminal_sessions.last_status
                ELSE EXCLUDED.last_status
            END,
            argv = CASE
                WHEN jsonb_array_length(EXCLUDED.argv) > 0 THEN EXCLUDED.argv
                ELSE terminal_sessions.argv
            END,
            cwd = COALESCE(EXCLUDED.cwd, terminal_sessions.cwd),
            cols = COALESCE(EXCLUDED.cols, terminal_sessions.cols),
            rows = COALESCE(EXCLUDED.rows, terminal_sessions.rows),
            idle_timeout_secs = COALESCE(
                EXCLUDED.idle_timeout_secs, terminal_sessions.idle_timeout_secs
            ),
            flow_window_bytes = COALESCE(
                EXCLUDED.flow_window_bytes, terminal_sessions.flow_window_bytes
            ),
            output_first_seq = COALESCE(
                terminal_sessions.output_first_seq, EXCLUDED.output_first_seq
            ),
            output_next_seq = GREATEST(
                terminal_sessions.output_next_seq, EXCLUDED.output_next_seq
            ),
            output_retained_first_seq = GREATEST(
                terminal_sessions.output_retained_first_seq,
                EXCLUDED.output_retained_first_seq
            ),
            output_retained_bytes = CASE
                WHEN EXCLUDED.output_next_seq IS NOT NULL
                     AND (
                         terminal_sessions.output_next_seq IS NULL
                         OR EXCLUDED.output_next_seq >= terminal_sessions.output_next_seq
                     )
                THEN EXCLUDED.output_retained_bytes
                ELSE terminal_sessions.output_retained_bytes
            END,
            output_dropped_bytes = GREATEST(
                terminal_sessions.output_dropped_bytes, EXCLUDED.output_dropped_bytes
            ),
            output_dropped_chunks = GREATEST(
                terminal_sessions.output_dropped_chunks, EXCLUDED.output_dropped_chunks
            ),
            output_replay_truncated =
                terminal_sessions.output_replay_truncated
                OR EXCLUDED.output_replay_truncated,
            last_input_seq = GREATEST(
                terminal_sessions.last_input_seq, EXCLUDED.last_input_seq
            ),
            close_reason = COALESCE(
                terminal_sessions.close_reason, EXCLUDED.close_reason
            ),
            last_event = CASE
                WHEN terminal_sessions.state IN (
                    'closed', 'missing', 'rejected', 'failed', 'exited'
                )
                AND EXCLUDED.state NOT IN (
                    'closed', 'missing', 'rejected', 'failed', 'exited'
                )
                THEN terminal_sessions.last_event
                ELSE EXCLUDED.last_event
            END,
            opened_at = CASE
                WHEN terminal_sessions.opened_at IS NULL THEN EXCLUDED.opened_at
                WHEN EXCLUDED.opened_at IS NULL THEN terminal_sessions.opened_at
                ELSE LEAST(terminal_sessions.opened_at, EXCLUDED.opened_at)
            END,
            observed_at = CASE
                WHEN terminal_sessions.state IN (
                    'closed', 'missing', 'rejected', 'failed', 'exited'
                )
                AND EXCLUDED.state NOT IN (
                    'closed', 'missing', 'rejected', 'failed', 'exited'
                )
                THEN terminal_sessions.observed_at
                ELSE EXCLUDED.observed_at
            END
        WHERE terminal_sessions.job_id = EXCLUDED.job_id
          AND (
              EXCLUDED.observed_at >= terminal_sessions.observed_at
              OR EXCLUDED.state IN (
                  'closed', 'missing', 'rejected', 'failed', 'exited'
              )
          )
        RETURNING job_id, state
        "#,
    )
    .bind(session.session_id)
    .bind(&session.client_id)
    .bind(session.job_id)
    .bind(&session.state)
    .bind(&session.last_status)
    .bind(SqlJson(&session.argv))
    .bind(&session.cwd)
    .bind(session.cols)
    .bind(session.rows)
    .bind(session.idle_timeout_secs)
    .bind(session.flow_window_bytes)
    .bind(session.output_first_seq)
    .bind(session.output_next_seq)
    .bind(session.output_retained_first_seq)
    .bind(session.output_retained_bytes)
    .bind(session.output_dropped_bytes)
    .bind(session.output_dropped_chunks)
    .bind(session.output_replay_truncated)
    .bind(session.last_input_seq)
    .bind(&session.close_reason)
    .bind(&session.last_event)
    .bind(&session.opened_at)
    .bind(&session.observed_at)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = row {
        let persisted_job_id: Uuid = row.try_get("job_id")?;
        anyhow::ensure!(
            persisted_job_id == session.job_id,
            "terminal_session_job_conflict"
        );
        return Ok(row.try_get("state")?);
    }
    let existing = sqlx::query(
        r#"
            SELECT job_id, state
            FROM terminal_sessions
            WHERE client_id = $1 AND session_id = $2
            "#,
    )
    .bind(&session.client_id)
    .bind(session.session_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(existing) = existing else {
        anyhow::bail!("terminal_session_missing_after_upsert");
    };
    let existing_job_id: Uuid = existing.try_get("job_id")?;
    if existing_job_id != session.job_id {
        anyhow::bail!("terminal_session_job_conflict");
    }
    Ok(existing.try_get("state")?)
}

async fn terminal_sessions_from_outputs(
    pool: &PgPool,
    client_id: Option<&str>,
    session_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<TerminalSessionView>> {
    let limit = limit.clamp(1, 200);
    let scan_limit = limit.saturating_mul(64).clamp(100, 10_000);
    let rows = sqlx::query(
        r#"
        SELECT
            output.job_id,
            output.client_id,
            output.seq,
            output.data,
            output.created_at::text AS created_at
        FROM job_outputs output
        JOIN jobs job ON job.id = output.job_id
        WHERE output.stream = 'status'
          AND job.command_type = 'terminal_open'
          AND ($2::text IS NULL OR output.client_id = $2)
        ORDER BY output.created_at DESC, output.job_id DESC, output.seq DESC
        LIMIT $1
        "#,
    )
    .bind(scan_limit)
    .bind(client_id)
    .fetch_all(pool)
    .await?;
    let outputs = rows
        .into_iter()
        .map(|row| {
            Ok(TerminalStatusOutput {
                job_id: row.try_get("job_id")?,
                client_id: row.try_get("client_id")?,
                seq: row.try_get("seq")?,
                data: row.try_get("data")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
    build_terminal_sessions(outputs, limit, session_id)
}

fn terminal_session_from_row(row: PgRow) -> std::result::Result<TerminalSessionView, sqlx::Error> {
    let argv: SqlJson<Vec<String>> = row.try_get("argv")?;
    Ok(TerminalSessionView {
        session_id: row.try_get("session_id")?,
        client_id: row.try_get("client_id")?,
        job_id: row.try_get("job_id")?,
        state: row.try_get("state")?,
        last_status: row.try_get("last_status")?,
        argv: argv.0,
        cwd: row.try_get("cwd")?,
        cols: row.try_get("cols")?,
        rows: row.try_get("rows")?,
        idle_timeout_secs: row.try_get("idle_timeout_secs")?,
        flow_window_bytes: row.try_get("flow_window_bytes")?,
        output_first_seq: row.try_get("output_first_seq")?,
        output_next_seq: row.try_get("output_next_seq")?,
        output_retained_first_seq: row.try_get("output_retained_first_seq")?,
        output_retained_bytes: row.try_get("output_retained_bytes")?,
        output_dropped_bytes: row.try_get("output_dropped_bytes")?,
        output_dropped_chunks: row.try_get("output_dropped_chunks")?,
        output_replay_truncated: row.try_get("output_replay_truncated")?,
        last_input_seq: row.try_get("last_input_seq")?,
        close_reason: row.try_get("close_reason")?,
        last_event: row.try_get("last_event")?,
        opened_at: row.try_get("opened_at")?,
        observed_at: row.try_get("observed_at")?,
    })
}

fn terminal_output_chunk_from_row(
    row: PgRow,
) -> std::result::Result<TerminalOutputChunkRecord, sqlx::Error> {
    Ok(TerminalOutputChunkRecord {
        client_id: row.try_get("client_id")?,
        session_id: row.try_get("session_id")?,
        terminal_seq: row.try_get("terminal_seq")?,
        job_id: row.try_get("job_id")?,
        data: row.try_get("data")?,
        size_bytes: row.try_get("size_bytes")?,
        sha256_hex: row.try_get("sha256_hex")?,
        created_at: row.try_get("created_at")?,
    })
}

#[derive(Clone, Debug)]
struct TerminalStatusOutput {
    job_id: Uuid,
    client_id: String,
    seq: i32,
    data: Vec<u8>,
    created_at: String,
}

#[derive(Clone, Debug)]
struct TerminalEvent {
    session_id: Uuid,
    client_id: String,
    state: &'static str,
    status: String,
    argv: Vec<String>,
    cwd: Option<String>,
    cols: Option<i64>,
    rows: Option<i64>,
    idle_timeout_secs: Option<i64>,
    flow_window_bytes: Option<i64>,
    output_first_seq: Option<i64>,
    output_next_seq: Option<i64>,
    output_retained_first_seq: Option<i64>,
    output_retained_bytes: Option<i64>,
    output_dropped_bytes: Option<i64>,
    output_dropped_chunks: Option<i64>,
    output_replay_truncated: bool,
    input_seq: Option<i64>,
    session_exited: bool,
    close_reason: Option<String>,
    event_type: String,
    job_id: Uuid,
    created_at: String,
}

#[derive(Clone, Debug)]
struct TerminalAggregate {
    latest: TerminalEvent,
    argv: Vec<String>,
    cwd: Option<String>,
    cols: Option<i64>,
    rows: Option<i64>,
    idle_timeout_secs: Option<i64>,
    flow_window_bytes: Option<i64>,
    output_first_seq: Option<i64>,
    output_next_seq: Option<i64>,
    output_retained_first_seq: Option<i64>,
    output_retained_bytes: Option<i64>,
    output_dropped_bytes: Option<i64>,
    output_dropped_chunks: Option<i64>,
    output_replay_truncated: bool,
    last_input_seq: Option<i64>,
    close_reason: Option<String>,
    opened_at: Option<String>,
}

impl TerminalAggregate {
    fn new(event: TerminalEvent) -> Self {
        Self {
            argv: event.argv.clone(),
            cwd: event.cwd.clone(),
            cols: event.cols,
            rows: event.rows,
            idle_timeout_secs: event.idle_timeout_secs,
            flow_window_bytes: event.flow_window_bytes,
            output_first_seq: event.output_first_seq,
            output_next_seq: event.output_next_seq,
            output_retained_first_seq: event.output_retained_first_seq,
            output_retained_bytes: event.output_retained_bytes,
            output_dropped_bytes: event.output_dropped_bytes,
            output_dropped_chunks: event.output_dropped_chunks,
            output_replay_truncated: event.output_replay_truncated,
            last_input_seq: event.input_seq,
            close_reason: event.close_reason.clone(),
            opened_at: (event.event_type == "terminal_open").then(|| event.created_at.clone()),
            latest: event,
        }
    }

    fn merge_older(&mut self, event: TerminalEvent) -> Result<()> {
        let replacement = (terminal_event_is_terminal(&event)
            && !terminal_event_is_terminal(&self.latest))
        .then(|| event.clone());
        let incoming_range_is_current = event.output_retained_bytes.is_some()
            && (self.output_retained_bytes.is_none()
                || event.output_next_seq.is_some_and(|next_seq| {
                    self.output_next_seq
                        .is_none_or(|current_seq| next_seq > current_seq)
                }));
        let session_exited = self.latest.session_exited || event.session_exited;
        if self.argv.is_empty() {
            self.argv = event.argv.clone();
        }
        self.cwd = self.cwd.take().or(event.cwd.clone());
        self.cols = self.cols.or(event.cols);
        self.rows = self.rows.or(event.rows);
        self.idle_timeout_secs = self.idle_timeout_secs.or(event.idle_timeout_secs);
        self.flow_window_bytes = self.flow_window_bytes.or(event.flow_window_bytes);
        self.output_first_seq = self.output_first_seq.or(event.output_first_seq);
        self.output_next_seq = max_optional_i64(self.output_next_seq, event.output_next_seq);
        self.output_retained_first_seq = max_optional_i64(
            self.output_retained_first_seq,
            event.output_retained_first_seq,
        );
        if incoming_range_is_current {
            self.output_retained_bytes = event.output_retained_bytes;
        }
        self.output_dropped_bytes =
            max_optional_i64(self.output_dropped_bytes, event.output_dropped_bytes);
        self.output_dropped_chunks =
            max_optional_i64(self.output_dropped_chunks, event.output_dropped_chunks);
        self.output_replay_truncated |= event.output_replay_truncated;
        self.last_input_seq = max_optional_i64(self.last_input_seq, event.input_seq);
        self.close_reason = self.close_reason.take().or(event.close_reason.clone());
        if event.event_type == "terminal_open" {
            self.opened_at =
                earliest_timestamp(self.opened_at.take(), Some(event.created_at.clone()))?;
        }
        if let Some(replacement) = replacement {
            self.latest = replacement;
        }
        self.latest.session_exited = session_exited;
        Ok(())
    }

    fn into_view(self) -> TerminalSessionView {
        TerminalSessionView {
            session_id: self.latest.session_id,
            client_id: self.latest.client_id,
            job_id: self.latest.job_id,
            state: self.latest.state.to_string(),
            last_status: self.latest.status,
            argv: self.argv,
            cwd: self.cwd,
            cols: self.cols,
            rows: self.rows,
            idle_timeout_secs: self.idle_timeout_secs,
            flow_window_bytes: self.flow_window_bytes,
            output_first_seq: self.output_first_seq,
            output_next_seq: self.output_next_seq,
            output_retained_first_seq: self.output_retained_first_seq,
            output_retained_bytes: self.output_retained_bytes,
            output_dropped_bytes: self.output_dropped_bytes,
            output_dropped_chunks: self.output_dropped_chunks,
            output_replay_truncated: self.output_replay_truncated,
            last_input_seq: self.last_input_seq.unwrap_or(0),
            close_reason: self.close_reason,
            last_event: self.latest.event_type,
            opened_at: self.opened_at,
            observed_at: self.latest.created_at,
        }
    }
}

fn terminal_state_is_terminal(state: &str) -> bool {
    matches!(
        state,
        "closed" | "missing" | "rejected" | "failed" | "exited"
    )
}

fn terminal_event_is_terminal(event: &TerminalEvent) -> bool {
    event.session_exited || terminal_state_is_terminal(event.state)
}

fn terminal_session_is_terminal(session: &TerminalSessionView) -> bool {
    terminal_state_is_terminal(&session.state)
}

fn sort_terminal_outputs_newest(outputs: &mut [TerminalStatusOutput]) -> Result<()> {
    for output in outputs.iter() {
        crate::util::parse_timestamp_utc(&output.created_at)
            .with_context(|| "terminal source timestamp is invalid")?;
    }
    outputs.sort_by(|left, right| {
        crate::util::parse_timestamp_utc(&right.created_at)
            .expect("terminal timestamps were validated before sorting")
            .cmp(
                &crate::util::parse_timestamp_utc(&left.created_at)
                    .expect("terminal timestamps were validated before sorting"),
            )
            .then_with(|| right.job_id.cmp(&left.job_id))
            .then_with(|| right.seq.cmp(&left.seq))
    });
    Ok(())
}

fn build_terminal_sessions(
    outputs: Vec<TerminalStatusOutput>,
    limit: i64,
    session_filter: Option<Uuid>,
) -> Result<Vec<TerminalSessionView>> {
    let mut order = Vec::<(String, Uuid)>::new();
    let mut aggregates = BTreeMap::<(String, Uuid), TerminalAggregate>::new();

    for output in outputs {
        let Some(event) = parse_terminal_event(output) else {
            continue;
        };
        if session_filter.is_some_and(|session_id| event.session_id != session_id) {
            continue;
        }
        let key = (event.client_id.clone(), event.session_id);
        if let Some(aggregate) = aggregates.get_mut(&key) {
            aggregate.merge_older(event)?;
        } else {
            order.push(key.clone());
            aggregates.insert(key, TerminalAggregate::new(event));
        }
    }

    let limit = limit.clamp(1, 200) as usize;
    let mut views = Vec::new();
    let mut emitted = BTreeSet::new();
    for key in order {
        if !emitted.insert(key.clone()) {
            continue;
        }
        if let Some(aggregate) = aggregates.remove(&key) {
            views.push(aggregate.into_view());
            if views.len() >= limit {
                break;
            }
        }
    }
    Ok(views)
}

fn parse_terminal_event(output: TerminalStatusOutput) -> Option<TerminalEvent> {
    let value = serde_json::from_slice::<Value>(&output.data).ok()?;
    let event_type = value.get("type")?.as_str()?.to_string();
    if !is_terminal_status_event(&event_type) {
        return None;
    }
    let session_id = value
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())?;
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let session_exited = value
        .get("session_exited")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state = terminal_state(&event_type, &status, session_exited);

    Some(TerminalEvent {
        session_id,
        client_id: output.client_id,
        state,
        status,
        argv: value
            .get("argv")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        cwd: json_string(&value, "cwd"),
        cols: value.get("cols").and_then(json_i64),
        rows: value.get("rows").and_then(json_i64),
        idle_timeout_secs: value.get("idle_timeout_secs").and_then(json_i64),
        flow_window_bytes: value.get("flow_window_bytes").and_then(json_i64),
        output_first_seq: value.get("output_first_seq").and_then(json_i64),
        output_next_seq: value.get("output_next_seq").and_then(json_i64),
        output_retained_first_seq: value.get("output_retained_first_seq").and_then(json_i64),
        output_retained_bytes: value.get("output_retained_bytes").and_then(json_i64),
        output_dropped_bytes: value.get("output_dropped_bytes").and_then(json_i64),
        output_dropped_chunks: value.get("output_dropped_chunks").and_then(json_i64),
        output_replay_truncated: value
            .get("output_replay_truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        input_seq: value.get("input_seq").and_then(json_i64),
        session_exited,
        close_reason: json_string(&value, "reason"),
        event_type,
        job_id: output.job_id,
        created_at: output.created_at,
    })
}

fn terminal_state(event_type: &str, status: &str, session_exited: bool) -> &'static str {
    terminal_session_state(event_type, status, session_exited)
}

fn json_string(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
}

fn is_terminal_command(command_type: &str) -> bool {
    is_terminal_command_type(command_type)
}

fn is_terminal_status_event(event_type: &str) -> bool {
    is_terminal_session_event(event_type)
}

#[derive(Clone, Debug)]
struct TerminalReplayStatus {
    session_id: Option<Uuid>,
    first_seq: Option<i64>,
    next_seq: Option<i64>,
    retained_first_seq: Option<i64>,
    retained_bytes: Option<i64>,
    dropped_bytes: Option<i64>,
    dropped_chunks: Option<i64>,
    replay_truncated: bool,
}

#[derive(Clone, Copy, Debug)]
struct TerminalRetentionBounds {
    retained_first_seq: i64,
    retained_bytes: i64,
    dropped_bytes: i64,
    dropped_chunks: i64,
    replay_truncated: bool,
}

impl TerminalRetentionBounds {
    fn from_stream(event: &TerminalStreamOutput) -> Result<Self> {
        Ok(Self {
            retained_first_seq: event
                .output_retained_first_seq
                .map(terminal_seq_i64)
                .transpose()?
                .unwrap_or(1)
                .max(1),
            retained_bytes: retention_cap_bytes(event.output_retained_bytes),
            dropped_bytes: u64_to_i64_saturating(event.output_dropped_bytes),
            dropped_chunks: u64_to_i64_saturating(event.output_dropped_chunks),
            replay_truncated: event.output_replay_truncated,
        })
    }
}

fn build_terminal_replay_from_chunks(
    client_id: &str,
    session_id: Uuid,
    mut chunks: Vec<TerminalOutputChunkRecord>,
    from_seq: i64,
    limit: i64,
    max_bytes: i64,
    include_data: bool,
    next_seq_hint: i64,
) -> TerminalReplayView {
    let limit = limit.clamp(1, 1000) as usize;
    let mut byte_count = 0_i64;
    let mut replay_chunks = Vec::new();
    let mut truncated = false;
    chunks.sort_by_key(|chunk| chunk.terminal_seq);
    for chunk in chunks {
        if chunk.terminal_seq < from_seq {
            continue;
        }
        if replay_chunks.len() >= limit {
            truncated = true;
            break;
        }
        let size_bytes = chunk.size_bytes.max(0);
        if byte_count.saturating_add(size_bytes) > max_bytes {
            truncated = true;
            break;
        }
        byte_count = byte_count.saturating_add(size_bytes);
        replay_chunks.push(TerminalReplayChunkView {
            terminal_seq: chunk.terminal_seq,
            job_id: chunk.job_id,
            data_base64: include_data.then(|| BASE64.encode(&chunk.data)),
            size_bytes,
            sha256_hex: chunk.sha256_hex,
            created_at: chunk.created_at,
        });
    }
    let available_first_seq = replay_chunks.first().map(|chunk| chunk.terminal_seq);
    let next_seq = next_seq_hint.max(
        replay_chunks
            .last()
            .map(|chunk| chunk.terminal_seq.saturating_add(1))
            .unwrap_or(from_seq),
    );
    TerminalReplayView {
        session_id,
        client_id: client_id.to_string(),
        from_seq,
        available_first_seq,
        next_seq,
        chunk_count: replay_chunks.len(),
        byte_count,
        truncated,
        source: "terminal_output_chunks".to_string(),
        chunks: replay_chunks,
    }
}

fn terminal_replay_status_for_job_outputs(
    outputs: &[JobOutputView],
) -> Option<TerminalReplayStatus> {
    let mut merged = TerminalReplayStatus {
        session_id: None,
        first_seq: None,
        next_seq: None,
        retained_first_seq: None,
        retained_bytes: None,
        dropped_bytes: None,
        dropped_chunks: None,
        replay_truncated: false,
    };
    let mut found = false;
    for status in outputs.iter().filter_map(|output| {
        if output.stream != "status" {
            return None;
        }
        parse_terminal_replay_status(output)
    }) {
        found = true;
        merged.session_id = merged.session_id.or(status.session_id);
        merged.first_seq = match (merged.first_seq, status.first_seq) {
            (Some(current), Some(next)) => Some(current.min(next)),
            (None, value) | (value, None) => value,
        };
        merged.next_seq = match (merged.next_seq, status.next_seq) {
            (Some(current), Some(next)) => Some(current.max(next)),
            (None, value) | (value, None) => value,
        };
        merged.retained_first_seq = status.retained_first_seq.or(merged.retained_first_seq);
        merged.retained_bytes = status.retained_bytes.or(merged.retained_bytes);
        merged.dropped_bytes = status.dropped_bytes.or(merged.dropped_bytes);
        merged.dropped_chunks = status.dropped_chunks.or(merged.dropped_chunks);
        merged.replay_truncated |= status.replay_truncated;
    }
    found.then_some(merged)
}

fn parse_terminal_replay_status(output: &JobOutputView) -> Option<TerminalReplayStatus> {
    let data = BASE64.decode(&output.data_base64).ok()?;
    let value = serde_json::from_slice::<Value>(&data).ok()?;
    if !is_terminal_status_event(value.get("type")?.as_str()?) {
        return None;
    }
    let session_id = value
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())?;
    Some(TerminalReplayStatus {
        session_id: Some(session_id),
        first_seq: value.get("output_first_seq").and_then(json_i64),
        next_seq: value.get("output_next_seq").and_then(json_i64),
        retained_first_seq: value.get("output_retained_first_seq").and_then(json_i64),
        retained_bytes: value.get("output_retained_bytes").and_then(json_i64),
        dropped_bytes: value.get("output_dropped_bytes").and_then(json_i64),
        dropped_chunks: value.get("output_dropped_chunks").and_then(json_i64),
        replay_truncated: value
            .get("output_replay_truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

async fn memory_terminal_next_seq(
    memory: &crate::repository::MemoryState,
    client_id: &str,
    session_id: Uuid,
) -> i64 {
    let session_next = memory
        .terminal_sessions
        .read()
        .await
        .iter()
        .find(|session| session.client_id == client_id && session.session_id == session_id)
        .and_then(|session| session.output_next_seq);
    let chunk_next = memory
        .terminal_output_chunks
        .read()
        .await
        .iter()
        .filter(|chunk| chunk.client_id == client_id && chunk.session_id == session_id)
        .map(|chunk| chunk.terminal_seq.saturating_add(1))
        .max();
    session_next.or(chunk_next).unwrap_or(1).max(1)
}

async fn postgres_terminal_next_seq(
    pool: &PgPool,
    client_id: &str,
    session_id: Uuid,
    from_seq: i64,
) -> Result<i64> {
    let next_seq: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(
            (
                SELECT output_next_seq
                FROM terminal_sessions
                WHERE client_id = $1 AND session_id = $2
            ),
            (
                SELECT MAX(terminal_seq) + 1
                FROM terminal_output_chunks
                WHERE client_id = $1 AND session_id = $2
            )
        )
        "#,
    )
    .bind(client_id)
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    Ok(next_seq.unwrap_or(from_seq).max(1))
}

fn terminal_output_chunk_record(
    client_id: &str,
    session_id: Uuid,
    terminal_seq: i64,
    job_id: Uuid,
    data: Vec<u8>,
    created_at: Option<String>,
) -> TerminalOutputChunkRecord {
    TerminalOutputChunkRecord {
        client_id: client_id.to_string(),
        session_id,
        terminal_seq,
        job_id,
        size_bytes: data.len() as i64,
        sha256_hex: payload_hash(&data),
        data,
        created_at: created_at.unwrap_or_else(now_rfc3339),
    }
}

fn terminal_output_chunk_matches(
    left: &TerminalOutputChunkRecord,
    right: &TerminalOutputChunkRecord,
) -> bool {
    left.size_bytes == right.size_bytes
        && left.sha256_hex == right.sha256_hex
        && left.data == right.data
}

fn prune_memory_terminal_chunks(
    chunks: &mut Vec<TerminalOutputChunkRecord>,
    client_id: &str,
    session_id: Uuid,
    retention: TerminalRetentionBounds,
) {
    let mut retained_bytes = 0_i64;
    let mut retained = HashSet::new();
    let mut matching = chunks
        .iter()
        .filter(|chunk| chunk.client_id == client_id && chunk.session_id == session_id)
        .map(|chunk| chunk.terminal_seq)
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| right.cmp(left));
    for terminal_seq in matching {
        if terminal_seq < retention.retained_first_seq {
            continue;
        }
        let Some(size_bytes) = chunks
            .iter()
            .find(|chunk| {
                chunk.client_id == client_id
                    && chunk.session_id == session_id
                    && chunk.terminal_seq == terminal_seq
            })
            .map(|chunk| chunk.size_bytes.max(0))
        else {
            continue;
        };
        if retained_bytes.saturating_add(size_bytes) > retention.retained_bytes {
            continue;
        }
        retained_bytes = retained_bytes.saturating_add(size_bytes);
        retained.insert(terminal_seq);
    }
    chunks.retain(|chunk| {
        chunk.client_id != client_id
            || chunk.session_id != session_id
            || retained.contains(&chunk.terminal_seq)
    });
}

async fn prune_postgres_terminal_chunks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    session_id: Uuid,
    retention: TerminalRetentionBounds,
) -> Result<()> {
    sqlx::query(
        r#"
        WITH ranked AS (
            SELECT
                terminal_seq,
                SUM(size_bytes) OVER (ORDER BY terminal_seq DESC) AS newest_bytes
            FROM terminal_output_chunks
            WHERE client_id = $1 AND session_id = $2
        )
        DELETE FROM terminal_output_chunks chunk
        USING ranked
        WHERE chunk.client_id = $1
          AND chunk.session_id = $2
          AND chunk.terminal_seq = ranked.terminal_seq
          AND (
              chunk.terminal_seq < $3
              OR ranked.newest_bytes > $4
          )
        "#,
    )
    .bind(client_id)
    .bind(session_id)
    .bind(retention.retained_first_seq)
    .bind(retention.retained_bytes)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn update_memory_terminal_session_range(
    memory: &crate::repository::MemoryState,
    record: &TerminalOutputChunkRecord,
    retention: TerminalRetentionBounds,
) {
    let mut sessions = memory.terminal_sessions.write().await;
    if let Some(session) = sessions.iter_mut().find(|session| {
        session.client_id == record.client_id && session.session_id == record.session_id
    }) {
        session.output_first_seq = session.output_first_seq.or(Some(1));
        session.output_next_seq = Some(
            session
                .output_next_seq
                .unwrap_or(1)
                .max(record.terminal_seq.saturating_add(1)),
        );
        session.output_retained_first_seq = Some(retention.retained_first_seq);
        session.output_retained_bytes = Some(retention.retained_bytes);
        session.output_dropped_bytes = Some(retention.dropped_bytes);
        session.output_dropped_chunks = Some(retention.dropped_chunks);
        session.output_replay_truncated |= retention.replay_truncated;
    }
}

async fn update_postgres_terminal_session_range(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    session_id: Uuid,
    next_seq: i64,
    retention: TerminalRetentionBounds,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE terminal_sessions
        SET
            output_first_seq = COALESCE(output_first_seq, 1),
            output_next_seq = GREATEST(COALESCE(output_next_seq, 1), $3),
            output_retained_first_seq = $4,
            output_retained_bytes = $5,
            output_dropped_bytes = $6,
            output_dropped_chunks = $7,
            output_replay_truncated = output_replay_truncated OR $8
        WHERE client_id = $1 AND session_id = $2
        "#,
    )
    .bind(client_id)
    .bind(session_id)
    .bind(next_seq.max(1))
    .bind(retention.retained_first_seq)
    .bind(retention.retained_bytes)
    .bind(retention.dropped_bytes)
    .bind(retention.dropped_chunks)
    .bind(retention.replay_truncated)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn upsert_memory_terminal_session(
    sessions: &mut Vec<TerminalSessionView>,
    next: TerminalSessionView,
) -> Result<()> {
    crate::util::parse_timestamp_utc(&next.observed_at)
        .context("terminal source timestamp is invalid")?;
    if let Some(opened_at) = next.opened_at.as_deref() {
        crate::util::parse_timestamp_utc(opened_at)
            .context("terminal opened timestamp is invalid")?;
    }
    if let Some(existing) = sessions.iter_mut().find(|session| {
        session.client_id == next.client_id && session.session_id == next.session_id
    }) {
        if existing.job_id != next.job_id {
            anyhow::bail!("terminal_session_job_conflict");
        }
        let advances_terminal =
            terminal_session_is_terminal(&next) && !terminal_session_is_terminal(existing);
        if !advances_terminal && !terminal_source_is_at_least_as_new(existing, &next)? {
            return Ok(());
        }
        let preserve_terminal =
            terminal_session_is_terminal(existing) && !terminal_session_is_terminal(&next);
        let incoming_range_is_current = next.output_retained_bytes.is_some()
            && (existing.output_retained_bytes.is_none()
                || next.output_next_seq.is_some_and(|next_seq| {
                    existing
                        .output_next_seq
                        .is_none_or(|existing_seq| next_seq >= existing_seq)
                }));
        if !preserve_terminal {
            existing.state = next.state;
            existing.last_status = next.last_status;
        }
        if !next.argv.is_empty() {
            existing.argv = next.argv;
        }
        existing.cwd = next.cwd.or_else(|| existing.cwd.take());
        existing.cols = next.cols.or(existing.cols);
        existing.rows = next.rows.or(existing.rows);
        existing.idle_timeout_secs = next.idle_timeout_secs.or(existing.idle_timeout_secs);
        existing.flow_window_bytes = next.flow_window_bytes.or(existing.flow_window_bytes);
        existing.output_first_seq = existing.output_first_seq.or(next.output_first_seq);
        existing.output_next_seq = max_optional_i64(existing.output_next_seq, next.output_next_seq);
        existing.output_retained_first_seq = max_optional_i64(
            existing.output_retained_first_seq,
            next.output_retained_first_seq,
        );
        if incoming_range_is_current {
            existing.output_retained_bytes = next.output_retained_bytes;
        }
        existing.output_dropped_bytes =
            max_optional_i64(existing.output_dropped_bytes, next.output_dropped_bytes);
        existing.output_dropped_chunks =
            max_optional_i64(existing.output_dropped_chunks, next.output_dropped_chunks);
        existing.output_replay_truncated |= next.output_replay_truncated;
        existing.last_input_seq = existing.last_input_seq.max(next.last_input_seq);
        existing.close_reason = existing.close_reason.take().or(next.close_reason);
        if !preserve_terminal {
            existing.last_event = next.last_event;
            existing.observed_at = next.observed_at;
        }
        existing.opened_at = earliest_timestamp(existing.opened_at.take(), next.opened_at)?;
    } else {
        sessions.push(next);
    }
    Ok(())
}

fn max_optional_i64(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn terminal_source_is_at_least_as_new(
    existing: &TerminalSessionView,
    next: &TerminalSessionView,
) -> Result<bool> {
    Ok(compare_terminal_timestamps(&next.observed_at, &existing.observed_at)? != Ordering::Less)
}

fn compare_terminal_timestamps(left: &str, right: &str) -> Result<Ordering> {
    let left = crate::util::parse_timestamp_utc(left)
        .context("incoming terminal source timestamp is invalid")?;
    let right = crate::util::parse_timestamp_utc(right)
        .context("stored terminal source timestamp is invalid")?;
    Ok(left.cmp(&right))
}

fn earliest_timestamp(left: Option<String>, right: Option<String>) -> Result<Option<String>> {
    match (left, right) {
        (Some(left), Some(right)) => {
            let order = compare_terminal_timestamps(&left, &right)?;
            Ok(Some(if order == Ordering::Greater {
                right
            } else {
                left
            }))
        }
        (Some(value), None) | (None, Some(value)) => {
            crate::util::parse_timestamp_utc(&value)
                .context("terminal opened timestamp is invalid")?;
            Ok(Some(value))
        }
        (None, None) => Ok(None),
    }
}

fn deduplicate_terminal_sessions(
    mut sessions: Vec<TerminalSessionView>,
    limit: i64,
) -> Result<Vec<TerminalSessionView>> {
    for session in &sessions {
        crate::util::parse_timestamp_utc(&session.observed_at)
            .context("terminal source timestamp is invalid")?;
    }
    sessions.sort_by(|left, right| {
        crate::util::parse_timestamp_utc(&right.observed_at)
            .expect("terminal timestamps were validated before sorting")
            .cmp(
                &crate::util::parse_timestamp_utc(&left.observed_at)
                    .expect("terminal timestamps were validated before sorting"),
            )
            .then_with(|| left.client_id.cmp(&right.client_id))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    let mut emitted = HashSet::new();
    let mut deduped = Vec::new();
    for session in sessions {
        if emitted.insert((session.client_id.clone(), session.session_id)) {
            deduped.push(session);
            if deduped.len() >= limit.clamp(1, 200) as usize {
                break;
            }
        }
    }
    Ok(deduped)
}

fn terminal_seq_i64(value: u64) -> Result<i64> {
    i64::try_from(value).context("terminal sequence exceeds i64 range")
}

fn u64_to_i64_saturating(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn retention_cap_bytes(value: u64) -> i64 {
    let value = value.min(u64::from(MAX_TERMINAL_FLOW_WINDOW_BYTES));
    if value == 0 {
        i64::from(MAX_TERMINAL_FLOW_WINDOW_BYTES)
    } else {
        u64_to_i64_saturating(value)
    }
}

fn retention_cap_i64(value: i64) -> i64 {
    if value <= 0 {
        i64::from(MAX_TERMINAL_FLOW_WINDOW_BYTES)
    } else {
        value.min(i64::from(MAX_TERMINAL_FLOW_WINDOW_BYTES))
    }
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
#[path = "tests_repository_terminal_sessions.rs"]
mod tests;
