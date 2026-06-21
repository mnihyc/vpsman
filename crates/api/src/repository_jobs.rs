use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use anyhow::{bail, Result};
use base64::Engine as _;
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::{
    job_command_safety, job_command_safety_by_operation_type, payload_hash, CommandOutput,
    JobCommand, JobCommandSafety, DEFAULT_MAX_COMMAND_TIMEOUT_SECS, JOB_COMMAND_SAFETY_EXCLUSIVE,
};
use vpsman_server_core::{
    target_status_is_active, JOB_STATUS_CANCELED, JOB_STATUS_COMPLETED, JOB_STATUS_PARTIAL_SUCCESS,
    JOB_STATUS_QUEUED, JOB_STATUS_RUNNING, JOB_STATUS_SKIPPED, TARGET_STATUS_AGENT_LOST,
    TARGET_STATUS_AGENT_TIMEOUT, TARGET_STATUS_CANCELED, TARGET_STATUS_COMPLETED,
    TARGET_STATUS_CONTROL_TIMEOUT, TARGET_STATUS_DISPATCHING, TARGET_STATUS_FAILED,
    TARGET_STATUS_QUEUED, TARGET_STATUS_REJECTED, TARGET_STATUS_RUNNING, TARGET_STATUS_SKIPPED,
};

pub(crate) use vpsman_server_core::aggregate_job_status_from_statuses;

const EXCLUSIVE_DISPATCH_ADVISORY_LOCK_CLASS: i32 = 0x5650_534d;

use crate::model::*;
use crate::model_webhook_rules::WebhookEventCandidate;
use crate::repository::Repository;
use crate::repository_job_outputs::append_lock_keys;
use crate::repository_terminal_sessions::finalize_active_terminal_input_request_for_terminal_target_in_tx;
use crate::util::{
    limit_or_default, offset_or_default, output_stream_name, search_pattern, sort_descending,
};
use crate::{unix_now, TargetDispatchOutcome};

#[derive(Debug)]
pub(crate) struct PrecompletedJobTarget {
    pub(crate) client_id: String,
    pub(crate) outcome: TargetDispatchOutcome,
}

fn precompleted_targets_by_client<'a>(
    resolved_targets: &[String],
    precompleted_targets: &'a [PrecompletedJobTarget],
) -> Result<HashMap<&'a str, &'a TargetDispatchOutcome>> {
    let resolved = resolved_targets
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut by_client = HashMap::with_capacity(precompleted_targets.len());
    for target in precompleted_targets {
        if !resolved.contains(target.client_id.as_str()) {
            bail!(
                "precompleted target {} is not part of resolved job targets",
                target.client_id
            );
        }
        if by_client
            .insert(target.client_id.as_str(), &target.outcome)
            .is_some()
        {
            bail!("duplicate precompleted target {}", target.client_id);
        }
    }
    Ok(by_client)
}

fn precompleted_output_view(
    job_id: Uuid,
    client_id: &str,
    seq: i32,
    output: &CommandOutput,
    created_at: &str,
) -> JobOutputView {
    JobOutputView {
        job_id,
        client_id: client_id.to_string(),
        seq,
        stream: output_stream_name(output.stream).to_string(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(&output.data),
        storage: "inline".to_string(),
        artifact_object_key: None,
        artifact_sha256_hex: Some(payload_hash(&output.data)),
        artifact_size_bytes: Some(output.data.len() as i64),
        exit_code: output.exit_code,
        done: output.done,
        received_at: None,
        created_at: created_at.to_string(),
    }
}

async fn insert_precompleted_output_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
    seq: i32,
    output: &CommandOutput,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO job_outputs (
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
            received_at
        )
        VALUES ($1, $2, $3, $4, $5, 'inline', NULL, $6, $7, $8, $9, NULL)
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(seq)
    .bind(output_stream_name(output.stream))
    .bind(&output.data)
    .bind(payload_hash(&output.data))
    .bind(output.data.len() as i64)
    .bind(output.exit_code)
    .bind(output.done)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_target_result_audit_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
    outcome: &TargetDispatchOutcome,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, NULL, $2, $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind("job.target_result")
    .bind(format!("client:{client_id}"))
    .bind(json!({
        "job_id": job_id,
        "status": outcome.status,
        "exit_code": outcome.exit_code,
        "accepted": outcome.accepted,
        "message": outcome.message,
        "received_at": outcome.received_at,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn agent_update_activation_failure_status(status: &str) -> bool {
    matches!(
        status,
        TARGET_STATUS_FAILED
            | TARGET_STATUS_REJECTED
            | TARGET_STATUS_AGENT_TIMEOUT
            | TARGET_STATUS_CONTROL_TIMEOUT
            | TARGET_STATUS_AGENT_LOST
            | TARGET_STATUS_CANCELED
    )
}

fn aggregate_schedule_job_outcome_error(status: &str) -> Option<&str> {
    if matches!(
        status,
        JOB_STATUS_COMPLETED
            | JOB_STATUS_PARTIAL_SUCCESS
            | JOB_STATUS_SKIPPED
            | JOB_STATUS_CANCELED
    ) {
        None
    } else {
        Some(status)
    }
}

fn schedule_target_operational_failure_status<'a>(
    statuses: impl IntoIterator<Item = &'a str>,
) -> Option<&'static str> {
    let mut rejected = false;
    let mut failed = false;
    let mut agent_lost = false;
    let mut agent_timeout = false;
    let mut control_timeout = false;
    for status in statuses {
        match status {
            TARGET_STATUS_CONTROL_TIMEOUT => control_timeout = true,
            TARGET_STATUS_AGENT_TIMEOUT => agent_timeout = true,
            TARGET_STATUS_AGENT_LOST => agent_lost = true,
            TARGET_STATUS_FAILED => failed = true,
            TARGET_STATUS_REJECTED => rejected = true,
            _ => {}
        }
    }
    if control_timeout {
        Some(TARGET_STATUS_CONTROL_TIMEOUT)
    } else if agent_timeout {
        Some(TARGET_STATUS_AGENT_TIMEOUT)
    } else if agent_lost {
        Some(TARGET_STATUS_AGENT_LOST)
    } else if failed {
        Some(TARGET_STATUS_FAILED)
    } else if rejected {
        Some(TARGET_STATUS_REJECTED)
    } else {
        None
    }
}

fn schedule_job_outcome_error(
    aggregate_status: &str,
    target_statuses: &[String],
) -> Option<String> {
    schedule_target_operational_failure_status(target_statuses.iter().map(String::as_str))
        .map(ToOwned::to_owned)
        .or_else(|| aggregate_schedule_job_outcome_error(aggregate_status).map(ToOwned::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_operation_types_follow_shared_command_safety() {
        let exclusive = exclusive_operation_types();
        assert!(exclusive.contains(&"hot_config"));
        assert!(exclusive.contains(&"agent_update"));
        assert!(exclusive.contains(&"agent_update_activate"));
        assert!(exclusive.contains(&"agent_update_rollback"));
        assert!(exclusive.contains(&"agent_update_check"));
        assert!(exclusive.contains(&"data_source_config_patch"));
        assert!(!exclusive.contains(&"backup"));
        assert!(!exclusive.contains(&"shell"));
        assert!(!exclusive.contains(&"network_apply"));
        assert!(!exclusive.contains(&"network_speed_test"));
        assert!(!exclusive.contains(&"network_status"));
    }
}

fn agent_lost_status_output_value(
    job_id: Uuid,
    client_id: &str,
    message: &str,
    expected_process_incarnation_id: Option<Uuid>,
    current_process_incarnation_id: Option<Uuid>,
    code: &str,
) -> serde_json::Value {
    json!({
        "type": "agent_lost",
        "status": TARGET_STATUS_AGENT_LOST,
        "code": code,
        "message": message,
        "job_id": job_id,
        "client_id": client_id,
        "previous_process_incarnation_id": expected_process_incarnation_id,
        "process_incarnation_id": current_process_incarnation_id,
        "expected_process_incarnation_id": expected_process_incarnation_id,
        "current_process_incarnation_id": current_process_incarnation_id,
    })
}

fn target_skipped_status_output_value(
    job_id: Uuid,
    client_id: &str,
    reason_code: &str,
    message: &str,
) -> serde_json::Value {
    json!({
        "type": "target_skipped",
        "status": TARGET_STATUS_SKIPPED,
        "code": reason_code,
        "reason": reason_code,
        "message": message,
        "job_id": job_id,
        "client_id": client_id,
    })
}

fn command_canceled_status_output_value(
    job_id: Uuid,
    client_id: &str,
    message: &str,
) -> serde_json::Value {
    json!({
        "type": "command_canceled",
        "status": TARGET_STATUS_CANCELED,
        "code": "operator_cancel_requested",
        "reason": "operator_cancel_requested",
        "message": message,
        "job_id": job_id,
        "client_id": client_id,
    })
}

pub(crate) async fn append_synthetic_status_output_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
    value: serde_json::Value,
    exit_code: Option<i32>,
) -> Result<()> {
    let data = serde_json::to_vec(&value)?;
    let (lock_a, lock_b) = append_lock_keys(job_id, client_id);
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(lock_a)
        .bind(lock_b)
        .execute(&mut **tx)
        .await?;

    for _ in 0..8 {
        let next_seq: i32 = sqlx::query_scalar(
            r#"
            SELECT COALESCE(max(seq) + 1, 0)
            FROM job_outputs
            WHERE job_id = $1 AND client_id = $2
            "#,
        )
        .bind(job_id)
        .bind(client_id)
        .fetch_one(&mut **tx)
        .await?;
        let inserted = sqlx::query(
            r#"
            INSERT INTO job_outputs (
                job_id,
                client_id,
                seq,
                stream,
                data,
                storage,
                data_sha256_hex,
                data_size_bytes,
                exit_code,
                done,
                received_at
            )
            VALUES ($1, $2, $3, 'status', $4, 'inline', $5, $6, $7, true, now())
            ON CONFLICT (job_id, client_id, seq)
            DO NOTHING
            "#,
        )
        .bind(job_id)
        .bind(client_id)
        .bind(next_seq)
        .bind(&data)
        .bind(payload_hash(&data))
        .bind(data.len() as i64)
        .bind(exit_code)
        .execute(&mut **tx)
        .await?;
        if inserted.rows_affected() > 0 {
            return Ok(());
        }
    }
    bail!("agent_lost_output_sequence_conflict:{job_id}:{client_id}")
}

pub(crate) async fn finish_job_in_tx_if_all_targets_terminal(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
) -> Result<Option<String>> {
    let Some(job_row) = sqlx::query(
        r#"
        SELECT completed_at::text AS completed_at
        FROM jobs
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(job_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(None);
    };
    let completed_at: Option<String> = job_row.try_get("completed_at")?;
    if completed_at.is_some() {
        return Ok(None);
    }
    let rows = sqlx::query(
        r#"
        SELECT status
        FROM job_targets
        WHERE job_id = $1
        ORDER BY client_id
        "#,
    )
    .bind(job_id)
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let statuses = rows
        .into_iter()
        .map(|row| row.try_get("status").map_err(Into::into))
        .collect::<Result<Vec<String>>>()?;
    if statuses
        .iter()
        .any(|status| target_status_is_active(status))
    {
        return Ok(None);
    }
    let status = aggregate_job_status_from_statuses(&statuses, statuses.len()).to_string();
    let updated = sqlx::query(
        r#"
        UPDATE jobs
        SET status = $2, completed_at = now()
        WHERE id = $1
          AND completed_at IS NULL
        "#,
    )
    .bind(job_id)
    .bind(&status)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() > 0 {
        Ok(Some(status))
    } else {
        Ok(None)
    }
}

pub(crate) async fn skip_unstarted_queued_targets_for_client_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    reason_code: &str,
    message: &str,
) -> Result<Vec<Uuid>> {
    let rows = sqlx::query(
        r#"
        SELECT job_id, client_id
        FROM job_targets
        WHERE client_id = $1
          AND completed_at IS NULL
          AND status = 'queued'
          AND started_at IS NULL
          AND process_incarnation_id IS NULL
        ORDER BY job_id
        FOR UPDATE
        "#,
    )
    .bind(client_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut job_ids = Vec::new();
    for row in rows {
        let job_id: Uuid = row.try_get("job_id")?;
        let target_client_id: String = row.try_get("client_id")?;
        append_synthetic_status_output_in_tx(
            tx,
            job_id,
            &target_client_id,
            target_skipped_status_output_value(job_id, &target_client_id, reason_code, message),
            Some(0),
        )
        .await?;
        let updated = sqlx::query(
            r#"
            UPDATE job_targets
            SET
                status = 'skipped',
                message = $3,
                exit_code = 0,
                started_at = COALESCE(started_at, now()),
                completed_at = now(),
                dispatch_lease_until = NULL,
                last_dispatch_error = NULL
            WHERE job_id = $1
              AND client_id = $2
              AND completed_at IS NULL
              AND status = 'queued'
              AND started_at IS NULL
              AND process_incarnation_id IS NULL
            "#,
        )
        .bind(job_id)
        .bind(&target_client_id)
        .bind(message)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() > 0 {
            finalize_active_terminal_input_request_for_terminal_target_in_tx(
                tx,
                job_id,
                &target_client_id,
            )
            .await?;
            let _ = finish_job_in_tx_if_all_targets_terminal(tx, job_id).await?;
            job_ids.push(job_id);
        }
    }
    job_ids.sort();
    job_ids.dedup();
    Ok(job_ids)
}

pub(crate) async fn mark_active_targets_agent_lost_for_client_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    expected_process_incarnation_id: Uuid,
    current_process_incarnation_id: Option<Uuid>,
    code: &str,
    message: &str,
) -> Result<Vec<Uuid>> {
    let rows = sqlx::query(
        r#"
        SELECT job_id, client_id
        FROM job_targets
        WHERE client_id = $1
          AND completed_at IS NULL
          AND status IN ('dispatching', 'running')
          AND process_incarnation_id = $2
        ORDER BY job_id
        FOR UPDATE
        "#,
    )
    .bind(client_id)
    .bind(expected_process_incarnation_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut job_ids = Vec::new();
    for row in rows {
        let job_id: Uuid = row.try_get("job_id")?;
        let target_client_id: String = row.try_get("client_id")?;
        append_synthetic_agent_lost_output_with_code_in_tx(
            tx,
            job_id,
            &target_client_id,
            message,
            Some(expected_process_incarnation_id),
            current_process_incarnation_id,
            code,
        )
        .await?;
        let updated = sqlx::query(
            r#"
            UPDATE job_targets
            SET
                status = 'agent_lost',
                message = $3,
                completed_at = now(),
                result_received_at = now(),
                dispatch_lease_until = NULL,
                cancel_requested_at = COALESCE(cancel_requested_at, now()),
                last_dispatch_error = $3
            WHERE job_id = $1
              AND client_id = $2
              AND completed_at IS NULL
              AND status IN ('dispatching', 'running')
              AND process_incarnation_id = $4
            "#,
        )
        .bind(job_id)
        .bind(&target_client_id)
        .bind(message)
        .bind(expected_process_incarnation_id)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() == 0 {
            bail!("agent_lost_target_cas_lost:{job_id}:{target_client_id}");
        }
        finalize_active_terminal_input_request_for_terminal_target_in_tx(
            tx,
            job_id,
            &target_client_id,
        )
        .await?;
        let _ = finish_job_in_tx_if_all_targets_terminal(tx, job_id).await?;
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, actor_id, action, target, command_hash, metadata
            )
            VALUES ($1, NULL, 'job.target_result', $2, NULL, $3)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(format!("client:{target_client_id}"))
        .bind(json!({
            "job_id": job_id,
            "status": TARGET_STATUS_AGENT_LOST,
            "message": message,
            "reason": code,
            "expected_process_incarnation_id": expected_process_incarnation_id,
            "current_process_incarnation_id": current_process_incarnation_id,
        }))
        .execute(&mut **tx)
        .await?;
        job_ids.push(job_id);
    }
    job_ids.sort();
    job_ids.dedup();
    Ok(job_ids)
}

pub(crate) async fn append_synthetic_agent_lost_output_with_code_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
    message: &str,
    expected_process_incarnation_id: Option<Uuid>,
    current_process_incarnation_id: Option<Uuid>,
    code: &str,
) -> Result<()> {
    append_synthetic_status_output_in_tx(
        tx,
        job_id,
        client_id,
        agent_lost_status_output_value(
            job_id,
            client_id,
            message,
            expected_process_incarnation_id,
            current_process_incarnation_id,
            code,
        ),
        None,
    )
    .await
}

pub(crate) async fn append_synthetic_agent_lost_output_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
    message: &str,
    expected_process_incarnation_id: Option<Uuid>,
    current_process_incarnation_id: Option<Uuid>,
) -> Result<()> {
    append_synthetic_agent_lost_output_with_code_in_tx(
        tx,
        job_id,
        client_id,
        message,
        expected_process_incarnation_id,
        current_process_incarnation_id,
        "agent_process_restarted",
    )
    .await
}

fn compare_text_or_number(left: &str, right: &str) -> Ordering {
    match (left.parse::<i128>(), right.parse::<i128>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn compare_job_history(
    left: &JobHistoryView,
    right: &JobHistoryView,
    sort: Option<&str>,
) -> Ordering {
    match sort.unwrap_or("created_at") {
        "actor_id" => left.actor_id.cmp(&right.actor_id),
        "command_type" | "command" => left.command_type.cmp(&right.command_type),
        "payload_hash" | "hash" => left.payload_hash.cmp(&right.payload_hash),
        "privileged" => left.privileged.cmp(&right.privileged),
        "status" => left.status.cmp(&right.status),
        "target_count" | "targets" => left.target_count.cmp(&right.target_count),
        "completed_at" => left.completed_at.cmp(&right.completed_at),
        _ => compare_text_or_number(&left.created_at, &right.created_at),
    }
}

fn job_matches_search(job: &JobHistoryView, needle: &str) -> bool {
    job.id.to_string().to_ascii_lowercase().contains(needle)
        || job
            .actor_id
            .map(|id| id.to_string().to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || job.command_type.to_ascii_lowercase().contains(needle)
        || job.status.to_ascii_lowercase().contains(needle)
        || job.payload_hash.to_ascii_lowercase().contains(needle)
}

fn aggregate_job_status_from_targets(targets: &[JobTargetView]) -> &'static str {
    let statuses = targets
        .iter()
        .map(|target| target.status.clone())
        .collect::<Vec<_>>();
    aggregate_job_status_from_statuses(&statuses, targets.len())
}

fn exclusive_operation_types() -> Vec<&'static str> {
    job_command_safety_by_operation_type()
        .iter()
        .filter_map(|(operation_type, safety)| {
            (*safety == JOB_COMMAND_SAFETY_EXCLUSIVE).then_some(*operation_type)
        })
        .collect()
}

fn job_history_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort.unwrap_or("created_at"), descending) {
        ("actor_id", true) => "actor_id DESC NULLS LAST, id DESC",
        ("actor_id", false) => "actor_id ASC NULLS LAST, id ASC",
        ("command_type" | "command", true) => "command_type DESC, id DESC",
        ("command_type" | "command", false) => "command_type ASC, id ASC",
        ("payload_hash" | "hash", true) => "payload_hash DESC, id DESC",
        ("payload_hash" | "hash", false) => "payload_hash ASC, id ASC",
        ("privileged", true) => "privileged DESC, id DESC",
        ("privileged", false) => "privileged ASC, id ASC",
        ("status", true) => "status DESC, id DESC",
        ("status", false) => "status ASC, id ASC",
        ("target_count" | "targets", true) => "target_count DESC, id DESC",
        ("target_count" | "targets", false) => "target_count ASC, id ASC",
        ("completed_at", true) => "completed_at DESC NULLS LAST, id DESC",
        ("completed_at", false) => "completed_at ASC NULLS LAST, id ASC",
        (_, true) => "created_at DESC, id DESC",
        (_, false) => "created_at ASC, id ASC",
    }
}

fn compare_audit_log(left: &AuditLogView, right: &AuditLogView, sort: Option<&str>) -> Ordering {
    match sort.unwrap_or("created_at") {
        "actor_id" | "operator" => left.actor_id.cmp(&right.actor_id),
        "action" => left.action.cmp(&right.action),
        "command_hash" | "hash" => left.command_hash.cmp(&right.command_hash),
        "target" => left.target.cmp(&right.target),
        _ => compare_text_or_number(&left.created_at, &right.created_at),
    }
}

fn audit_matches_search(audit: &AuditLogView, needle: &str) -> bool {
    audit.id.to_string().to_ascii_lowercase().contains(needle)
        || audit
            .actor_id
            .map(|id| id.to_string().to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || audit.action.to_ascii_lowercase().contains(needle)
        || audit.target.to_ascii_lowercase().contains(needle)
        || audit
            .command_hash
            .as_deref()
            .map(|value| value.to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
}

fn audit_log_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort.unwrap_or("created_at"), descending) {
        ("actor_id" | "operator", true) => "actor_id DESC NULLS LAST, id DESC",
        ("actor_id" | "operator", false) => "actor_id ASC NULLS LAST, id ASC",
        ("action", true) => "action DESC, id DESC",
        ("action", false) => "action ASC, id ASC",
        ("command_hash" | "hash", true) => "command_hash DESC NULLS LAST, id DESC",
        ("command_hash" | "hash", false) => "command_hash ASC NULLS LAST, id ASC",
        ("target", true) => "target DESC, id DESC",
        ("target", false) => "target ASC, id ASC",
        (_, true) => "created_at DESC, id DESC",
        (_, false) => "created_at ASC, id ASC",
    }
}

struct WebhookJobSummary {
    actor_id: Option<Uuid>,
    command_type: String,
    privileged: bool,
    status: String,
    target_count: i32,
    payload_hash: String,
    source_schedule_id: Option<Uuid>,
    targets: Vec<String>,
    target_statuses: Vec<String>,
}

struct JobCreatedWebhookEvent<'a> {
    job_id: Uuid,
    command_type: &'a str,
    status: &'a str,
    privileged: bool,
    command_hash: &'a str,
    resolved_targets: &'a [String],
    actor_id: Option<Uuid>,
    source_schedule_id: Option<Uuid>,
    operation: Option<&'a JobCommand>,
}

struct ScheduleJobOutcome {
    schedule_id: Uuid,
    schedule_name: String,
    job_id: Uuid,
    status: String,
    error: Option<String>,
    enabled: bool,
    failure_count: i32,
    max_failures: i32,
    retry_delay_secs: i64,
    next_run_at: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ClaimedJobTarget {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) actor_id: Option<Uuid>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) command_type: String,
    pub(crate) payload_hash: String,
    pub(crate) process_incarnation_id: Uuid,
    pub(crate) operation: JobCommand,
    pub(crate) source_schedule_id: Option<Uuid>,
    pub(crate) timeout_secs: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct DeadlineExpiredJobTarget {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) status: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct JobCancelPlan {
    pub(crate) cancel_targets: Vec<String>,
    pub(crate) pending_canceled: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct JobCompletionContext {
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) payload_hash: String,
    pub(crate) operation: JobCommand,
}

fn job_webhook_predicates(command_type: &str, status: &str, include_created: bool) -> Vec<String> {
    let mut predicates = vec![
        format!("job.status:{status}"),
        format!("job.status.become_{status}"),
        format!("job.type:{command_type}"),
    ];
    if include_created {
        predicates.push("job.created".to_string());
    }
    predicates.sort();
    predicates.dedup();
    predicates
}

impl Repository {
    pub(crate) async fn get_job(&self, job_id: Uuid) -> Result<Option<JobHistoryView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .jobs
                .read()
                .await
                .iter()
                .find(|job| job.id == job_id)
                .cloned()),
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        command_type,
                        privileged,
                        status,
                        target_count,
                        payload_hash,
                        timeout_secs,
                        created_at::text AS created_at,
                        completed_at::text AS completed_at
                    FROM jobs
                    WHERE id = $1
                    "#,
                )
                .bind(job_id)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(None);
                };
                Ok(Some(JobHistoryView {
                    id: row.try_get("id")?,
                    actor_id: row.try_get("actor_id")?,
                    command_type: row.try_get("command_type")?,
                    privileged: row.try_get("privileged")?,
                    status: row.try_get("status")?,
                    target_count: row.try_get("target_count")?,
                    payload_hash: row.try_get("payload_hash")?,
                    timeout_secs: row.try_get::<i64, _>("timeout_secs")?.max(1) as u64,
                    created_at: row.try_get("created_at")?,
                    completed_at: row.try_get("completed_at")?,
                }))
            }
        }
    }

    pub(crate) async fn get_job_completion_context(
        &self,
        job_id: Uuid,
    ) -> Result<Option<JobCompletionContext>> {
        match self {
            Self::Memory(memory) => {
                let Some(job) = memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .find(|job| job.id == job_id)
                    .cloned()
                else {
                    return Ok(None);
                };
                let Some(operation) = memory.job_operations.read().await.get(&job_id).cloned()
                else {
                    return Ok(None);
                };
                Ok(Some(JobCompletionContext {
                    actor_id: job.actor_id,
                    payload_hash: job.payload_hash,
                    operation,
                }))
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT actor_id, payload_hash, operation
                    FROM jobs
                    WHERE id = $1
                    "#,
                )
                .bind(job_id)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(None);
                };
                let operation: sqlx::types::Json<JobCommand> = row.try_get("operation")?;
                Ok(Some(JobCompletionContext {
                    actor_id: row.try_get("actor_id")?,
                    payload_hash: row.try_get("payload_hash")?,
                    operation: operation.0,
                }))
            }
        }
    }

    pub(crate) async fn get_job_request_fingerprint(&self, job_id: Uuid) -> Result<Option<String>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .job_request_fingerprints
                .read()
                .await
                .get(&job_id)
                .cloned()),
            Self::Postgres(pool) => sqlx::query_scalar(
                r#"
                    SELECT request_fingerprint
                    FROM jobs
                    WHERE id = $1
                    "#,
            )
            .bind(job_id)
            .fetch_optional(pool)
            .await
            .map_err(Into::into),
        }
    }

    pub(crate) async fn list_jobs(&self, limit: i64) -> Result<Vec<JobHistoryView>> {
        match self {
            Self::Memory(memory) => {
                let jobs = memory.jobs.read().await;
                Ok(jobs.iter().rev().take(limit as usize).cloned().collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        command_type,
                        privileged,
                        status,
                        target_count,
                        payload_hash,
                        timeout_secs,
                        created_at::text AS created_at,
                        completed_at::text AS completed_at
                    FROM jobs
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(JobHistoryView {
                            id: row.try_get("id")?,
                            actor_id: row.try_get("actor_id")?,
                            command_type: row.try_get("command_type")?,
                            privileged: row.try_get("privileged")?,
                            status: row.try_get("status")?,
                            target_count: row.try_get("target_count")?,
                            payload_hash: row.try_get("payload_hash")?,
                            timeout_secs: row.try_get::<i64, _>("timeout_secs")?.max(1) as u64,
                            created_at: row.try_get("created_at")?,
                            completed_at: row.try_get("completed_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn query_jobs(&self, query: &ListQuery) -> Result<Vec<JobHistoryView>> {
        let limit = limit_or_default(query.limit);
        let offset = offset_or_default(query.offset);
        let descending = sort_descending(query.dir.as_deref(), true);
        let q = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        match self {
            Self::Memory(memory) => {
                let q = q.map(|value| value.to_ascii_lowercase());
                let mut jobs = memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .filter(|job| {
                        q.as_deref()
                            .map(|needle| job_matches_search(job, needle))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                jobs.sort_by(|left, right| {
                    compare_job_history(left, right, query.sort.as_deref())
                        .then_with(|| left.id.cmp(&right.id))
                });
                if descending {
                    jobs.reverse();
                }
                Ok(jobs
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect())
            }
            Self::Postgres(pool) => {
                let order_by = job_history_order_by(query.sort.as_deref(), descending);
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        command_type,
                        privileged,
                        status,
                        target_count,
                        payload_hash,
                        timeout_secs,
                        created_at::text AS created_at,
                        completed_at::text AS completed_at
                    FROM jobs
                    WHERE (
                        $3::text IS NULL
                        OR id::text ILIKE $3 ESCAPE '\'
                        OR actor_id::text ILIKE $3 ESCAPE '\'
                        OR command_type ILIKE $3 ESCAPE '\'
                        OR status ILIKE $3 ESCAPE '\'
                        OR payload_hash ILIKE $3 ESCAPE '\'
                    )
                    ORDER BY {order_by}
                    LIMIT $1
                    OFFSET $2
                    "#,
                ))
                .bind(limit)
                .bind(offset)
                .bind(search_pattern(&query.q))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(JobHistoryView {
                            id: row.try_get("id")?,
                            actor_id: row.try_get("actor_id")?,
                            command_type: row.try_get("command_type")?,
                            privileged: row.try_get("privileged")?,
                            status: row.try_get("status")?,
                            target_count: row.try_get("target_count")?,
                            payload_hash: row.try_get("payload_hash")?,
                            timeout_secs: row.try_get::<i64, _>("timeout_secs")?.max(1) as u64,
                            created_at: row.try_get("created_at")?,
                            completed_at: row.try_get("completed_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_job_targets(&self, job_id: Uuid) -> Result<Vec<JobTargetView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .job_targets
                .read()
                .await
                .iter()
                .filter(|target| target.job_id == job_id)
                .cloned()
                .collect()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        job_id,
                        client_id,
                        status,
                        message,
                        exit_code,
                        started_at::text AS started_at,
                        deadline_at::text AS deadline_at,
                        completed_at::text AS completed_at,
                        process_incarnation_id
                    FROM job_targets
                    WHERE job_id = $1
                    ORDER BY client_id
                    "#,
                )
                .bind(job_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(JobTargetView {
                            job_id: row.try_get("job_id")?,
                            client_id: row.try_get("client_id")?,
                            status: row.try_get("status")?,
                            message: row.try_get("message")?,
                            exit_code: row.try_get("exit_code")?,
                            started_at: row.try_get("started_at")?,
                            deadline_at: row.try_get("deadline_at")?,
                            completed_at: row.try_get("completed_at")?,
                            process_incarnation_id: row.try_get("process_incarnation_id")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn active_job_target_client_ids(
        &self,
        client_ids: &[String],
        exclude_job_id: Uuid,
    ) -> Result<HashSet<String>> {
        if client_ids.is_empty() {
            return Ok(HashSet::new());
        }
        match self {
            Self::Memory(memory) => Ok(memory
                .job_targets
                .read()
                .await
                .iter()
                .filter(|target| target.job_id != exclude_job_id)
                .filter(|target| target.completed_at.is_none())
                .filter(|target| target_status_is_active(&target.status))
                .filter(|target| {
                    client_ids
                        .iter()
                        .any(|client_id| client_id == &target.client_id)
                })
                .map(|target| target.client_id.clone())
                .collect()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT DISTINCT client_id
                    FROM job_targets
                    WHERE client_id = ANY($1::text[])
                      AND job_id <> $2
                      AND completed_at IS NULL
                      AND status IN ('queued', 'dispatching', 'running')
                    "#,
                )
                .bind(client_ids)
                .bind(exclude_job_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| row.try_get("client_id").map_err(Into::into))
                    .collect()
            }
        }
    }

    pub(crate) async fn list_audit_logs(&self, limit: i64) -> Result<Vec<AuditLogView>> {
        match self {
            Self::Memory(memory) => {
                let audits = memory.audits.read().await;
                Ok(audits.iter().rev().take(limit as usize).cloned().collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        action,
                        target,
                        command_hash,
                        metadata,
                        created_at::text AS created_at
                    FROM audit_logs
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let metadata: sqlx::types::Json<serde_json::Value> =
                            row.try_get("metadata")?;
                        Ok(AuditLogView {
                            id: row.try_get("id")?,
                            actor_id: row.try_get("actor_id")?,
                            action: row.try_get("action")?,
                            target: row.try_get("target")?,
                            command_hash: row.try_get("command_hash")?,
                            metadata: metadata.0,
                            created_at: row.try_get("created_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn query_audit_logs(&self, query: &ListQuery) -> Result<Vec<AuditLogView>> {
        let limit = limit_or_default(query.limit);
        let offset = offset_or_default(query.offset);
        let descending = sort_descending(query.dir.as_deref(), true);
        let q = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        match self {
            Self::Memory(memory) => {
                let q = q.map(|value| value.to_ascii_lowercase());
                let mut audits = memory
                    .audits
                    .read()
                    .await
                    .iter()
                    .filter(|audit| {
                        q.as_deref()
                            .map(|needle| audit_matches_search(audit, needle))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                audits.sort_by(|left, right| {
                    compare_audit_log(left, right, query.sort.as_deref())
                        .then_with(|| left.id.cmp(&right.id))
                });
                if descending {
                    audits.reverse();
                }
                Ok(audits
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect())
            }
            Self::Postgres(pool) => {
                let order_by = audit_log_order_by(query.sort.as_deref(), descending);
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        action,
                        target,
                        command_hash,
                        metadata,
                        created_at::text AS created_at
                    FROM audit_logs
                    WHERE (
                        $3::text IS NULL
                        OR id::text ILIKE $3 ESCAPE '\'
                        OR actor_id::text ILIKE $3 ESCAPE '\'
                        OR action ILIKE $3 ESCAPE '\'
                        OR target ILIKE $3 ESCAPE '\'
                        OR command_hash ILIKE $3 ESCAPE '\'
                    )
                    ORDER BY {order_by}
                    LIMIT $1
                    OFFSET $2
                    "#,
                ))
                .bind(limit)
                .bind(offset)
                .bind(search_pattern(&query.q))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let metadata: sqlx::types::Json<serde_json::Value> =
                            row.try_get("metadata")?;
                        Ok(AuditLogView {
                            id: row.try_get("id")?,
                            actor_id: row.try_get("actor_id")?,
                            action: row.try_get("action")?,
                            target: row.try_get("target")?,
                            command_hash: row.try_get("command_hash")?,
                            metadata: metadata.0,
                            created_at: row.try_get("created_at")?,
                        })
                    })
                    .collect()
            }
        }
    }
    pub(crate) async fn record_rejected_job(
        &self,
        job_id: Uuid,
        request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        operator: &AuthContext,
        status: &str,
        reason: &str,
    ) -> Result<Uuid> {
        let resolved_targets = request.fixed_target_ids().unwrap_or_default();
        let metadata = json!({
            "selector_expression": request.selector_expression,
            "resolved_targets": &resolved_targets,
            "destructive": request.destructive,
            "confirmed": request.confirmed,
            "privileged": request.privileged,
            "force_unprivileged": request.force_unprivileged,
            "operator_id": operator.operator.id,
            "operator_username": operator.operator.username,
            "operator_role": operator.operator.role,
            "session_id": operator.session_id,
            "reason": reason,
        });
        let operation = request.job_command().ok();
        match self {
            Self::Memory(memory) => {
                let created_at = unix_now().to_string();
                memory.jobs.write().await.push(JobHistoryView {
                    id: job_id,
                    actor_id: Some(operator.operator.id),
                    command_type: "api_job_request".to_string(),
                    privileged: request.privileged,
                    status: status.to_string(),
                    target_count: resolved_targets.len() as i32,
                    payload_hash: command_hash.to_string(),
                    timeout_secs: request
                        .timeout_secs
                        .unwrap_or(DEFAULT_MAX_COMMAND_TIMEOUT_SECS)
                        .max(1),
                    created_at: created_at.clone(),
                    completed_at: Some(created_at.clone()),
                });
                memory
                    .job_request_fingerprints
                    .write()
                    .await
                    .insert(job_id, request_fingerprint.to_string());
                memory
                    .job_targets
                    .write()
                    .await
                    .extend(
                        resolved_targets
                            .iter()
                            .cloned()
                            .map(|client_id| JobTargetView {
                                job_id,
                                client_id,
                                status: status.to_string(),
                                message: Some(reason.to_string()),
                                exit_code: None,
                                started_at: None,
                                deadline_at: None,
                                completed_at: Some(created_at.clone()),
                                process_incarnation_id: None,
                            }),
                    );
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: format!("job.{status}"),
                    target: "api:/api/v1/jobs".to_string(),
                    command_hash: Some(command_hash.to_string()),
                    metadata,
                    created_at,
                });
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    INSERT INTO jobs (
                        id, actor_id, command_type, privileged, status,
                        target_count, payload_hash, operation, request_fingerprint,
                        timeout_secs, completed_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now())
                    "#,
                )
                .bind(job_id)
                .bind(operator.operator.id)
                .bind("api_job_request")
                .bind(request.privileged)
                .bind(status)
                .bind(resolved_targets.len() as i32)
                .bind(command_hash)
                .bind(operation.clone().map(sqlx::types::Json))
                .bind(request_fingerprint)
                .bind(
                    request
                        .timeout_secs
                        .unwrap_or(DEFAULT_MAX_COMMAND_TIMEOUT_SECS) as i64,
                )
                .execute(&mut *tx)
                .await?;
                for client_id in &resolved_targets {
                    sqlx::query(
                        r#"
                        INSERT INTO job_targets (
                            job_id, client_id, status, message, completed_at
                        )
                        VALUES ($1, $2, $3, $4, now())
                        "#,
                    )
                    .bind(job_id)
                    .bind(client_id)
                    .bind(status)
                    .bind(reason)
                    .execute(&mut *tx)
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
                .bind(format!("job.{status}"))
                .bind("api:/api/v1/jobs")
                .bind(command_hash)
                .bind(metadata)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        self.record_job_created_webhook_event(JobCreatedWebhookEvent {
            job_id,
            command_type: "api_job_request",
            status,
            privileged: request.privileged,
            command_hash,
            resolved_targets: &resolved_targets,
            actor_id: Some(operator.operator.id),
            source_schedule_id: None,
            operation: operation.as_ref(),
        })
        .await?;
        Ok(job_id)
    }

    #[cfg(test)]
    pub(crate) async fn record_dispatching_job(
        &self,
        job_id: Uuid,
        request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
    ) -> Result<Uuid> {
        self.record_dispatching_job_with_source(
            job_id,
            request,
            command_hash,
            request_fingerprint,
            operator,
            resolved_targets,
            None,
            &[],
        )
        .await
    }

    pub(crate) async fn record_dispatching_job_with_precompleted(
        &self,
        job_id: Uuid,
        request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
        precompleted_targets: &[PrecompletedJobTarget],
    ) -> Result<Uuid> {
        self.record_dispatching_job_with_source(
            job_id,
            request,
            command_hash,
            request_fingerprint,
            operator,
            resolved_targets,
            None,
            precompleted_targets,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn record_dispatching_job_from_schedule(
        &self,
        job_id: Uuid,
        request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
        source_schedule_id: Uuid,
    ) -> Result<Uuid> {
        self.record_dispatching_job_with_source(
            job_id,
            request,
            command_hash,
            request_fingerprint,
            operator,
            resolved_targets,
            Some(source_schedule_id),
            &[],
        )
        .await
    }

    pub(crate) async fn record_dispatching_job_from_schedule_with_precompleted(
        &self,
        job_id: Uuid,
        request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
        source_schedule_id: Uuid,
        precompleted_targets: &[PrecompletedJobTarget],
    ) -> Result<Uuid> {
        self.record_dispatching_job_with_source(
            job_id,
            request,
            command_hash,
            request_fingerprint,
            operator,
            resolved_targets,
            Some(source_schedule_id),
            precompleted_targets,
        )
        .await
    }

    async fn record_dispatching_job_with_source(
        &self,
        job_id: Uuid,
        request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
        source_schedule_id: Option<Uuid>,
        precompleted_targets: &[PrecompletedJobTarget],
    ) -> Result<Uuid> {
        let command_type = request.command_type_label().to_string();
        let metadata = json!({
            "selector_expression": request.selector_expression,
            "resolved_targets": resolved_targets,
            "destructive": request.destructive,
            "confirmed": request.confirmed,
            "privileged": request.privileged,
            "force_unprivileged": request.force_unprivileged,
            "source_schedule_id": source_schedule_id,
            "operator_id": operator.operator.id,
            "operator_username": operator.operator.username,
            "operator_role": operator.operator.role,
            "session_id": operator.session_id,
        });
        let operation = request
            .job_command()
            .map_err(|error| anyhow::anyhow!(error.code))?;
        let precompleted_by_client =
            precompleted_targets_by_client(resolved_targets, precompleted_targets)?;
        let mut finished_status = None::<String>;
        match self {
            Self::Memory(memory) => {
                let created_at = unix_now().to_string();
                memory.jobs.write().await.push(JobHistoryView {
                    id: job_id,
                    actor_id: Some(operator.operator.id),
                    command_type: command_type.clone(),
                    privileged: request.privileged,
                    status: JOB_STATUS_QUEUED.to_string(),
                    target_count: resolved_targets.len() as i32,
                    payload_hash: command_hash.to_string(),
                    timeout_secs: request
                        .timeout_secs
                        .unwrap_or(DEFAULT_MAX_COMMAND_TIMEOUT_SECS)
                        .max(1),
                    created_at: created_at.clone(),
                    completed_at: None,
                });
                memory
                    .job_request_fingerprints
                    .write()
                    .await
                    .insert(job_id, request_fingerprint.to_string());
                memory
                    .job_operations
                    .write()
                    .await
                    .insert(job_id, operation.clone());
                memory.job_timeouts.write().await.insert(
                    job_id,
                    request
                        .timeout_secs
                        .unwrap_or(DEFAULT_MAX_COMMAND_TIMEOUT_SECS)
                        .max(1),
                );
                if let Some(schedule_id) = source_schedule_id {
                    memory
                        .job_source_schedule_ids
                        .write()
                        .await
                        .insert(job_id, schedule_id);
                }
                memory
                    .job_targets
                    .write()
                    .await
                    .extend(resolved_targets.iter().cloned().map(|client_id| {
                        JobTargetView {
                            job_id,
                            status: precompleted_by_client
                                .get(client_id.as_str())
                                .map(|outcome| outcome.status.clone())
                                .unwrap_or_else(|| TARGET_STATUS_QUEUED.to_string()),
                            message: precompleted_by_client
                                .get(client_id.as_str())
                                .map(|outcome| outcome.message.clone()),
                            exit_code: precompleted_by_client
                                .get(client_id.as_str())
                                .and_then(|outcome| outcome.exit_code),
                            started_at: precompleted_by_client
                                .contains_key(client_id.as_str())
                                .then_some(created_at.clone()),
                            deadline_at: None,
                            completed_at: precompleted_by_client
                                .contains_key(client_id.as_str())
                                .then_some(created_at.clone()),
                            process_incarnation_id: None,
                            client_id,
                        }
                    }));
                if !precompleted_targets.is_empty() {
                    let mut outputs = memory.job_outputs.write().await;
                    for target in precompleted_targets {
                        for (index, output) in target.outcome.outputs.iter().enumerate() {
                            outputs.push(precompleted_output_view(
                                job_id,
                                &target.client_id,
                                i32::try_from(index)?,
                                output,
                                &created_at,
                            ));
                        }
                    }
                }
                for target in precompleted_targets {
                    self.finalize_active_terminal_input_request_for_target_status(
                        job_id,
                        &target.client_id,
                        &target.outcome.status,
                    )
                    .await?;
                }
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "job.dispatch_requested".to_string(),
                    target: "api:/api/v1/jobs".to_string(),
                    command_hash: Some(command_hash.to_string()),
                    metadata,
                    created_at: created_at.clone(),
                });
                if !precompleted_targets.is_empty() {
                    let mut audits = memory.audits.write().await;
                    for target in precompleted_targets {
                        audits.push(AuditLogView {
                            id: Uuid::new_v4(),
                            actor_id: None,
                            action: "job.target_result".to_string(),
                            target: format!("client:{}", target.client_id),
                            command_hash: None,
                            metadata: json!({
                                "job_id": job_id,
                                "status": target.outcome.status,
                                "exit_code": target.outcome.exit_code,
                                "accepted": target.outcome.accepted,
                                "message": target.outcome.message,
                                "received_at": target.outcome.received_at,
                            }),
                            created_at: created_at.clone(),
                        });
                    }
                }
                let target_statuses = resolved_targets
                    .iter()
                    .map(|client_id| {
                        precompleted_by_client
                            .get(client_id.as_str())
                            .map(|outcome| outcome.status.as_str())
                            .unwrap_or(TARGET_STATUS_QUEUED)
                    })
                    .collect::<Vec<_>>();
                if !target_statuses.is_empty()
                    && !target_statuses
                        .iter()
                        .any(|status| target_status_is_active(status))
                {
                    let status = aggregate_job_status_from_statuses(
                        &target_statuses
                            .iter()
                            .map(|status| (*status).to_string())
                            .collect::<Vec<_>>(),
                        target_statuses.len(),
                    )
                    .to_string();
                    if let Some(job) = memory
                        .jobs
                        .write()
                        .await
                        .iter_mut()
                        .find(|job| job.id == job_id && job.completed_at.is_none())
                    {
                        job.status = status.clone();
                        job.completed_at = Some(created_at.clone());
                        finished_status = Some(status);
                    }
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    INSERT INTO jobs (
                        id, actor_id, command_type, privileged, status,
                        target_count, payload_hash, operation, source_schedule_id, request_fingerprint,
                        timeout_secs
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                    "#,
                )
                .bind(job_id)
                .bind(operator.operator.id)
                .bind(&command_type)
                .bind(request.privileged)
                .bind(JOB_STATUS_QUEUED)
                .bind(resolved_targets.len() as i32)
                .bind(command_hash)
                .bind(sqlx::types::Json(operation.clone()))
                .bind(source_schedule_id)
                .bind(request_fingerprint)
                .bind(request.timeout_secs.unwrap_or(DEFAULT_MAX_COMMAND_TIMEOUT_SECS) as i64)
                .execute(&mut *tx)
                .await?;
                for client_id in resolved_targets {
                    if let Some(outcome) = precompleted_by_client.get(client_id.as_str()) {
                        sqlx::query(
                            r#"
                            INSERT INTO job_targets (
                                job_id,
                                client_id,
                                status,
                                message,
                                exit_code,
                                started_at,
                                completed_at,
                                result_received_at
                            )
                            VALUES ($1, $2, $3, $4, $5, now(), now(), COALESCE($6::timestamptz, now()))
                            "#,
                        )
                        .bind(job_id)
                        .bind(client_id)
                        .bind(&outcome.status)
                        .bind(&outcome.message)
                        .bind(outcome.exit_code)
                        .bind(outcome.received_at.as_deref())
                        .execute(&mut *tx)
                        .await?;
                        for (index, output) in outcome.outputs.iter().enumerate() {
                            insert_precompleted_output_in_tx(
                                &mut tx,
                                job_id,
                                client_id,
                                i32::try_from(index)?,
                                output,
                            )
                            .await?;
                        }
                        finalize_active_terminal_input_request_for_terminal_target_in_tx(
                            &mut tx, job_id, client_id,
                        )
                        .await?;
                        insert_target_result_audit_in_tx(&mut tx, job_id, client_id, outcome)
                            .await?;
                    } else {
                        sqlx::query(
                            r#"
                            INSERT INTO job_targets (
                                job_id, client_id, status, message
                            )
                            VALUES ($1, $2, $3, NULL)
                            "#,
                        )
                        .bind(job_id)
                        .bind(client_id)
                        .bind(TARGET_STATUS_QUEUED)
                        .execute(&mut *tx)
                        .await?;
                    }
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
                .bind("job.dispatch_requested")
                .bind("api:/api/v1/jobs")
                .bind(command_hash)
                .bind(metadata)
                .execute(&mut *tx)
                .await?;
                finished_status = finish_job_in_tx_if_all_targets_terminal(&mut tx, job_id).await?;
                tx.commit().await?;
            }
        }
        self.record_job_created_webhook_event(JobCreatedWebhookEvent {
            job_id,
            command_type: &command_type,
            status: finished_status.as_deref().unwrap_or(JOB_STATUS_QUEUED),
            privileged: request.privileged,
            command_hash,
            resolved_targets,
            actor_id: Some(operator.operator.id),
            source_schedule_id,
            operation: Some(&operation),
        })
        .await?;
        for target in precompleted_targets {
            self.record_job_target_webhook_event(job_id, &target.client_id, &target.outcome)
                .await?;
        }
        if let Some(status) = finished_status {
            self.record_job_terminal_side_effects(job_id, &status, None)
                .await?;
        }
        Ok(job_id)
    }

    pub(crate) async fn claim_due_job_targets(
        &self,
        limit: i64,
        lease_secs: i64,
        control_deadline_extra_secs: u64,
    ) -> Result<Vec<ClaimedJobTarget>> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let operations = memory.job_operations.read().await.clone();
                let source_schedule_ids = memory.job_source_schedule_ids.read().await.clone();
                let timeouts = memory.job_timeouts.read().await.clone();
                let jobs = memory.jobs.read().await.clone();
                let target_snapshot = memory.job_targets.read().await.clone();
                let mut active_clients = target_snapshot
                    .iter()
                    .filter(|target| {
                        target.completed_at.is_none()
                            && matches!(
                                target.status.as_str(),
                                TARGET_STATUS_DISPATCHING | TARGET_STATUS_RUNNING
                            )
                    })
                    .map(|target| target.client_id.clone())
                    .collect::<std::collections::HashSet<_>>();
                let mut active_exclusive_clients = target_snapshot
                    .iter()
                    .filter(|target| {
                        target.completed_at.is_none()
                            && matches!(
                                target.status.as_str(),
                                TARGET_STATUS_DISPATCHING | TARGET_STATUS_RUNNING
                            )
                    })
                    .filter_map(|target| {
                        let operation = operations.get(&target.job_id)?;
                        (job_command_safety(operation) == JobCommandSafety::Exclusive)
                            .then(|| target.client_id.clone())
                    })
                    .collect::<std::collections::HashSet<_>>();
                let mut targets = memory.job_targets.write().await;
                let mut claimed = Vec::new();
                for target in targets.iter_mut().filter(|target| {
                    target.completed_at.is_none() && target.status == TARGET_STATUS_QUEUED
                }) {
                    if claimed.len() >= limit.clamp(1, 500) as usize {
                        break;
                    }
                    let Some(job) = jobs.iter().find(|job| job.id == target.job_id) else {
                        continue;
                    };
                    let Some(operation) = operations.get(&target.job_id).cloned() else {
                        continue;
                    };
                    let is_exclusive =
                        job_command_safety(&operation) == JobCommandSafety::Exclusive;
                    if (is_exclusive && active_clients.contains(&target.client_id))
                        || (!is_exclusive && active_exclusive_clients.contains(&target.client_id))
                    {
                        continue;
                    }
                    let timeout_secs = timeouts
                        .get(&target.job_id)
                        .copied()
                        .unwrap_or(DEFAULT_MAX_COMMAND_TIMEOUT_SECS)
                        .max(1);
                    target.status = TARGET_STATUS_DISPATCHING.to_string();
                    target.started_at.get_or_insert_with(|| now.clone());
                    if is_exclusive {
                        active_exclusive_clients.insert(target.client_id.clone());
                    }
                    active_clients.insert(target.client_id.clone());
                    claimed.push(ClaimedJobTarget {
                        job_id: target.job_id,
                        client_id: target.client_id.clone(),
                        actor_id: job.actor_id,
                        command_type: job.command_type.clone(),
                        payload_hash: job.payload_hash.clone(),
                        process_incarnation_id: Uuid::nil(),
                        operation,
                        source_schedule_id: source_schedule_ids.get(&target.job_id).copied(),
                        timeout_secs,
                    });
                }
                let claimed_job_ids = claimed
                    .iter()
                    .map(|target| target.job_id)
                    .collect::<std::collections::HashSet<_>>();
                drop(targets);
                if !claimed_job_ids.is_empty() {
                    let mut jobs = memory.jobs.write().await;
                    for job in jobs.iter_mut().filter(|job| {
                        claimed_job_ids.contains(&job.id)
                            && job.completed_at.is_none()
                            && job.status == JOB_STATUS_QUEUED
                    }) {
                        job.status = JOB_STATUS_RUNNING.to_string();
                    }
                }
                Ok(claimed)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH due AS (
                        SELECT
                            target.job_id,
                            target.client_id,
                            job.actor_id,
                            job.command_type,
                            job.payload_hash,
                            job.operation,
                            job.source_schedule_id,
                            job.timeout_secs,
                            clients.process_incarnation_id AS client_process_incarnation_id
                        FROM job_targets target
                        JOIN jobs job ON job.id = target.job_id
                        JOIN clients ON clients.id = target.client_id
                        WHERE target.completed_at IS NULL
                              AND target.cancel_requested_at IS NULL
                              AND target.status IN ('queued', 'dispatching')
                              AND job.completed_at IS NULL
                              AND job.status IN ('queued', 'running')
                              AND clients.hidden_at IS NULL
                              AND clients.process_incarnation_id IS NOT NULL
                              AND (
                                (
                                  target.status = 'queued'
                                  AND target.started_at IS NULL
                                  AND target.process_incarnation_id IS NULL
                                )
                                OR (
                                  target.status = 'dispatching'
                                  AND target.started_at IS NOT NULL
                                  AND target.process_incarnation_id IS NOT NULL
                                  AND target.process_incarnation_id = clients.process_incarnation_id
                                  AND target.deadline_at IS NOT NULL
                                  AND target.deadline_at > now()
                                )
                              )
                              AND (
                                (
                                  COALESCE(job.operation ->> 'type', '') <> ALL($3::text[])
                                      AND pg_try_advisory_xact_lock(
                                        $4::integer,
                                        hashtext(target.client_id)
                                      )
                                  AND NOT EXISTS (
                                    SELECT 1
                                    FROM job_targets active_target
                                    JOIN jobs active_job
                                      ON active_job.id = active_target.job_id
                                    WHERE active_target.client_id = target.client_id
                                      AND active_target.completed_at IS NULL
                                      AND active_target.status IN ('dispatching', 'running')
                                      AND active_target.started_at IS NOT NULL
                                      AND active_target.process_incarnation_id IS NOT NULL
                                      AND active_job.completed_at IS NULL
                                      AND COALESCE(active_job.operation ->> 'type', '') = ANY($3::text[])
                                      AND (
                                        active_target.job_id <> target.job_id
                                        OR active_target.client_id <> target.client_id
                                      )
                                  )
                                  AND NOT EXISTS (
                                    SELECT 1
                                    FROM job_targets earlier_target
                                    JOIN jobs earlier_job
                                      ON earlier_job.id = earlier_target.job_id
                                    WHERE earlier_target.client_id = target.client_id
                                      AND earlier_target.completed_at IS NULL
                                      AND earlier_target.cancel_requested_at IS NULL
                                      AND earlier_target.status IN ('queued', 'dispatching')
                                      AND earlier_job.completed_at IS NULL
                                      AND earlier_job.status IN ('queued', 'running')
                                      AND COALESCE(earlier_job.operation ->> 'type', '') = ANY($3::text[])
                                      AND (
                                        (
                                          earlier_target.status = 'queued'
                                          AND earlier_target.started_at IS NULL
                                          AND earlier_target.process_incarnation_id IS NULL
                                        )
                                        OR (
                                          earlier_target.status = 'dispatching'
                                          AND earlier_target.started_at IS NOT NULL
                                          AND earlier_target.process_incarnation_id IS NOT NULL
                                          AND earlier_target.process_incarnation_id = clients.process_incarnation_id
                                          AND earlier_target.deadline_at IS NOT NULL
                                          AND earlier_target.deadline_at > now()
                                        )
                                      )
                                      AND (
                                        earlier_target.status = 'queued'
                                        OR earlier_target.dispatch_lease_until IS NULL
                                        OR earlier_target.dispatch_lease_until < now()
                                      )
                                      AND (
                                        earlier_job.created_at,
                                        earlier_target.job_id,
                                        earlier_target.client_id
                                      ) < (
                                        job.created_at,
                                        target.job_id,
                                        target.client_id
                                      )
                                  )
                                )
                                OR (
                                  COALESCE(job.operation ->> 'type', '') = ANY($3::text[])
                                  AND pg_try_advisory_xact_lock(
                                    $4::integer,
                                    hashtext(target.client_id)
                                  )
                                  AND NOT EXISTS (
                                    SELECT 1
                                    FROM job_targets active_target
                                    JOIN jobs active_job
                                      ON active_job.id = active_target.job_id
                                    WHERE active_target.client_id = target.client_id
                                      AND active_target.completed_at IS NULL
                                      AND active_target.status IN ('dispatching', 'running')
                                      AND active_target.started_at IS NOT NULL
                                      AND active_target.process_incarnation_id IS NOT NULL
                                      AND active_job.completed_at IS NULL
                                      AND (
                                        active_target.job_id <> target.job_id
                                        OR active_target.client_id <> target.client_id
                                      )
                                  )
                                  AND NOT EXISTS (
                                    SELECT 1
                                    FROM job_targets earlier_target
                                    JOIN jobs earlier_job
                                      ON earlier_job.id = earlier_target.job_id
                                    WHERE earlier_target.client_id = target.client_id
                                      AND earlier_target.completed_at IS NULL
                                      AND earlier_target.cancel_requested_at IS NULL
                                      AND earlier_target.status IN ('queued', 'dispatching')
                                      AND earlier_job.completed_at IS NULL
                                      AND earlier_job.status IN ('queued', 'running')
                                      AND (
                                        (
                                          earlier_target.status = 'queued'
                                          AND earlier_target.started_at IS NULL
                                          AND earlier_target.process_incarnation_id IS NULL
                                        )
                                        OR (
                                          earlier_target.status = 'dispatching'
                                          AND earlier_target.started_at IS NOT NULL
                                          AND earlier_target.process_incarnation_id IS NOT NULL
                                          AND earlier_target.process_incarnation_id = clients.process_incarnation_id
                                          AND earlier_target.deadline_at IS NOT NULL
                                          AND earlier_target.deadline_at > now()
                                        )
                                      )
                                      AND (
                                        earlier_target.status = 'queued'
                                        OR earlier_target.dispatch_lease_until IS NULL
                                        OR earlier_target.dispatch_lease_until < now()
                                      )
                                      AND (
                                        earlier_job.created_at,
                                        earlier_target.job_id,
                                        earlier_target.client_id
                                      ) < (
                                        job.created_at,
                                        target.job_id,
                                        target.client_id
                                      )
                                  )
                                )
                              )
                              AND (
                                target.status = 'queued'
                                OR target.dispatch_lease_until IS NULL
                                OR target.dispatch_lease_until < now()
                              )
                        ORDER BY job.created_at ASC, target.client_id ASC
                        LIMIT $1
                        FOR UPDATE SKIP LOCKED
                    ),
                    updated_targets AS (
                        UPDATE job_targets target
                        SET
                            status = 'dispatching',
                            started_at = COALESCE(target.started_at, now()),
                            process_incarnation_id = COALESCE(
                                target.process_incarnation_id,
                                due.client_process_incarnation_id
                            ),
                            dispatch_attempts = target.dispatch_attempts + 1,
                            dispatch_lease_until = now() + make_interval(secs => $2::integer),
                            deadline_at = COALESCE(
                                target.deadline_at,
                                COALESCE(target.started_at, now())
                                    + make_interval(secs => (due.timeout_secs + $5)::integer)
                            ),
                            last_dispatch_error = NULL
                        FROM due
                        WHERE target.job_id = due.job_id
                          AND target.client_id = due.client_id
                        RETURNING
                            due.job_id,
                            due.client_id,
                            due.actor_id,
                            due.command_type,
                            due.payload_hash,
                            COALESCE(
                                target.process_incarnation_id,
                                due.client_process_incarnation_id
                            ) AS process_incarnation_id,
                            due.operation,
                            due.source_schedule_id,
                            due.timeout_secs
                    ),
                    promoted_jobs AS (
                        UPDATE jobs job
                        SET status = 'running'
                        FROM (
                            SELECT DISTINCT job_id
                            FROM updated_targets
                        ) claimed
                        WHERE job.id = claimed.job_id
                          AND job.completed_at IS NULL
                          AND job.status = 'queued'
                        RETURNING job.id
                    )
                    SELECT
                        updated_targets.job_id,
                        updated_targets.client_id,
                        updated_targets.actor_id,
                        updated_targets.command_type,
                        updated_targets.payload_hash,
                        updated_targets.process_incarnation_id,
                        updated_targets.operation,
                        updated_targets.source_schedule_id,
                        updated_targets.timeout_secs,
                        (SELECT count(*) FROM promoted_jobs) AS promoted_jobs
                    FROM updated_targets
                    "#,
                )
                .bind(limit.clamp(1, 500))
                .bind(lease_secs.clamp(1, 7200) as i32)
                .bind(exclusive_operation_types())
                .bind(EXCLUSIVE_DISPATCH_ADVISORY_LOCK_CLASS)
                .bind(control_deadline_extra_secs.min(i32::MAX as u64) as i32)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let operation: sqlx::types::Json<JobCommand> = row.try_get("operation")?;
                        let timeout_secs = row.try_get::<i64, _>("timeout_secs")?.max(1) as u64;
                        Ok(ClaimedJobTarget {
                            job_id: row.try_get("job_id")?,
                            client_id: row.try_get("client_id")?,
                            actor_id: row.try_get("actor_id")?,
                            command_type: row.try_get("command_type")?,
                            payload_hash: row.try_get("payload_hash")?,
                            process_incarnation_id: row.try_get("process_incarnation_id")?,
                            operation: operation.0,
                            source_schedule_id: row.try_get("source_schedule_id")?,
                            timeout_secs,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn refresh_job_status_from_targets(
        &self,
        job_id: Uuid,
    ) -> Result<Option<String>> {
        let Some(job) = self.get_job(job_id).await? else {
            return Ok(None);
        };
        if job.completed_at.is_some() {
            if job.status == JOB_STATUS_COMPLETED {
                if let Some(operation) = self.job_operation(job_id).await? {
                    self.repair_tunnel_plan_execution(job_id, &operation, &job.status)
                        .await?;
                }
            }
            return Ok(None);
        }
        let targets = self.list_job_targets(job_id).await?;
        if targets.is_empty()
            || targets
                .iter()
                .any(|target| target_status_is_active(&target.status))
        {
            return Ok(Some(job.status));
        }
        let status = aggregate_job_status_from_targets(&targets);
        if self.finish_job(job_id, status).await? {
            Ok(Some(status.to_string()))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn skip_unstarted_queued_targets_for_client(
        &self,
        client_id: &str,
        reason_code: &str,
        message: &str,
    ) -> Result<Vec<Uuid>> {
        let job_ids = match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut changed = Vec::new();
                {
                    let mut targets = memory.job_targets.write().await;
                    for target in targets.iter_mut().filter(|target| {
                        target.client_id == client_id
                            && target.completed_at.is_none()
                            && target.status == TARGET_STATUS_QUEUED
                            && target.started_at.is_none()
                            && target.process_incarnation_id.is_none()
                    }) {
                        target.status = TARGET_STATUS_SKIPPED.to_string();
                        target.message = Some(message.to_string());
                        target.exit_code = Some(0);
                        target.started_at = Some(now.clone());
                        target.completed_at = Some(now.clone());
                        changed.push((target.job_id, target.client_id.clone()));
                    }
                }
                if !changed.is_empty() {
                    for (job_id, target_client_id) in &changed {
                        self.finalize_active_terminal_input_request_for_target_status(
                            *job_id,
                            target_client_id,
                            TARGET_STATUS_SKIPPED,
                        )
                        .await?;
                    }
                    let mut outputs = memory.job_outputs.write().await;
                    for (job_id, target_client_id) in &changed {
                        let seq = outputs
                            .iter()
                            .filter(|output| {
                                output.job_id == *job_id && output.client_id == *target_client_id
                            })
                            .map(|output| output.seq)
                            .max()
                            .map_or(0, |seq| seq + 1);
                        let value = target_skipped_status_output_value(
                            *job_id,
                            target_client_id,
                            reason_code,
                            message,
                        );
                        let data = serde_json::to_vec(&value)?;
                        outputs.push(JobOutputView {
                            job_id: *job_id,
                            client_id: target_client_id.clone(),
                            seq,
                            stream: "status".to_string(),
                            data_base64: base64::engine::general_purpose::STANDARD.encode(&data),
                            storage: "inline".to_string(),
                            artifact_object_key: None,
                            artifact_sha256_hex: None,
                            artifact_size_bytes: None,
                            exit_code: Some(0),
                            done: true,
                            received_at: Some(now.clone()),
                            created_at: now.clone(),
                        });
                    }
                }
                changed
                    .into_iter()
                    .map(|(job_id, _)| job_id)
                    .collect::<Vec<_>>()
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let job_ids = skip_unstarted_queued_targets_for_client_in_tx(
                    &mut tx,
                    client_id,
                    reason_code,
                    message,
                )
                .await?;
                tx.commit().await?;
                job_ids
            }
        };
        let mut unique_job_ids = job_ids;
        unique_job_ids.sort();
        unique_job_ids.dedup();
        for job_id in &unique_job_ids {
            self.refresh_job_status_from_targets(*job_id).await?;
        }
        Ok(unique_job_ids)
    }

    pub(crate) async fn mark_active_targets_agent_lost_for_client(
        &self,
        client_id: &str,
        expected_process_incarnation_id: Uuid,
        current_process_incarnation_id: Option<Uuid>,
        code: &str,
        message: &str,
    ) -> Result<Vec<Uuid>> {
        let job_ids = match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut changed = Vec::new();
                {
                    let mut targets = memory.job_targets.write().await;
                    for target in targets.iter_mut().filter(|target| {
                        target.client_id == client_id
                            && target.completed_at.is_none()
                            && matches!(
                                target.status.as_str(),
                                TARGET_STATUS_DISPATCHING | TARGET_STATUS_RUNNING
                            )
                            && target.process_incarnation_id
                                == Some(expected_process_incarnation_id)
                    }) {
                        target.status = TARGET_STATUS_AGENT_LOST.to_string();
                        target.message = Some(message.to_string());
                        target.completed_at = Some(now.clone());
                        changed.push((target.job_id, target.client_id.clone()));
                    }
                }
                if !changed.is_empty() {
                    for (job_id, target_client_id) in &changed {
                        self.finalize_active_terminal_input_request_for_target_status(
                            *job_id,
                            target_client_id,
                            TARGET_STATUS_AGENT_LOST,
                        )
                        .await?;
                    }
                    let mut outputs = memory.job_outputs.write().await;
                    for (job_id, target_client_id) in &changed {
                        let seq = outputs
                            .iter()
                            .filter(|output| {
                                output.job_id == *job_id && output.client_id == *target_client_id
                            })
                            .map(|output| output.seq)
                            .max()
                            .map_or(0, |seq| seq + 1);
                        let value = agent_lost_status_output_value(
                            *job_id,
                            target_client_id,
                            message,
                            Some(expected_process_incarnation_id),
                            current_process_incarnation_id,
                            code,
                        );
                        let data = serde_json::to_vec(&value)?;
                        outputs.push(JobOutputView {
                            job_id: *job_id,
                            client_id: target_client_id.clone(),
                            seq,
                            stream: "status".to_string(),
                            data_base64: base64::engine::general_purpose::STANDARD.encode(&data),
                            storage: "inline".to_string(),
                            artifact_object_key: None,
                            artifact_sha256_hex: None,
                            artifact_size_bytes: None,
                            exit_code: None,
                            done: true,
                            received_at: Some(now.clone()),
                            created_at: now.clone(),
                        });
                    }
                }
                changed
                    .into_iter()
                    .map(|(job_id, _)| job_id)
                    .collect::<Vec<_>>()
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let job_ids = mark_active_targets_agent_lost_for_client_in_tx(
                    &mut tx,
                    client_id,
                    expected_process_incarnation_id,
                    current_process_incarnation_id,
                    code,
                    message,
                )
                .await?;
                tx.commit().await?;
                job_ids
            }
        };
        let mut unique_job_ids = job_ids;
        unique_job_ids.sort();
        unique_job_ids.dedup();
        for job_id in &unique_job_ids {
            self.refresh_job_status_from_targets(*job_id).await?;
        }
        Ok(unique_job_ids)
    }

    pub(crate) async fn mark_job_target_running(
        &self,
        job_id: Uuid,
        client_id: &str,
        message: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                if let Some(job) = memory
                    .jobs
                    .write()
                    .await
                    .iter_mut()
                    .find(|job| job.id == job_id)
                {
                    job.status = "running".to_string();
                }
                if let Some(target) = memory
                    .job_targets
                    .write()
                    .await
                    .iter_mut()
                    .find(|target| target.job_id == job_id && target.client_id == client_id)
                {
                    target.status = TARGET_STATUS_RUNNING.to_string();
                    target.message = Some(message.to_string());
                    target
                        .started_at
                        .get_or_insert_with(|| unix_now().to_string());
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET status = 'running',
                        message = $3,
                        delivered_at = COALESCE(delivered_at, now()),
                        acked_at = COALESCE(acked_at, now()),
                        started_at = COALESCE(started_at, now()),
                        dispatch_lease_until = NULL,
                        last_dispatch_error = NULL
                    WHERE job_id = $1
                      AND client_id = $2
                      AND completed_at IS NULL
                      AND status IN ('queued', 'dispatching', 'running')
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(message)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    UPDATE jobs
                    SET status = 'running'
                    WHERE id = $1
                      AND completed_at IS NULL
                      AND status = 'queued'
                    "#,
                )
                .bind(job_id)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_job_target_delivery_error(
        &self,
        job_id: Uuid,
        client_id: &str,
        message: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(_) => {}
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET last_dispatch_error = $3
                    WHERE job_id = $1
                      AND client_id = $2
                      AND completed_at IS NULL
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(message)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_agent_lost_target(
        &self,
        job_id: Uuid,
        client_id: &str,
        message: &str,
        expected_process_incarnation_id: Option<Uuid>,
        observed_process_incarnation_id: Option<Uuid>,
    ) -> Result<Option<String>> {
        let outcome = TargetDispatchOutcome {
            status: TARGET_STATUS_AGENT_LOST.to_string(),
            exit_code: None,
            #[cfg(test)]
            command_version: None,
            accepted: false,
            message: message.to_string(),
            received_at: None,
            outputs: Vec::new(),
        };
        match self {
            Self::Memory(memory) => {
                let completed_at = unix_now().to_string();
                let output_data = serde_json::to_vec(&agent_lost_status_output_value(
                    job_id,
                    client_id,
                    message,
                    expected_process_incarnation_id,
                    observed_process_incarnation_id,
                    "agent_process_restarted",
                ))?;
                let mut targets = memory.job_targets.write().await;
                let Some(target) = targets.iter_mut().find(|target| {
                    target.job_id == job_id
                        && target.client_id == client_id
                        && target.completed_at.is_none()
                        && target_status_is_active(&target.status)
                }) else {
                    return Ok(None);
                };
                target.status = TARGET_STATUS_AGENT_LOST.to_string();
                target.message = Some(message.to_string());
                target.completed_at = Some(completed_at.clone());
                target
                    .started_at
                    .get_or_insert_with(|| completed_at.clone());
                drop(targets);
                self.finalize_active_terminal_input_request_for_target_status(
                    job_id,
                    client_id,
                    TARGET_STATUS_AGENT_LOST,
                )
                .await?;
                let seq = memory
                    .job_outputs
                    .read()
                    .await
                    .iter()
                    .filter(|output| output.job_id == job_id && output.client_id == client_id)
                    .map(|output| output.seq)
                    .max()
                    .unwrap_or(-1)
                    .saturating_add(1);
                memory.job_outputs.write().await.push(JobOutputView {
                    job_id,
                    client_id: client_id.to_string(),
                    seq,
                    stream: "status".to_string(),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(output_data),
                    storage: "inline".to_string(),
                    artifact_object_key: None,
                    artifact_sha256_hex: None,
                    artifact_size_bytes: None,
                    exit_code: None,
                    done: true,
                    received_at: Some(completed_at.clone()),
                    created_at: completed_at.clone(),
                });
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: None,
                    action: "job.target_result".to_string(),
                    target: format!("client:{client_id}"),
                    command_hash: None,
                    metadata: json!({
                        "job_id": job_id,
                        "status": TARGET_STATUS_AGENT_LOST,
                        "message": message,
                        "expected_process_incarnation_id": expected_process_incarnation_id,
                        "current_process_incarnation_id": observed_process_incarnation_id,
                    }),
                    created_at: completed_at,
                });
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let target_row = sqlx::query(
                    r#"
                    SELECT process_incarnation_id
                    FROM job_targets
                    WHERE job_id = $1
                      AND client_id = $2
                      AND completed_at IS NULL
                      AND status IN ('queued', 'dispatching', 'running')
                    FOR UPDATE
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(target_row) = target_row else {
                    tx.commit().await?;
                    return Ok(None);
                };
                let current_process_incarnation_id: Option<Uuid> =
                    target_row.try_get("process_incarnation_id")?;
                let evidence_process_incarnation_id =
                    observed_process_incarnation_id.or(current_process_incarnation_id);
                if let Some(expected) = expected_process_incarnation_id {
                    if current_process_incarnation_id != Some(expected) {
                        tx.commit().await?;
                        return Ok(None);
                    }
                }
                append_synthetic_agent_lost_output_in_tx(
                    &mut tx,
                    job_id,
                    client_id,
                    message,
                    expected_process_incarnation_id,
                    evidence_process_incarnation_id,
                )
                .await?;
                let updated = sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET status = 'agent_lost',
                        message = $3,
                        completed_at = now(),
                        result_received_at = now(),
                        dispatch_lease_until = NULL,
                        cancel_requested_at = COALESCE(cancel_requested_at, now()),
                        last_dispatch_error = $3
                    WHERE job_id = $1
                      AND client_id = $2
                      AND completed_at IS NULL
                      AND status IN ('queued', 'dispatching', 'running')
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(message)
                .execute(&mut *tx)
                .await?;
                if updated.rows_affected() == 0 {
                    tx.rollback().await?;
                    return Ok(None);
                }
                finalize_active_terminal_input_request_for_terminal_target_in_tx(
                    &mut tx, job_id, client_id,
                )
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
                .bind("job.target_result")
                .bind(format!("client:{client_id}"))
                .bind(json!({
                    "job_id": job_id,
                    "status": TARGET_STATUS_AGENT_LOST,
                    "message": message,
                    "expected_process_incarnation_id": expected_process_incarnation_id,
                    "target_process_incarnation_id": current_process_incarnation_id,
                    "current_process_incarnation_id": evidence_process_incarnation_id,
                }))
                .execute(&mut *tx)
                .await?;
                let _ = finish_job_in_tx_if_all_targets_terminal(&mut tx, job_id).await?;
                tx.commit().await?;
            }
        }
        let status = self.refresh_job_status_from_targets(job_id).await?;
        self.record_job_target_webhook_event(job_id, client_id, &outcome)
            .await?;
        Ok(status)
    }

    pub(crate) async fn expire_control_timeout_targets(
        &self,
        limit: i64,
        control_deadline_extra_secs: u64,
    ) -> Result<Vec<DeadlineExpiredJobTarget>> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now();
                let completed_at = now.to_string();
                let timeouts = memory.job_timeouts.read().await.clone();
                let operations = memory.job_operations.read().await.clone();
                let mut expired = Vec::new();
                let mut synthetic_outputs = Vec::new();
                let mut terminalized_inputs = Vec::new();
                let mut targets = memory.job_targets.write().await;
                for target in targets
                    .iter_mut()
                    .filter(|target| {
                        target.completed_at.is_none()
                            && matches!(
                                target.status.as_str(),
                                TARGET_STATUS_DISPATCHING | TARGET_STATUS_RUNNING
                            )
                    })
                    .take(limit.clamp(1, 500) as usize)
                {
                    let Some(started_at) = target
                        .started_at
                        .as_deref()
                        .and_then(|value| value.parse::<u64>().ok())
                    else {
                        continue;
                    };
                    let timeout_secs = timeouts
                        .get(&target.job_id)
                        .copied()
                        .unwrap_or(DEFAULT_MAX_COMMAND_TIMEOUT_SECS)
                        .max(1)
                        .saturating_add(control_deadline_extra_secs);
                    if now.saturating_sub(started_at) < timeout_secs {
                        continue;
                    }
                    let (status, message, output_value, exit_code) = if matches!(
                        operations.get(&target.job_id),
                        Some(JobCommand::AgentUpdateActivate {
                            restart_agent: true,
                            ..
                        })
                    ) {
                        let message = "agent update activation restart did not reconnect with matching heartbeat before deadline".to_string();
                        (
                            TARGET_STATUS_AGENT_LOST,
                            message.clone(),
                            agent_lost_status_output_value(
                                target.job_id,
                                &target.client_id,
                                &message,
                                target.process_incarnation_id,
                                None,
                                "agent_update_restart_missing_heartbeat",
                            ),
                            None,
                        )
                    } else {
                        let message =
                            "control deadline elapsed before final command output".to_string();
                        (
                            TARGET_STATUS_CONTROL_TIMEOUT,
                            message.clone(),
                            json!({
                                "type": "control_timeout",
                                "status": TARGET_STATUS_CONTROL_TIMEOUT,
                                "code": "control_deadline_elapsed",
                                "message": message,
                                "job_id": target.job_id,
                                "client_id": &target.client_id,
                                "process_incarnation_id": target.process_incarnation_id,
                            }),
                            None,
                        )
                    };
                    target.status = status.to_string();
                    target.message = Some(message.clone());
                    target.completed_at = Some(completed_at.clone());
                    synthetic_outputs.push((
                        target.job_id,
                        target.client_id.clone(),
                        output_value,
                        exit_code,
                    ));
                    terminalized_inputs.push((
                        target.job_id,
                        target.client_id.clone(),
                        status.to_string(),
                    ));
                    expired.push(DeadlineExpiredJobTarget {
                        job_id: target.job_id,
                        client_id: target.client_id.clone(),
                        status: status.to_string(),
                    });
                }
                drop(targets);
                for (job_id, client_id, status) in terminalized_inputs {
                    self.finalize_active_terminal_input_request_for_target_status(
                        job_id, &client_id, &status,
                    )
                    .await?;
                }
                if !synthetic_outputs.is_empty() {
                    let mut outputs = memory.job_outputs.write().await;
                    for (job_id, client_id, output_value, exit_code) in synthetic_outputs {
                        let data = serde_json::to_vec(&output_value)?;
                        let seq = outputs
                            .iter()
                            .filter(|output| {
                                output.job_id == job_id && output.client_id == client_id
                            })
                            .map(|output| output.seq)
                            .max()
                            .unwrap_or(-1)
                            .saturating_add(1);
                        outputs.push(JobOutputView {
                            job_id,
                            client_id,
                            seq,
                            stream: "status".to_string(),
                            data_base64: base64::engine::general_purpose::STANDARD.encode(data),
                            storage: "inline".to_string(),
                            artifact_object_key: None,
                            artifact_sha256_hex: None,
                            artifact_size_bytes: None,
                            exit_code,
                            done: true,
                            received_at: Some(completed_at.clone()),
                            created_at: completed_at.clone(),
                        });
                    }
                }
                Ok(expired)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let rows = sqlx::query(
                    r#"
                    SELECT
                        target.job_id,
                        target.client_id,
                        target.process_incarnation_id,
                        target.last_dispatch_error,
                        job.operation
                    FROM job_targets target
                    JOIN jobs job ON job.id = target.job_id
                    WHERE target.completed_at IS NULL
                      AND target.status IN ('dispatching', 'running')
                      AND target.deadline_at IS NOT NULL
                      AND target.deadline_at <= now()
                      AND target.started_at IS NOT NULL
                      AND target.started_at + make_interval(secs => (job.timeout_secs + $2)::integer) <= now()
                    ORDER BY target.deadline_at ASC, target.job_id, target.client_id
                    LIMIT $1
                    FOR UPDATE SKIP LOCKED
                    "#,
                )
                .bind(limit.clamp(1, 500))
                .bind(control_deadline_extra_secs.min(i32::MAX as u64) as i32)
                .fetch_all(&mut *tx)
                .await?;
                let mut expired = Vec::new();
                for row in rows {
                    let job_id: Uuid = row.try_get("job_id")?;
                    let client_id: String = row.try_get("client_id")?;
                    let process_incarnation_id: Option<Uuid> =
                        row.try_get("process_incarnation_id")?;
                    let last_dispatch_error: Option<String> = row.try_get("last_dispatch_error")?;
                    let operation: sqlx::types::Json<JobCommand> = row.try_get("operation")?;
                    let missing_update_heartbeat = matches!(
                        operation.0,
                        JobCommand::AgentUpdateActivate {
                            restart_agent: true,
                            ..
                        }
                    );
                    let (status, message) = if missing_update_heartbeat {
                        (
                            TARGET_STATUS_AGENT_LOST,
                            "agent update activation restart did not reconnect with matching heartbeat before deadline".to_string(),
                        )
                    } else {
                        (
                            TARGET_STATUS_CONTROL_TIMEOUT,
                            last_dispatch_error.unwrap_or_else(|| {
                                "control deadline elapsed before final command output".to_string()
                            }),
                        )
                    };
                    if missing_update_heartbeat {
                        append_synthetic_agent_lost_output_with_code_in_tx(
                            &mut tx,
                            job_id,
                            &client_id,
                            &message,
                            process_incarnation_id,
                            None,
                            "agent_update_restart_missing_heartbeat",
                        )
                        .await?;
                    } else {
                        append_synthetic_status_output_in_tx(
                            &mut tx,
                            job_id,
                            &client_id,
                            json!({
                                "type": "control_timeout",
                                "status": TARGET_STATUS_CONTROL_TIMEOUT,
                                "code": "control_deadline_elapsed",
                                "message": message,
                                "job_id": job_id,
                                "client_id": &client_id,
                                "process_incarnation_id": process_incarnation_id,
                            }),
                            None,
                        )
                        .await?;
                    }
                    let updated = sqlx::query(
                        r#"
                        UPDATE job_targets target
                        SET status = $3,
                            message = $4,
                            completed_at = now(),
                            result_received_at = now(),
                            dispatch_lease_until = NULL,
                            cancel_requested_at = COALESCE(cancel_requested_at, now()),
                            last_dispatch_error = CASE WHEN $3 = 'control_timeout' OR $3 = 'agent_lost' THEN $4 ELSE NULL END
                        FROM jobs job
                        WHERE target.job_id = $1
                          AND target.client_id = $2
                          AND job.id = target.job_id
                          AND target.completed_at IS NULL
                          AND target.status IN ('dispatching', 'running')
                          AND target.deadline_at IS NOT NULL
                          AND target.deadline_at <= now()
                          AND target.started_at IS NOT NULL
                          AND target.started_at + make_interval(secs => (job.timeout_secs + $5)::integer) <= now()
                          AND (
                            ($6::uuid IS NULL AND target.process_incarnation_id IS NULL)
                            OR target.process_incarnation_id = $6::uuid
                          )
                        "#,
                    )
                    .bind(job_id)
                    .bind(&client_id)
                    .bind(status)
                    .bind(&message)
                    .bind(control_deadline_extra_secs.min(i32::MAX as u64) as i32)
                    .bind(process_incarnation_id)
                    .execute(&mut *tx)
                    .await?;
                    if updated.rows_affected() == 0 {
                        anyhow::bail!("deadline_terminal_cas_lost:{job_id}:{client_id}");
                    }
                    finalize_active_terminal_input_request_for_terminal_target_in_tx(
                        &mut tx, job_id, &client_id,
                    )
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
                    .bind("job.target_result")
                    .bind(format!("client:{client_id}"))
                    .bind(json!({
                        "job_id": job_id,
                        "status": status,
                        "message": message,
                        "reason": if missing_update_heartbeat {
                            "agent_update_restart_missing_heartbeat"
                        } else {
                            "control_deadline_elapsed"
                        },
                        "process_incarnation_id": process_incarnation_id,
                    }))
                    .execute(&mut *tx)
                    .await?;
                    let _ = finish_job_in_tx_if_all_targets_terminal(&mut tx, job_id).await?;
                    expired.push(DeadlineExpiredJobTarget {
                        job_id,
                        client_id,
                        status: status.to_string(),
                    });
                }
                tx.commit().await?;
                Ok(expired)
            }
        }
    }

    pub(crate) async fn request_job_cancel(
        &self,
        job_id: Uuid,
        actor_id: Uuid,
        reason: Option<&str>,
    ) -> Result<JobCancelPlan> {
        let message = reason
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("operator_cancel_requested");
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut cancel_targets = Vec::new();
                let mut canceled_targets = Vec::new();
                {
                    let targets = memory.job_targets.read().await;
                    for target in targets
                        .iter()
                        .filter(|target| target.job_id == job_id && target.completed_at.is_none())
                    {
                        match target.status.as_str() {
                            TARGET_STATUS_QUEUED => {
                                canceled_targets.push(target.client_id.clone());
                            }
                            TARGET_STATUS_DISPATCHING | TARGET_STATUS_RUNNING => {
                                cancel_targets.push(target.client_id.clone());
                            }
                            _ => {}
                        }
                    }
                }
                if !canceled_targets.is_empty() {
                    let mut outputs = memory.job_outputs.write().await;
                    for client_id in &canceled_targets {
                        let value =
                            command_canceled_status_output_value(job_id, client_id, message);
                        let data = serde_json::to_vec(&value)?;
                        let seq = outputs
                            .iter()
                            .filter(|output| {
                                output.job_id == job_id && output.client_id == *client_id
                            })
                            .map(|output| output.seq)
                            .max()
                            .unwrap_or(-1)
                            .saturating_add(1);
                        outputs.push(JobOutputView {
                            job_id,
                            client_id: client_id.clone(),
                            seq,
                            stream: "status".to_string(),
                            data_base64: base64::engine::general_purpose::STANDARD.encode(&data),
                            storage: "inline".to_string(),
                            artifact_object_key: None,
                            artifact_sha256_hex: None,
                            artifact_size_bytes: None,
                            exit_code: None,
                            done: true,
                            received_at: Some(now.clone()),
                            created_at: now.clone(),
                        });
                    }
                }
                let canceled_target_set = canceled_targets.iter().cloned().collect::<HashSet<_>>();
                if !canceled_target_set.is_empty() {
                    let mut targets = memory.job_targets.write().await;
                    for target in targets.iter_mut().filter(|target| {
                        target.job_id == job_id
                            && target.completed_at.is_none()
                            && target.status == TARGET_STATUS_QUEUED
                            && canceled_target_set.contains(&target.client_id)
                    }) {
                        target.status = TARGET_STATUS_CANCELED.to_string();
                        target.message = Some(message.to_string());
                        target.completed_at = Some(now.clone());
                    }
                }
                for client_id in &canceled_targets {
                    self.finalize_active_terminal_input_request_for_target_status(
                        job_id,
                        client_id,
                        TARGET_STATUS_CANCELED,
                    )
                    .await?;
                }
                let pending_canceled = canceled_targets.len();
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor_id),
                    action: "job.cancel_requested".to_string(),
                    target: format!("job:{job_id}"),
                    command_hash: None,
                    metadata: json!({
                        "job_id": job_id,
                        "reason": message,
                        "pending_canceled": pending_canceled,
                        "cancel_targets": cancel_targets,
                    }),
                    created_at: now,
                });
                Ok(JobCancelPlan {
                    cancel_targets,
                    pending_canceled,
                })
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let pending_rows = sqlx::query(
                    r#"
                    SELECT client_id
                    FROM job_targets
                    WHERE job_id = $1
                      AND completed_at IS NULL
                      AND status = 'queued'
                    ORDER BY client_id
                    FOR UPDATE
                    "#,
                )
                .bind(job_id)
                .fetch_all(&mut *tx)
                .await?;
                for row in &pending_rows {
                    let client_id: String = row.try_get("client_id")?;
                    append_synthetic_status_output_in_tx(
                        &mut tx,
                        job_id,
                        &client_id,
                        command_canceled_status_output_value(job_id, &client_id, message),
                        None,
                    )
                    .await?;
                }
                if !pending_rows.is_empty() {
                    let updated = sqlx::query(
                        r#"
                        UPDATE job_targets
                        SET
                            status = 'canceled',
                            message = $2,
                            completed_at = now(),
                            dispatch_lease_until = NULL,
                            cancel_requested_at = COALESCE(cancel_requested_at, now())
                        WHERE job_id = $1
                          AND completed_at IS NULL
                          AND status = 'queued'
                        "#,
                    )
                    .bind(job_id)
                    .bind(message)
                    .execute(&mut *tx)
                    .await?;
                    if updated.rows_affected() != pending_rows.len() as u64 {
                        anyhow::bail!("queued_cancel_target_cas_lost:{job_id}");
                    }
                }
                let active_rows = sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET
                        cancel_requested_at = COALESCE(cancel_requested_at, now()),
                        message = COALESCE(message, $2)
                    WHERE job_id = $1
                      AND completed_at IS NULL
                      AND status IN ('dispatching', 'running')
                    RETURNING client_id
                    "#,
                )
                .bind(job_id)
                .bind(message)
                .fetch_all(&mut *tx)
                .await?;
                let pending_canceled = pending_rows.len();
                for row in &pending_rows {
                    let client_id: String = row.try_get("client_id")?;
                    finalize_active_terminal_input_request_for_terminal_target_in_tx(
                        &mut tx, job_id, &client_id,
                    )
                    .await?;
                }
                let cancel_targets = active_rows
                    .into_iter()
                    .map(|row| row.try_get("client_id").map_err(Into::into))
                    .collect::<Result<Vec<String>>>()?;
                if pending_canceled > 0 {
                    let _ = finish_job_in_tx_if_all_targets_terminal(&mut tx, job_id).await?;
                }
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, 'job.cancel_requested', $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(actor_id)
                .bind(format!("job:{job_id}"))
                .bind(json!({
                    "job_id": job_id,
                    "reason": message,
                    "pending_canceled": pending_canceled,
                    "cancel_targets": &cancel_targets,
                }))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(JobCancelPlan {
                    cancel_targets,
                    pending_canceled,
                })
            }
        }
    }

    pub(crate) async fn record_job_target_cancel_result(
        &self,
        job_id: Uuid,
        client_id: &str,
        _accepted: bool,
        acked: bool,
        applied: bool,
        message: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                if applied {
                    let now = unix_now().to_string();
                    let mut terminalized = false;
                    if let Some(target) =
                        memory.job_targets.write().await.iter_mut().find(|target| {
                            target.job_id == job_id
                                && target.client_id == client_id
                                && target.completed_at.is_none()
                        })
                    {
                        target.status = TARGET_STATUS_CANCELED.to_string();
                        target.message = Some(message.to_string());
                        target.completed_at = Some(now);
                        terminalized = true;
                    }
                    if terminalized {
                        self.finalize_active_terminal_input_request_for_target_status(
                            job_id,
                            client_id,
                            TARGET_STATUS_CANCELED,
                        )
                        .await?;
                    }
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let updated = sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET
                        cancel_sent_at = COALESCE(cancel_sent_at, now()),
                        cancel_acked_at = CASE WHEN $3 THEN COALESCE(cancel_acked_at, now()) ELSE cancel_acked_at END,
                        status = CASE WHEN $4 AND completed_at IS NULL THEN 'canceled' ELSE status END,
                        completed_at = CASE WHEN $4 AND completed_at IS NULL THEN now() ELSE completed_at END,
                        dispatch_lease_until = CASE WHEN $4 AND completed_at IS NULL THEN NULL ELSE dispatch_lease_until END,
                        message = CASE WHEN $4 AND completed_at IS NULL THEN $5 ELSE COALESCE(message, $5) END,
                        last_dispatch_error = CASE WHEN $4 THEN NULL ELSE $5 END
                    WHERE job_id = $1
                      AND client_id = $2
                      AND (
                        completed_at IS NULL
                        OR status IN ('control_timeout', 'canceled')
                      )
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(acked)
                .bind(applied)
                .bind(message)
                .execute(&mut *tx)
                .await?;
                if applied && updated.rows_affected() > 0 {
                    finalize_active_terminal_input_request_for_terminal_target_in_tx(
                        &mut tx, job_id, client_id,
                    )
                    .await?;
                    let _ = finish_job_in_tx_if_all_targets_terminal(&mut tx, job_id).await?;
                }
                tx.commit().await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_job_target_cancel_sent(
        &self,
        job_id: Uuid,
        client_id: &str,
    ) -> Result<()> {
        match self {
            Self::Memory(_) => {}
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET cancel_sent_at = COALESCE(cancel_sent_at, now())
                    WHERE job_id = $1
                      AND client_id = $2
                      AND (
                        completed_at IS NULL
                        OR status IN ('control_timeout', 'canceled')
                      )
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn update_job_target_result(
        &self,
        job_id: Uuid,
        client_id: &str,
        outcome: &TargetDispatchOutcome,
    ) -> Result<bool> {
        match self {
            Self::Memory(memory) => {
                let completed_at = unix_now().to_string();
                let mut updated = false;
                {
                    let mut targets = memory.job_targets.write().await;
                    if let Some(target) = targets.iter_mut().find(|target| {
                        target.job_id == job_id
                            && target.client_id == client_id
                            && target.completed_at.is_none()
                    }) {
                        target.status = outcome.status.clone();
                        target.message = Some(outcome.message.clone());
                        target.exit_code = outcome.exit_code;
                        target
                            .started_at
                            .get_or_insert_with(|| completed_at.clone());
                        target.completed_at = Some(completed_at.clone());
                        updated = true;
                    }
                    if !updated {
                        return Ok(false);
                    }
                }
                if updated {
                    self.finalize_active_terminal_input_request_for_target_status(
                        job_id,
                        client_id,
                        &outcome.status,
                    )
                    .await?;
                    memory.audits.write().await.push(AuditLogView {
                        id: Uuid::new_v4(),
                        actor_id: None,
                        action: "job.target_result".to_string(),
                        target: format!("client:{client_id}"),
                        command_hash: None,
                        metadata: json!({
                            "job_id": job_id,
                            "status": outcome.status,
                            "exit_code": outcome.exit_code,
                            "accepted": outcome.accepted,
                            "message": outcome.message,
                            "received_at": outcome.received_at,
                        }),
                        created_at: completed_at,
                    });
                    let update_lifecycle_operation = if outcome.status == TARGET_STATUS_COMPLETED
                        || agent_update_activation_failure_status(&outcome.status)
                    {
                        match memory.job_operations.read().await.get(&job_id).cloned() {
                            Some(
                                operation @ (JobCommand::AgentUpdateActivate { .. }
                                | JobCommand::AgentUpdateRollback { .. }),
                            ) => Some(operation),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    match update_lifecycle_operation {
                        Some(JobCommand::AgentUpdateActivate {
                            staged_sha256_hex, ..
                        }) if outcome.status == TARGET_STATUS_COMPLETED => {
                            self.record_agent_update_activation_completed(
                                client_id,
                                job_id,
                                &staged_sha256_hex,
                            )
                            .await?;
                        }
                        Some(JobCommand::AgentUpdateActivate {
                            staged_sha256_hex, ..
                        }) if agent_update_activation_failure_status(&outcome.status) => {
                            self.record_agent_update_activation_failed(
                                client_id,
                                job_id,
                                &staged_sha256_hex,
                                &outcome.status,
                                outcome.exit_code,
                                &outcome.message,
                            )
                            .await?;
                        }
                        Some(JobCommand::AgentUpdateRollback {
                            rollback_sha256_hex,
                        }) if outcome.status == TARGET_STATUS_COMPLETED => {
                            self.record_agent_update_rollback_completed(
                                client_id,
                                job_id,
                                rollback_sha256_hex.as_deref(),
                            )
                            .await?;
                        }
                        Some(JobCommand::AgentUpdateRollback {
                            rollback_sha256_hex,
                        }) if agent_update_activation_failure_status(&outcome.status) => {
                            self.record_agent_update_rollback_failed(
                                client_id,
                                job_id,
                                rollback_sha256_hex.as_deref(),
                                &outcome.status,
                                outcome.exit_code,
                                &outcome.message,
                            )
                            .await?;
                        }
                        _ => {}
                    }
                }
                self.record_job_target_webhook_event(job_id, client_id, outcome)
                    .await?;
                Ok(true)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let updated = sqlx::query(
                    r#"
                    UPDATE job_targets
                    SET status = $3,
                        message = $4,
                        exit_code = $5,
                        started_at = COALESCE(started_at, now()),
                        completed_at = now(),
                        result_received_at = COALESCE($6::timestamptz, now()),
                        dispatch_lease_until = NULL,
                        last_dispatch_error = CASE WHEN $3 IN ('failed', 'control_timeout', 'agent_lost') THEN $4 ELSE NULL END
                    WHERE job_id = $1
                      AND client_id = $2
                      AND completed_at IS NULL
                      AND status IN ('queued', 'dispatching', 'running')
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(&outcome.status)
                .bind(&outcome.message)
                .bind(outcome.exit_code)
                .bind(outcome.received_at.as_deref())
                .execute(&mut *tx)
                .await?;
                if updated.rows_affected() == 0 {
                    return Ok(false);
                } else {
                    finalize_active_terminal_input_request_for_terminal_target_in_tx(
                        &mut tx, job_id, client_id,
                    )
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
                    .bind("job.target_result")
                    .bind(format!("client:{client_id}"))
                    .bind(json!({
                        "job_id": job_id,
                        "status": outcome.status,
                        "exit_code": outcome.exit_code,
                        "accepted": outcome.accepted,
                        "message": outcome.message,
                        "received_at": outcome.received_at,
                    }))
                    .execute(&mut *tx)
                    .await?;
                }
                let finished_status =
                    finish_job_in_tx_if_all_targets_terminal(&mut tx, job_id).await?;
                tx.commit().await?;
                let update_lifecycle_operation = if outcome.status == TARGET_STATUS_COMPLETED
                    || agent_update_activation_failure_status(&outcome.status)
                {
                    match self.job_operation(job_id).await? {
                        Some(
                            operation @ (JobCommand::AgentUpdateActivate { .. }
                            | JobCommand::AgentUpdateRollback { .. }),
                        ) => Some(operation),
                        _ => None,
                    }
                } else {
                    None
                };
                match update_lifecycle_operation {
                    Some(JobCommand::AgentUpdateActivate {
                        staged_sha256_hex, ..
                    }) if outcome.status == TARGET_STATUS_COMPLETED => {
                        self.record_agent_update_activation_completed(
                            client_id,
                            job_id,
                            &staged_sha256_hex,
                        )
                        .await?;
                    }
                    Some(JobCommand::AgentUpdateActivate {
                        staged_sha256_hex, ..
                    }) if agent_update_activation_failure_status(&outcome.status) => {
                        self.record_agent_update_activation_failed(
                            client_id,
                            job_id,
                            &staged_sha256_hex,
                            &outcome.status,
                            outcome.exit_code,
                            &outcome.message,
                        )
                        .await?;
                    }
                    Some(JobCommand::AgentUpdateRollback {
                        rollback_sha256_hex,
                    }) if outcome.status == TARGET_STATUS_COMPLETED => {
                        self.record_agent_update_rollback_completed(
                            client_id,
                            job_id,
                            rollback_sha256_hex.as_deref(),
                        )
                        .await?;
                    }
                    Some(JobCommand::AgentUpdateRollback {
                        rollback_sha256_hex,
                    }) if agent_update_activation_failure_status(&outcome.status) => {
                        self.record_agent_update_rollback_failed(
                            client_id,
                            job_id,
                            rollback_sha256_hex.as_deref(),
                            &outcome.status,
                            outcome.exit_code,
                            &outcome.message,
                        )
                        .await?;
                    }
                    _ => {}
                }
                if let Some(status) = finished_status {
                    self.record_job_terminal_side_effects(job_id, &status, None)
                        .await?;
                }
                self.record_job_target_webhook_event(job_id, client_id, outcome)
                    .await?;
                Ok(true)
            }
        }
    }

    pub(crate) async fn finish_job(&self, job_id: Uuid, status: &str) -> Result<bool> {
        let completed_operation = match self {
            Self::Memory(memory) => {
                let completed_at = unix_now().to_string();
                let mut jobs = memory.jobs.write().await;
                let Some(job) = jobs
                    .iter_mut()
                    .find(|job| job.id == job_id && job.completed_at.is_none())
                else {
                    return Ok(false);
                };
                job.status = status.to_string();
                job.completed_at = Some(completed_at);
                drop(jobs);
                if status == JOB_STATUS_COMPLETED {
                    memory.job_operations.read().await.get(&job_id).cloned()
                } else {
                    None
                }
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    UPDATE jobs
                    SET status = $2, completed_at = now()
                    WHERE id = $1
                      AND completed_at IS NULL
                    RETURNING operation
                    "#,
                )
                .bind(job_id)
                .bind(status)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(false);
                };
                if status == JOB_STATUS_COMPLETED {
                    let operation: sqlx::types::Json<JobCommand> = row.try_get("operation")?;
                    Some(operation.0)
                } else {
                    None
                }
            }
        };
        self.record_job_terminal_side_effects(job_id, status, completed_operation)
            .await?;
        Ok(true)
    }

    pub(crate) async fn record_job_terminal_side_effects(
        &self,
        job_id: Uuid,
        status: &str,
        completed_operation: Option<JobCommand>,
    ) -> Result<()> {
        if status == JOB_STATUS_COMPLETED {
            match completed_operation {
                Some(operation) => {
                    self.record_tunnel_plan_execution(job_id, &operation, status)
                        .await?;
                }
                None => {
                    if let Some(operation) = self.job_operation(job_id).await? {
                        self.repair_tunnel_plan_execution(job_id, &operation, status)
                            .await?;
                    }
                }
            }
        }
        self.record_job_status_webhook_event(job_id, status).await?;
        self.record_schedule_job_outcome(job_id, status).await?;
        Ok(())
    }

    async fn job_operation(&self, job_id: Uuid) -> Result<Option<JobCommand>> {
        match self {
            Self::Memory(memory) => Ok(memory.job_operations.read().await.get(&job_id).cloned()),
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT operation
                    FROM jobs
                    WHERE id = $1
                    "#,
                )
                .bind(job_id)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(None);
                };
                let operation: sqlx::types::Json<JobCommand> = row.try_get("operation")?;
                Ok(Some(operation.0))
            }
        }
    }

    async fn record_schedule_job_outcome(&self, job_id: Uuid, status: &str) -> Result<()> {
        let Some(summary) = self.webhook_job_summary(job_id).await? else {
            return Ok(());
        };
        let Some(schedule_id) = summary.source_schedule_id else {
            return Ok(());
        };
        let outcome_error = schedule_job_outcome_error(status, &summary.target_statuses);
        let outcome_neutral = outcome_error.is_none()
            && matches!(
                status,
                JOB_STATUS_PARTIAL_SUCCESS | JOB_STATUS_SKIPPED | JOB_STATUS_CANCELED
            );
        let event_id = format!("schedule:{}:job:{}:finished", schedule_id, job_id);
        let schedule_outcome = match self {
            Self::Memory(memory) => {
                let already_recorded = memory.webhook_events.read().await.iter().any(|event| {
                    event.kind == "schedule.job_finished" && event.event_id == event_id
                });
                let mut schedules = memory.schedules.write().await;
                let schedule = schedules
                    .iter_mut()
                    .find(|schedule| schedule.id == schedule_id);
                let Some(schedule) = schedule else {
                    return Ok(());
                };
                if !already_recorded {
                    if let Some(error) = outcome_error.as_deref() {
                        schedule.failure_count += 1;
                        schedule.last_error = Some(error.to_string());
                        if schedule.failure_count >= schedule.max_failures {
                            schedule.enabled = false;
                        } else {
                            schedule.next_run_at = (Utc::now()
                                + Duration::seconds(schedule.retry_delay_secs.max(0)))
                            .to_rfc3339();
                        }
                    } else if status == JOB_STATUS_COMPLETED {
                        schedule.failure_count = 0;
                        schedule.last_error = None;
                    }
                    schedule.updated_at = unix_now().to_string();
                }
                Some(ScheduleJobOutcome {
                    schedule_id,
                    schedule_name: schedule.name.clone(),
                    job_id,
                    status: status.to_string(),
                    error: outcome_error.clone(),
                    enabled: schedule.enabled,
                    failure_count: schedule.failure_count,
                    max_failures: schedule.max_failures,
                    retry_delay_secs: schedule.retry_delay_secs,
                    next_run_at: schedule.next_run_at.clone(),
                })
            }
            Self::Postgres(pool) => {
                let row = if outcome_neutral {
                    sqlx::query(
                        r#"
                        UPDATE schedules
                        SET
                            last_job_id = $2,
                            last_job_status = $3,
                            last_job_completed_at = now(),
                            last_job_error = NULL,
                            updated_at = now()
                        WHERE id = $1
                          AND (
                              last_job_id IS NULL
                              OR last_job_id = $2
                              OR last_job_completed_at IS NULL
                              OR last_job_completed_at <= (
                                  SELECT completed_at FROM jobs WHERE id = $2
                              )
                          )
                        RETURNING
                            name,
                            enabled,
                            failure_count,
                            max_failures,
                            retry_delay_secs,
                            next_run_at::text AS next_run_at
                        "#,
                    )
                    .bind(schedule_id)
                    .bind(job_id)
                    .bind(status)
                    .fetch_optional(pool)
                    .await?
                } else if let Some(error) = outcome_error.as_deref() {
                    sqlx::query(
                        r#"
                        UPDATE schedules
                        SET
                            last_job_id = $2,
                            last_job_status = $3,
                            last_job_completed_at = now(),
                            last_job_error = $4,
                            failure_count = CASE
                                WHEN last_job_id = $2 AND last_job_status = $3 THEN failure_count
                                ELSE failure_count + 1
                            END,
                            last_error = $4,
                            enabled = CASE
                                WHEN last_job_id = $2 AND last_job_status = $3 THEN enabled
                                WHEN failure_count + 1 >= max_failures THEN FALSE
                                ELSE enabled
                            END,
                            next_run_at = CASE
                                WHEN last_job_id = $2 AND last_job_status = $3 THEN next_run_at
                                WHEN failure_count + 1 >= max_failures THEN next_run_at
                                ELSE now() + (retry_delay_secs * interval '1 second')
                            END,
                            updated_at = now()
                        WHERE id = $1
                          AND (
                              last_job_id IS NULL
                              OR last_job_id = $2
                              OR last_job_completed_at IS NULL
                              OR last_job_completed_at <= (
                                  SELECT completed_at FROM jobs WHERE id = $2
                              )
                          )
                        RETURNING
                            name,
                            enabled,
                            failure_count,
                            max_failures,
                            retry_delay_secs,
                            next_run_at::text AS next_run_at
                        "#,
                    )
                    .bind(schedule_id)
                    .bind(job_id)
                    .bind(status)
                    .bind(error)
                    .fetch_optional(pool)
                    .await?
                } else {
                    sqlx::query(
                        r#"
                        UPDATE schedules
                        SET
                            last_job_id = $2,
                            last_job_status = $3,
                            last_job_completed_at = now(),
                            last_job_error = NULL,
                            failure_count = 0,
                            last_error = NULL,
                            updated_at = now()
                        WHERE id = $1
                          AND (
                              last_job_id IS NULL
                              OR last_job_id = $2
                              OR last_job_completed_at IS NULL
                              OR last_job_completed_at <= (
                                  SELECT completed_at FROM jobs WHERE id = $2
                              )
                          )
                        RETURNING
                            name,
                            enabled,
                            failure_count,
                            max_failures,
                            retry_delay_secs,
                            next_run_at::text AS next_run_at
                        "#,
                    )
                    .bind(schedule_id)
                    .bind(job_id)
                    .bind(status)
                    .fetch_optional(pool)
                    .await?
                };
                row.map(|row| {
                    let schedule_name: String = row.try_get("name")?;
                    Ok::<_, sqlx::Error>(ScheduleJobOutcome {
                        schedule_id,
                        schedule_name,
                        job_id,
                        status: status.to_string(),
                        error: outcome_error.clone(),
                        enabled: row.try_get("enabled")?,
                        failure_count: row.try_get("failure_count")?,
                        max_failures: row.try_get("max_failures")?,
                        retry_delay_secs: row.try_get("retry_delay_secs")?,
                        next_run_at: row.try_get("next_run_at")?,
                    })
                })
                .transpose()?
            }
        };
        let Some(schedule_outcome) = schedule_outcome else {
            return Ok(());
        };
        let mut predicates = vec![
            "schedule.job_finished".to_string(),
            format!("schedule.id:{}", schedule_outcome.schedule_id),
            format!("schedule.name:{}", schedule_outcome.schedule_name),
            format!("job.status:{}", schedule_outcome.status),
            format!("job.status.become_{}", schedule_outcome.status),
            format!("job.type:{}", summary.command_type),
        ];
        predicates.sort();
        predicates.dedup();
        self.record_webhook_event(WebhookEventCandidate {
            kind: "schedule.job_finished".to_string(),
            event_id: event_id.clone(),
            event_predicates: predicates.clone(),
            subject_client_ids: summary.targets.clone(),
            actor_id: summary.actor_id,
            payload: json!({
                "event": {
                    "kind": "schedule.job_finished",
                    "id": &event_id,
                    "predicates": &predicates,
                },
                "schedule": {
                    "id": schedule_outcome.schedule_id,
                    "name": &schedule_outcome.schedule_name,
                    "last_job_id": schedule_outcome.job_id,
                    "last_job_status": &schedule_outcome.status,
                    "last_job_error": &schedule_outcome.error,
                },
                "job": {
                    "id": job_id,
                    "status": status,
                    "type": &summary.command_type,
                    "privileged": summary.privileged,
                    "payload_hash": &summary.payload_hash,
                    "source_schedule_id": schedule_id,
                    "target_count": summary.target_count,
                    "target_ids": &summary.targets,
                },
            }),
        })
        .await?;
        self.record_schedule_job_failure_visibility(&summary, &schedule_outcome)
            .await?;
        Ok(())
    }

    async fn record_schedule_job_failure_visibility(
        &self,
        summary: &WebhookJobSummary,
        schedule_outcome: &ScheduleJobOutcome,
    ) -> Result<()> {
        let Some(error) = schedule_outcome.error.as_ref() else {
            return Ok(());
        };
        match self {
            Self::Memory(memory) => {
                let job_id_string = schedule_outcome.job_id.to_string();
                let mut audits = memory.audits.write().await;
                let audit_exists = audits.iter().any(|audit| {
                    audit.action == "schedule.job_failed"
                        && audit.metadata["job_id"].as_str() == Some(job_id_string.as_str())
                });
                if !audit_exists {
                    audits.push(AuditLogView {
                        id: Uuid::new_v4(),
                        actor_id: summary.actor_id,
                        action: "schedule.job_failed".to_string(),
                        target: format!("schedule:{}", schedule_outcome.schedule_id),
                        command_hash: None,
                        metadata: json!({
                            "schedule_id": schedule_outcome.schedule_id,
                            "schedule_name": &schedule_outcome.schedule_name,
                            "failure_count": schedule_outcome.failure_count,
                            "max_failures": schedule_outcome.max_failures,
                            "retry_delay_secs": schedule_outcome.retry_delay_secs,
                            "next_run_at": &schedule_outcome.next_run_at,
                            "disabled": !schedule_outcome.enabled,
                            "error": error,
                            "job_id": schedule_outcome.job_id,
                            "job_status": &schedule_outcome.status,
                        }),
                        created_at: unix_now().to_string(),
                    });
                }
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    SELECT $1, $2, $3, $4, NULL, $5
                    WHERE NOT EXISTS (
                        SELECT 1
                        FROM audit_logs
                        WHERE action = $3
                          AND target = $4
                          AND metadata->>'job_id' = $6
                    )
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(summary.actor_id)
                .bind("schedule.job_failed")
                .bind(format!("schedule:{}", schedule_outcome.schedule_id))
                .bind(json!({
                    "schedule_id": schedule_outcome.schedule_id,
                    "schedule_name": &schedule_outcome.schedule_name,
                    "failure_count": schedule_outcome.failure_count,
                    "max_failures": schedule_outcome.max_failures,
                    "retry_delay_secs": schedule_outcome.retry_delay_secs,
                    "next_run_at": &schedule_outcome.next_run_at,
                    "disabled": !schedule_outcome.enabled,
                    "error": error,
                    "job_id": schedule_outcome.job_id,
                    "job_status": &schedule_outcome.status,
                }))
                .bind(schedule_outcome.job_id.to_string())
                .execute(pool)
                .await?;
            }
        }
        let event_id = format!(
            "schedule:{}:job:{}:failed",
            schedule_outcome.schedule_id, schedule_outcome.job_id
        );
        let mut predicates = vec![
            "schedule.failed".to_string(),
            format!("schedule.id:{}", schedule_outcome.schedule_id),
            format!("schedule.name:{}", schedule_outcome.schedule_name),
            format!("job.status:{}", schedule_outcome.status),
            format!("job.status.become_{}", schedule_outcome.status),
            format!("job.type:{}", summary.command_type),
        ];
        predicates.sort();
        predicates.dedup();
        self.record_webhook_event(WebhookEventCandidate {
            kind: "schedule.failed".to_string(),
            event_id: event_id.clone(),
            event_predicates: predicates.clone(),
            subject_client_ids: summary.targets.clone(),
            actor_id: summary.actor_id,
            payload: json!({
                "event": {
                    "kind": "schedule.failed",
                    "id": event_id,
                    "predicates": &predicates,
                },
                "schedule": {
                    "id": schedule_outcome.schedule_id,
                    "name": &schedule_outcome.schedule_name,
                    "failure_count": schedule_outcome.failure_count,
                    "max_failures": schedule_outcome.max_failures,
                    "retry_delay_secs": schedule_outcome.retry_delay_secs,
                    "next_run_at": &schedule_outcome.next_run_at,
                    "disabled": !schedule_outcome.enabled,
                    "error": error,
                    "last_job_id": schedule_outcome.job_id,
                    "last_job_status": &schedule_outcome.status,
                    "last_job_error": error,
                },
                "job": {
                    "id": schedule_outcome.job_id,
                    "status": &schedule_outcome.status,
                    "type": &summary.command_type,
                    "source_schedule_id": schedule_outcome.schedule_id,
                    "target_count": summary.target_count,
                    "target_ids": &summary.targets,
                },
            }),
        })
        .await?;
        Ok(())
    }

    async fn record_job_created_webhook_event(
        &self,
        event: JobCreatedWebhookEvent<'_>,
    ) -> Result<()> {
        let event_id = format!("job:{}:created", event.job_id);
        let predicates = job_webhook_predicates(event.command_type, event.status, true);
        self.record_webhook_event(WebhookEventCandidate {
            kind: "job.created".to_string(),
            event_id: event_id.clone(),
            event_predicates: predicates.clone(),
            subject_client_ids: event.resolved_targets.to_vec(),
            actor_id: event.actor_id,
            payload: json!({
                "event": {
                    "kind": "job.created",
                    "id": &event_id,
                    "predicates": &predicates,
                },
                "job": {
                    "id": event.job_id,
                    "status": event.status,
                    "type": event.command_type,
                    "privileged": event.privileged,
                    "payload_hash": event.command_hash,
                    "source_schedule_id": event.source_schedule_id,
                    "target_count": event.resolved_targets.len(),
                    "target_ids": event.resolved_targets,
                    "operation": event.operation
                        .map(|value| json!(value))
                        .unwrap_or(serde_json::Value::Null),
                },
            }),
        })
        .await?;
        Ok(())
    }

    pub(crate) async fn record_job_target_webhook_event(
        &self,
        job_id: Uuid,
        client_id: &str,
        outcome: &TargetDispatchOutcome,
    ) -> Result<()> {
        let Some(summary) = self.webhook_job_summary(job_id).await? else {
            return Ok(());
        };
        let event_id = format!("job:{job_id}:target:{client_id}:status:{}", outcome.status);
        let mut predicates = job_webhook_predicates(&summary.command_type, &summary.status, false);
        predicates.push(format!("job.target.status:{}", outcome.status));
        predicates.sort();
        predicates.dedup();
        self.record_webhook_event(WebhookEventCandidate {
            kind: "job.target.status".to_string(),
            event_id: event_id.clone(),
            event_predicates: predicates.clone(),
            subject_client_ids: vec![client_id.to_string()],
            actor_id: summary.actor_id,
            payload: json!({
                "event": {
                    "kind": "job.target.status",
                    "id": &event_id,
                    "predicates": &predicates,
                },
                "job": {
                    "id": job_id,
                    "status": &summary.status,
                    "type": &summary.command_type,
                    "privileged": summary.privileged,
                    "payload_hash": &summary.payload_hash,
                    "source_schedule_id": summary.source_schedule_id,
                    "target_count": summary.target_count,
                    "target_ids": &summary.targets,
                    "target": {
                        "client_id": client_id,
                        "status": &outcome.status,
                        "accepted": outcome.accepted,
                        "exit_code": outcome.exit_code,
                        "message": &outcome.message,
                    },
                },
            }),
        })
        .await?;
        Ok(())
    }

    async fn record_job_status_webhook_event(&self, job_id: Uuid, status: &str) -> Result<()> {
        let Some(summary) = self.webhook_job_summary(job_id).await? else {
            return Ok(());
        };
        let event_id = format!("job:{job_id}:status:{status}");
        let predicates = job_webhook_predicates(&summary.command_type, status, false);
        self.record_webhook_event(WebhookEventCandidate {
            kind: "job.status".to_string(),
            event_id: event_id.clone(),
            event_predicates: predicates.clone(),
            subject_client_ids: summary.targets.clone(),
            actor_id: summary.actor_id,
            payload: json!({
                "event": {
                    "kind": "job.status",
                    "id": &event_id,
                    "predicates": &predicates,
                },
                "job": {
                    "id": job_id,
                    "status": status,
                    "type": &summary.command_type,
                    "privileged": summary.privileged,
                    "payload_hash": &summary.payload_hash,
                    "source_schedule_id": summary.source_schedule_id,
                    "target_count": summary.target_count,
                    "target_ids": &summary.targets,
                },
            }),
        })
        .await?;
        Ok(())
    }

    async fn webhook_job_summary(&self, job_id: Uuid) -> Result<Option<WebhookJobSummary>> {
        match self {
            Self::Memory(memory) => {
                let Some(job) = memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .find(|job| job.id == job_id)
                    .cloned()
                else {
                    return Ok(None);
                };
                let target_records = memory
                    .job_targets
                    .read()
                    .await
                    .iter()
                    .filter(|target| target.job_id == job_id)
                    .cloned()
                    .collect::<Vec<_>>();
                let targets = target_records
                    .iter()
                    .map(|target| target.client_id.clone())
                    .collect::<Vec<_>>();
                let target_statuses = target_records
                    .iter()
                    .map(|target| target.status.clone())
                    .collect::<Vec<_>>();
                let source_schedule_id = memory
                    .job_source_schedule_ids
                    .read()
                    .await
                    .get(&job_id)
                    .copied();
                Ok(Some(WebhookJobSummary {
                    actor_id: job.actor_id,
                    command_type: job.command_type,
                    privileged: job.privileged,
                    status: job.status,
                    target_count: job.target_count,
                    payload_hash: job.payload_hash,
                    source_schedule_id,
                    targets,
                    target_statuses,
                }))
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT
                        job.actor_id,
                        job.command_type,
                        job.privileged,
                        job.status,
                        job.target_count,
                        job.payload_hash,
                        job.source_schedule_id,
                        COALESCE(
                            (
                                SELECT array_agg(target.client_id ORDER BY target.client_id)
                                FROM job_targets target
                                WHERE target.job_id = job.id
                            ),
                            ARRAY[]::TEXT[]
                        ) AS targets,
                        COALESCE(
                            (
                                SELECT array_agg(target.status ORDER BY target.client_id)
                                FROM job_targets target
                                WHERE target.job_id = job.id
                            ),
                            ARRAY[]::TEXT[]
                        ) AS target_statuses
                    FROM jobs job
                    WHERE job.id = $1
                    "#,
                )
                .bind(job_id)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(None);
                };
                Ok(Some(WebhookJobSummary {
                    actor_id: row.try_get("actor_id")?,
                    command_type: row.try_get("command_type")?,
                    privileged: row.try_get("privileged")?,
                    status: row.try_get("status")?,
                    target_count: row.try_get("target_count")?,
                    payload_hash: row.try_get("payload_hash")?,
                    source_schedule_id: row.try_get("source_schedule_id")?,
                    targets: row.try_get("targets")?,
                    target_statuses: row.try_get("target_statuses")?,
                }))
            }
        }
    }
}
