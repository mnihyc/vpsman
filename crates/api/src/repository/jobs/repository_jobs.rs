use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use anyhow::{bail, Result};
use base64::Engine as _;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, Postgres, Row, Transaction};
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    job_command_safety, job_command_safety_by_operation_type, payload_hash,
    runtime_config_content_hash, AgentRuntimeConfig, CommandOutput, JobCommand, JobCommandSafety,
    DEFAULT_MAX_JOB_TIMEOUT_SECS, JOB_COMMAND_SAFETY_EXCLUSIVE,
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
const MAX_CAPABILITY_DEGRADED_REASON_CHARS: usize = 256;
const MAX_CAPABILITY_DEGRADED_HINT_CHARS: usize = 2048;
const INVALID_JOB_OPERATION_CODE: &str = "invalid_job_operation";
const MAX_JOB_OPERATION_DECODE_ERROR_CHARS: usize = 1024;
const INVALID_JOB_OPERATION_RETRY_DEFER_SECS: i32 = 30;
const INVALID_JOB_OPERATION_RETRY_MARKER: &str = "invalid_job_operation:";

use crate::model::*;
use crate::model_webhook_rules::WebhookEventCandidate;
use crate::repository::Repository;
use crate::repository_job_outputs::{append_lock_keys, job_output_sequence_contiguous_in_views};
use crate::repository_key_lifecycle::{
    lock_postgres_agent_identity_lifecycle, require_visible_memory_clients,
    require_visible_postgres_clients_in_tx,
};
use crate::repository_runtime_config::{
    queue_runtime_config_apply_memory_state, queue_runtime_config_apply_postgres_in_tx,
};
use crate::runtime_config::redact_runtime_tunnel_credentials;
use crate::util::{
    compare_timestamps_desc, limit_or_default, offset_or_default, output_stream_name,
    search_pattern, sort_descending,
};
use crate::{unix_now, TargetDispatchOutcome};

#[derive(Debug)]
pub(crate) struct PrecompletedJobTarget {
    pub(crate) client_id: String,
    pub(crate) outcome: TargetDispatchOutcome,
}

#[derive(Clone, Debug)]
struct PendingRuntimeConfigApply {
    client_id: String,
    version: u64,
    content_hash: String,
    config: AgentRuntimeConfig,
    reason: String,
}

fn capability_degraded_metadata(
    data: &[u8],
    command_type: &str,
    client_id: &str,
) -> Option<(String, String)> {
    let payload = serde_json::from_slice::<Value>(data).ok()?;
    if payload.get("type")?.as_str()? != "capability_degraded"
        || payload.get("status")?.as_str()? != TARGET_STATUS_SKIPPED
        || payload.get("client_id")?.as_str()? != client_id
        || payload.get("command_type")?.as_str()? != command_type
    {
        return None;
    }
    let reason = payload.get("reason")?.as_str()?.trim();
    let hint = payload.get("hint")?.as_str()?.trim();
    if reason.is_empty()
        || hint.is_empty()
        || reason.chars().count() > MAX_CAPABILITY_DEGRADED_REASON_CHARS
        || hint.chars().count() > MAX_CAPABILITY_DEGRADED_HINT_CHARS
    {
        return None;
    }
    Some((reason.to_string(), hint.to_string()))
}

fn capability_degraded_outcome_metadata(
    outcome: &TargetDispatchOutcome,
    command_type: &str,
    client_id: &str,
) -> Option<(String, String)> {
    outcome.outputs.iter().find_map(|output| {
        if output.stream != vpsman_common::OutputStream::Status {
            return None;
        }
        capability_degraded_metadata(&output.data, command_type, client_id)
    })
}

#[derive(Clone, Debug)]
struct PreparedJobRollout {
    policy: JobRolloutPolicy,
    target_batches: HashMap<String, u16>,
    total_batches: u16,
}

fn prepare_job_rollout(
    policy: Option<&JobRolloutPolicy>,
    resolved_targets: &[String],
) -> Result<Option<PreparedJobRollout>> {
    let Some(policy) = policy else {
        return Ok(None);
    };
    let canary_set = policy
        .canary_client_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut remaining = resolved_targets
        .iter()
        .filter(|client_id| !canary_set.contains(client_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    remaining.sort();
    let mut target_batches = policy
        .canary_client_ids
        .iter()
        .cloned()
        .map(|client_id| (client_id, 0_u16))
        .collect::<HashMap<_, _>>();
    for (index, chunk) in remaining
        .chunks(usize::from(policy.batch_size.max(1)))
        .enumerate()
    {
        let batch_index = u16::try_from(index + 1)?;
        for client_id in chunk {
            target_batches.insert(client_id.clone(), batch_index);
        }
    }
    anyhow::ensure!(
        target_batches.len() == resolved_targets.len(),
        "job_rollout_target_assignment_incomplete"
    );
    let total_batches = target_batches.values().copied().max().unwrap_or(0) + 1;
    Ok(Some(PreparedJobRollout {
        policy: policy.clone(),
        target_batches,
        total_batches,
    }))
}

fn pending_runtime_config_apply(
    operation: &JobCommand,
    resolved_targets: &[String],
) -> Result<Option<PendingRuntimeConfigApply>> {
    let JobCommand::RuntimeConfigSync {
        desired_version,
        reason,
        config,
    } = operation
    else {
        return Ok(None);
    };
    anyhow::ensure!(
        resolved_targets.len() == 1,
        "runtime_config_sync_requires_single_target"
    );
    anyhow::ensure!(
        config.version == *desired_version,
        "runtime_config_version_mismatch"
    );
    Ok(Some(PendingRuntimeConfigApply {
        client_id: resolved_targets[0].clone(),
        version: *desired_version,
        content_hash: runtime_config_content_hash(config)?,
        config: (**config).clone(),
        reason: reason.clone(),
    }))
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalizedTarget {
    pub(crate) event_id: Uuid,
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) outcome: TargetDispatchOutcome,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalizedJob {
    pub(crate) job_id: Uuid,
    pub(crate) status: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TerminalizationBatch {
    pub(crate) targets: Vec<TerminalizedTarget>,
    pub(crate) jobs: Vec<TerminalizedJob>,
}

impl TerminalizationBatch {
    pub(crate) fn push_target(
        &mut self,
        event_id: Uuid,
        job_id: Uuid,
        client_id: impl Into<String>,
        outcome: TargetDispatchOutcome,
    ) {
        self.targets.push(TerminalizedTarget {
            event_id,
            job_id,
            client_id: client_id.into(),
            outcome,
        });
    }

    pub(crate) fn push_job(&mut self, job_id: Uuid, status: impl Into<String>) {
        self.jobs.push(TerminalizedJob {
            job_id,
            status: status.into(),
        });
    }

    pub(crate) fn extend(&mut self, other: TerminalizationBatch) {
        self.targets.extend(other.targets);
        self.jobs.extend(other.jobs);
    }
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

fn target_outcome_event_payload(outcome: &TargetDispatchOutcome) -> Value {
    json!({
        "status": &outcome.status,
        "exit_code": outcome.exit_code,
        "accepted": outcome.accepted,
        "message": &outcome.message,
        "received_at": &outcome.received_at,
    })
}

fn target_outcome_from_event_payload(
    status: &str,
    payload: Option<Value>,
) -> TargetDispatchOutcome {
    let payload = payload.unwrap_or_else(|| json!({}));
    let message = payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(status)
        .to_string();
    let exit_code = payload
        .get("exit_code")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    let accepted = payload
        .get("accepted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let received_at = payload
        .get("received_at")
        .and_then(Value::as_str)
        .map(str::to_string);
    TargetDispatchOutcome {
        status: status.to_string(),
        exit_code,
        #[cfg(test)]
        command_version: None,
        accepted,
        message,
        received_at,
        outputs: Vec::new(),
    }
}

fn synthetic_terminal_outcome(
    status: &str,
    message: impl Into<String>,
    exit_code: Option<i32>,
    accepted: bool,
) -> TargetDispatchOutcome {
    TargetDispatchOutcome {
        status: status.to_string(),
        exit_code,
        #[cfg(test)]
        command_version: None,
        accepted,
        message: message.into(),
        received_at: None,
        outputs: Vec::new(),
    }
}

async fn insert_agent_update_lifecycle_for_target_outcome_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
    operation: &JobCommand,
    outcome: &TargetDispatchOutcome,
) -> Result<()> {
    let event = match operation {
        JobCommand::AgentUpdateActivate {
            staged_sha256_hex, ..
        } if outcome.status == TARGET_STATUS_COMPLETED => Some((
            "agent_update.activation_completed",
            json!({
                "activation_job_id": job_id,
                "client_id": client_id,
                "artifact_sha256_hex": staged_sha256_hex.to_ascii_lowercase(),
                "status": "activation_completed",
                "result": "succeeded",
                "origin_kind": "gateway_ingest",
                "component": "agent-update-lifecycle",
            }),
        )),
        JobCommand::AgentUpdateActivate {
            staged_sha256_hex, ..
        } if agent_update_activation_failure_status(&outcome.status) => Some((
            "agent_update.activation_failed",
            json!({
                "activation_job_id": job_id,
                "client_id": client_id,
                "artifact_sha256_hex": staged_sha256_hex.to_ascii_lowercase(),
                "activation_outcome_status": outcome.status,
                "exit_code": outcome.exit_code,
                "message": outcome.message,
                "status": "activation_failed",
                "rollback_recommended": true,
                "result": "failed",
                "origin_kind": "gateway_ingest",
                "component": "agent-update-lifecycle",
            }),
        )),
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex,
        } if outcome.status == TARGET_STATUS_COMPLETED => Some((
            "agent_update.rollback_completed",
            json!({
                "rollback_job_id": job_id,
                "client_id": client_id,
                "rollback_sha256_hex": rollback_sha256_hex.as_deref().map(str::to_ascii_lowercase),
                "status": "rolled_back",
                "result": "succeeded",
                "origin_kind": "gateway_ingest",
                "component": "agent-update-lifecycle",
            }),
        )),
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex,
        } if agent_update_activation_failure_status(&outcome.status) => Some((
            "agent_update.rollback_failed",
            json!({
                "rollback_job_id": job_id,
                "client_id": client_id,
                "rollback_sha256_hex": rollback_sha256_hex.as_deref().map(str::to_ascii_lowercase),
                "rollback_outcome_status": outcome.status,
                "exit_code": outcome.exit_code,
                "message": outcome.message,
                "status": "rollback_failed",
                "result": "failed",
                "origin_kind": "gateway_ingest",
                "component": "agent-update-lifecycle",
            }),
        )),
        _ => None,
    };
    let Some((action, metadata)) = event else {
        return Ok(());
    };
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, NULL, $2, $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(action)
    .bind(format!("client:{client_id}"))
    .bind(metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn insert_agent_update_lifecycle_for_stored_job_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
    outcome: &TargetDispatchOutcome,
) -> Result<()> {
    if outcome.status != TARGET_STATUS_COMPLETED
        && !agent_update_activation_failure_status(&outcome.status)
    {
        return Ok(());
    }
    let raw_operation: Option<sqlx::types::Json<Value>> =
        sqlx::query_scalar("SELECT operation FROM jobs WHERE id = $1")
            .bind(job_id)
            .fetch_optional(&mut **tx)
            .await?
            .flatten();
    match decode_persisted_job_operation(raw_operation) {
        Ok(operation) => {
            insert_agent_update_lifecycle_for_target_outcome_in_tx(
                tx, job_id, client_id, &operation, outcome,
            )
            .await?;
        }
        Err(error) => warn!(
            job_id = %job_id,
            client_id,
            %error,
            "skipping agent-update lifecycle audit for invalid stored job operation"
        ),
    }
    Ok(())
}

fn decode_persisted_job_operation(
    operation: Option<sqlx::types::Json<Value>>,
) -> std::result::Result<JobCommand, String> {
    let operation = operation
        .map(|operation| operation.0)
        .ok_or_else(|| "operation is null".to_string())?;
    serde_json::from_value(operation).map_err(|error| {
        error
            .to_string()
            .chars()
            .take(MAX_JOB_OPERATION_DECODE_ERROR_CHARS)
            .collect()
    })
}

fn invalid_job_operation_message(context: &str, decode_error: &str) -> String {
    let decode_error = decode_error
        .chars()
        .take(MAX_JOB_OPERATION_DECODE_ERROR_CHARS)
        .collect::<String>();
    format!("{context}: {decode_error}")
}

struct InvalidJobOperationEvidence<'a> {
    phase: &'a str,
    message: &'a str,
    decode_error: &'a str,
    process_incarnation_id: Option<Uuid>,
}

struct InvalidJobOperationTarget {
    job_id: Uuid,
    client_id: String,
    message: String,
    decode_error: String,
    process_incarnation_id: Option<Uuid>,
}

fn invalid_job_operation_status_output_value(
    job_id: Uuid,
    client_id: &str,
    status: &str,
    evidence: &InvalidJobOperationEvidence<'_>,
) -> Value {
    json!({
        "type": "dispatch_error",
        "status": status,
        "code": INVALID_JOB_OPERATION_CODE,
        "reason": INVALID_JOB_OPERATION_CODE,
        "phase": evidence.phase,
        "message": evidence.message,
        "decode_error": evidence.decode_error,
        "job_id": job_id,
        "client_id": client_id,
        "process_incarnation_id": evidence.process_incarnation_id,
    })
}

pub(crate) async fn enqueue_target_terminal_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    client_id: &str,
    outcome: &TargetDispatchOutcome,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO job_terminal_events (
            id,
            event_kind,
            job_id,
            client_id,
            status,
            outcome
        )
        SELECT $1, 'target_terminalized', $2, $3, $4, $5
        WHERE NOT EXISTS (
            SELECT 1
            FROM job_terminal_events
            WHERE event_kind = 'target_terminalized'
              AND job_id = $2
              AND client_id = $3
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(job_id)
    .bind(client_id)
    .bind(&outcome.status)
    .bind(sqlx::types::Json(target_outcome_event_payload(outcome)))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn enqueue_job_terminal_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    status: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO job_terminal_events (
            id,
            event_kind,
            job_id,
            client_id,
            status,
            outcome
        )
        SELECT $1, 'job_terminalized', $2, NULL, $3, NULL
        WHERE NOT EXISTS (
            SELECT 1
            FROM job_terminal_events
            WHERE event_kind = 'job_terminalized'
              AND job_id = $2
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(job_id)
    .bind(status)
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
        VALUES ($1, NULL, $2, $3, (SELECT payload_hash FROM jobs WHERE id = $5), $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind("job.target_result")
    .bind(format!("client:{client_id}"))
    .bind(json!({
        "job_id": job_id,
        "status": outcome.status,
        "result": outcome.status,
        "exit_code": outcome.exit_code,
        "accepted": outcome.accepted,
        "message": outcome.message,
        "received_at": outcome.received_at,
        "origin_kind": "control_plane",
        "component": "job-dispatch-validation",
    }))
    .bind(job_id)
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

fn backup_request_terminal_status_for_target(status: &str) -> Option<BackupRequestStatus> {
    match status {
        TARGET_STATUS_CANCELED => Some(BackupRequestStatus::ExecutionCanceled),
        TARGET_STATUS_FAILED
        | TARGET_STATUS_REJECTED
        | TARGET_STATUS_AGENT_LOST
        | TARGET_STATUS_AGENT_TIMEOUT
        | TARGET_STATUS_CONTROL_TIMEOUT => Some(BackupRequestStatus::ExecutionFailed),
        _ => None,
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
#[path = "tests_repository_jobs.rs"]
mod tests;

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
    let output_type = if reason_code == "target_suspended" {
        "target_suspended"
    } else {
        "target_skipped"
    };
    json!({
        "type": output_type,
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

async fn terminalize_invalid_job_operation_target_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    target: &InvalidJobOperationTarget,
    status: &str,
    phase: &str,
    request_cancel: bool,
    control_deadline_extra_secs: Option<i32>,
) -> Result<bool> {
    let component = if phase == "control_deadline_expiry" {
        "job-deadline-reconciler"
    } else {
        "job-dispatcher"
    };
    let updated = sqlx::query(
        r#"
        UPDATE job_targets target
        SET
            status = $3,
            message = $4,
            exit_code = NULL,
            started_at = COALESCE(started_at, now()),
            completed_at = now(),
            result_received_at = now(),
            dispatch_lease_until = NULL,
            cancel_requested_at = CASE
                WHEN $5 THEN COALESCE(cancel_requested_at, now())
                ELSE cancel_requested_at
            END,
            last_dispatch_error = $4
        FROM jobs job
        WHERE target.job_id = $1
          AND target.client_id = $2
          AND job.id = target.job_id
          AND target.completed_at IS NULL
          AND target.status IN ('dispatching', 'running')
          AND (
            $6::integer IS NULL
            OR (
              target.deadline_at IS NOT NULL
              AND target.deadline_at <= now()
              AND target.started_at IS NOT NULL
              AND target.started_at
                    + make_interval(secs => (job.max_timeout_secs + $6)::integer) <= now()
              AND (
                ($7::uuid IS NULL AND target.process_incarnation_id IS NULL)
                OR target.process_incarnation_id = $7::uuid
              )
            )
          )
        "#,
    )
    .bind(target.job_id)
    .bind(&target.client_id)
    .bind(status)
    .bind(&target.message)
    .bind(request_cancel)
    .bind(control_deadline_extra_secs)
    .bind(target.process_incarnation_id)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Ok(false);
    }
    append_synthetic_status_output_in_tx(
        tx,
        target.job_id,
        &target.client_id,
        invalid_job_operation_status_output_value(
            target.job_id,
            &target.client_id,
            status,
            &InvalidJobOperationEvidence {
                phase,
                message: &target.message,
                decode_error: &target.decode_error,
                process_incarnation_id: target.process_incarnation_id,
            },
        ),
        None,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES (
            $1,
            NULL,
            'job.target_result',
            $2,
            (SELECT payload_hash FROM jobs WHERE id = $4),
            $3
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(format!("client:{}", target.client_id))
    .bind(json!({
        "job_id": target.job_id,
        "status": status,
        "result": status,
        "message": target.message,
        "reason": INVALID_JOB_OPERATION_CODE,
        "phase": phase,
        "decode_error": target.decode_error,
        "process_incarnation_id": target.process_incarnation_id,
        "origin_kind": "control_plane",
        "component": component,
    }))
    .bind(target.job_id)
    .execute(&mut **tx)
    .await?;
    let outcome = synthetic_terminal_outcome(status, target.message.clone(), None, false);
    enqueue_target_terminal_event_in_tx(tx, target.job_id, &target.client_id, &outcome).await?;
    finish_jobs_in_tx_and_reconcile_event_sources(tx, &[target.job_id]).await?;
    Ok(true)
}

async fn terminalize_invalid_job_operation_target(
    pool: &sqlx::PgPool,
    target: &InvalidJobOperationTarget,
    status: &str,
    phase: &str,
    request_cancel: bool,
    control_deadline_extra_secs: Option<i32>,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let terminalized = terminalize_invalid_job_operation_target_in_tx(
        &mut tx,
        target,
        status,
        phase,
        request_cancel,
        control_deadline_extra_secs,
    )
    .await?;
    tx.commit().await?;
    Ok(terminalized)
}

pub(crate) async fn finish_job_in_tx_if_all_targets_terminal_and_enqueue_event(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
) -> Result<Option<String>> {
    let Some(job_row) = sqlx::query(
        r#"
        SELECT completed_at::text AS completed_at
        FROM jobs
        WHERE id = $1
        FOR NO KEY UPDATE
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
        enqueue_job_terminal_event_in_tx(tx, job_id, &status).await?;
        Ok(Some(status))
    } else {
        Ok(None)
    }
}

pub(crate) async fn finish_jobs_in_tx_and_reconcile_event_sources(
    tx: &mut Transaction<'_, Postgres>,
    job_ids: &[Uuid],
) -> Result<()> {
    let mut job_ids = job_ids.to_vec();
    job_ids.sort();
    job_ids.dedup();
    for job_id in job_ids {
        finish_job_in_tx_if_all_targets_terminal_and_enqueue_event(tx, job_id).await?;
        crate::repository_operational_alerts::reconcile_postgres_job_event_sources_in_tx(
            tx, job_id,
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn skip_unstarted_queued_targets_for_client_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    reason_code: &str,
    message: &str,
) -> Result<Vec<Uuid>> {
    skip_undelivered_targets_for_client_in_tx(tx, client_id, reason_code, message, false, &[]).await
}

pub(crate) async fn skip_suspended_undelivered_targets_for_client_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    reason_code: &str,
    message: &str,
) -> Result<Vec<Uuid>> {
    skip_suspended_undelivered_targets_for_client_except_in_tx(
        tx,
        client_id,
        reason_code,
        message,
        &[],
    )
    .await
}

pub(crate) async fn skip_suspended_undelivered_targets_for_client_except_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    reason_code: &str,
    message: &str,
    protected_enqueued_job_ids: &[Uuid],
) -> Result<Vec<Uuid>> {
    skip_undelivered_targets_for_client_in_tx(
        tx,
        client_id,
        reason_code,
        message,
        true,
        protected_enqueued_job_ids,
    )
    .await
}

async fn skip_undelivered_targets_for_client_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    reason_code: &str,
    message: &str,
    include_claimed_dispatching: bool,
    protected_enqueued_job_ids: &[Uuid],
) -> Result<Vec<Uuid>> {
    let rows = sqlx::query(
        r#"
        SELECT job_id, client_id
        FROM job_targets
        WHERE client_id = $1
          AND completed_at IS NULL
          AND NOT (job_id = ANY($3::uuid[]))
          AND (
                (
                    status = 'queued'
                    AND started_at IS NULL
                    AND process_incarnation_id IS NULL
                )
                OR (
                    $2
                    AND status = 'dispatching'
                    AND started_at IS NOT NULL
                    AND process_incarnation_id IS NOT NULL
                )
          )
        ORDER BY job_id
        FOR UPDATE
        "#,
    )
    .bind(client_id)
    .bind(include_claimed_dispatching)
    .bind(protected_enqueued_job_ids)
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
                deadline_at = NULL,
                last_dispatch_error = NULL
            WHERE job_id = $1
              AND client_id = $2
              AND completed_at IS NULL
              AND NOT (job_id = ANY($5::uuid[]))
              AND (
                    (
                        status = 'queued'
                        AND started_at IS NULL
                        AND process_incarnation_id IS NULL
                    )
                    OR (
                        $4
                        AND status = 'dispatching'
                        AND started_at IS NOT NULL
                        AND process_incarnation_id IS NOT NULL
                    )
              )
            "#,
        )
        .bind(job_id)
        .bind(&target_client_id)
        .bind(message)
        .bind(include_claimed_dispatching)
        .bind(protected_enqueued_job_ids)
        .execute(&mut **tx)
        .await?;
        if updated.rows_affected() > 0 {
            let outcome =
                synthetic_terminal_outcome(TARGET_STATUS_SKIPPED, message, Some(0), false);
            enqueue_target_terminal_event_in_tx(tx, job_id, &target_client_id, &outcome).await?;
            sqlx::query(
                r#"
                INSERT INTO audit_logs (
                    id, actor_id, action, target, command_hash, metadata
                )
                VALUES (
                    $1,
                    NULL,
                    'job.target_result',
                    $2,
                    (SELECT payload_hash FROM jobs WHERE id = $4),
                    $3
                )
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(format!("client:{target_client_id}"))
            .bind(json!({
                "job_id": job_id,
                "status": TARGET_STATUS_SKIPPED,
                "result": TARGET_STATUS_SKIPPED,
                "exit_code": 0,
                "accepted": false,
                "message": message,
                "reason": reason_code,
                "origin_kind": "control_plane",
                "component": "client-lifecycle",
            }))
            .bind(job_id)
            .execute(&mut **tx)
            .await?;
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
        let outcome = synthetic_terminal_outcome(TARGET_STATUS_AGENT_LOST, message, None, false);
        enqueue_target_terminal_event_in_tx(tx, job_id, &target_client_id, &outcome).await?;
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, actor_id, action, target, command_hash, metadata
            )
            VALUES (
                $1,
                NULL,
                'job.target_result',
                $2,
                (SELECT payload_hash FROM jobs WHERE id = $4),
                $3
            )
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(format!("client:{target_client_id}"))
        .bind(json!({
            "job_id": job_id,
            "status": TARGET_STATUS_AGENT_LOST,
            "result": TARGET_STATUS_AGENT_LOST,
            "message": message,
            "reason": code,
            "expected_process_incarnation_id": expected_process_incarnation_id,
            "current_process_incarnation_id": current_process_incarnation_id,
            "origin_kind": "control_plane",
            "component": "client-lifecycle",
        }))
        .bind(job_id)
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
        || job
            .causation_id
            .map(|id| id.to_string().to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || job
            .schedule_lineage
            .iter()
            .any(|id| id.to_string().to_ascii_lowercase().contains(needle))
}

fn compare_job_approval(
    left: &JobApprovalView,
    right: &JobApprovalView,
    sort: Option<&str>,
) -> Ordering {
    match sort.unwrap_or("requested_at") {
        "command_type" | "command" => left.command_type.cmp(&right.command_type),
        "decided_at" => left.decided_at.cmp(&right.decided_at),
        "job_id" => left.job_id.cmp(&right.job_id),
        "payload_hash" | "hash" => left.payload_hash.cmp(&right.payload_hash),
        "requester" | "requester_username" => {
            left.requester_username.cmp(&right.requester_username)
        }
        "risk" => left.risk.cmp(&right.risk),
        "status" => left.status.cmp(&right.status),
        "target_count" | "targets" => left.target_count.cmp(&right.target_count),
        _ => compare_text_or_number(&left.requested_at, &right.requested_at),
    }
}

fn job_approval_matches_search(approval: &JobApprovalView, needle: &str) -> bool {
    approval
        .id
        .to_string()
        .to_ascii_lowercase()
        .contains(needle)
        || approval
            .job_id
            .to_string()
            .to_ascii_lowercase()
            .contains(needle)
        || approval.status.to_ascii_lowercase().contains(needle)
        || approval.command_type.to_ascii_lowercase().contains(needle)
        || approval
            .selector_expression
            .to_ascii_lowercase()
            .contains(needle)
        || approval
            .target_client_ids
            .iter()
            .any(|target| target.to_ascii_lowercase().contains(needle))
        || approval.payload_hash.to_ascii_lowercase().contains(needle)
        || approval
            .request_fingerprint
            .to_ascii_lowercase()
            .contains(needle)
        || approval
            .requester_username
            .to_ascii_lowercase()
            .contains(needle)
        || approval.risk.to_ascii_lowercase().contains(needle)
}

fn job_approval_allows_dispatch(
    approvals: &[JobApprovalView],
    approval_id: Option<Uuid>,
    job_id: Uuid,
    payload_hash: &str,
    request_fingerprint: Option<&str>,
) -> bool {
    approval_id.is_none_or(|approval_id| {
        approvals
            .iter()
            .find(|approval| approval.id == approval_id)
            .is_some_and(|approval| {
                approval.job_id == job_id
                    && approval.status == "approved"
                    && approval.payload_hash == payload_hash
                    && request_fingerprint.is_some_and(|fingerprint| {
                        approval.request_fingerprint.as_str() == fingerprint
                    })
            })
    })
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

fn job_approval_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort.unwrap_or("requested_at"), descending) {
        ("command_type" | "command", true) => "command_type DESC, id DESC",
        ("command_type" | "command", false) => "command_type ASC, id ASC",
        ("decided_at", true) => "decided_at DESC NULLS LAST, id DESC",
        ("decided_at", false) => "decided_at ASC NULLS LAST, id ASC",
        ("job_id", true) => "job_id DESC, id DESC",
        ("job_id", false) => "job_id ASC, id ASC",
        ("payload_hash" | "hash", true) => "payload_hash DESC, id DESC",
        ("payload_hash" | "hash", false) => "payload_hash ASC, id ASC",
        ("requester" | "requester_username", true) => "requester_username DESC, id DESC",
        ("requester" | "requester_username", false) => "requester_username ASC, id ASC",
        ("risk", true) => "risk DESC, id DESC",
        ("risk", false) => "risk ASC, id ASC",
        ("status", true) => "status DESC, id DESC",
        ("status", false) => "status ASC, id ASC",
        ("target_count" | "targets", true) => "target_count DESC, id DESC",
        ("target_count" | "targets", false) => "target_count ASC, id ASC",
        (_, true) => "requested_at DESC, id DESC",
        (_, false) => "requested_at ASC, id ASC",
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
    actor_username: Option<String>,
    actor_role: Option<String>,
    command_type: String,
    privileged: bool,
    status: String,
    target_count: i32,
    payload_hash: String,
    source_schedule_id: Option<Uuid>,
    targets: Vec<String>,
    target_statuses: Vec<String>,
}

pub(crate) struct JobCreatedWebhookEvent<'a> {
    pub(crate) job_id: Uuid,
    pub(crate) command_type: &'a str,
    pub(crate) status: &'a str,
    pub(crate) privileged: bool,
    pub(crate) command_hash: &'a str,
    pub(crate) resolved_targets: &'a [String],
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) source_schedule_id: Option<Uuid>,
    pub(crate) operation: Option<&'a JobCommand>,
}

fn job_created_webhook_event_candidate(event: JobCreatedWebhookEvent<'_>) -> WebhookEventCandidate {
    let event_id = format!("job:{}:created", event.job_id);
    let predicates = job_webhook_predicates(event.command_type, event.status, true);
    let operation = event
        .operation
        .map(|value| {
            let mut value = json!(value);
            redact_runtime_tunnel_credentials(&mut value);
            value
        })
        .unwrap_or(Value::Null);
    WebhookEventCandidate {
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
                "operation": operation,
            },
        }),
    }
}

pub(crate) async fn record_job_created_webhook_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    event: JobCreatedWebhookEvent<'_>,
) -> Result<()> {
    crate::repository_webhook_rules::record_webhook_event_in_tx(
        tx,
        job_created_webhook_event_candidate(event),
        Utc::now(),
    )
    .await?;
    Ok(())
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
    retry_delay_secs: Option<i64>,
    next_run_at: Option<String>,
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
    pub(crate) causation_id: Option<Uuid>,
    pub(crate) schedule_lineage: Vec<Uuid>,
    pub(crate) max_timeout_secs: u64,
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

#[derive(Clone, Debug)]
struct ClaimedJobTerminalEvent {
    id: Uuid,
    event_kind: String,
    job_id: Uuid,
    client_id: Option<String>,
    status: String,
    outcome: Option<Value>,
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

fn job_actor_id(operator: &AuthContext) -> Option<Uuid> {
    if operator.operator.id.is_nil() && operator.operator.role == "system" {
        None
    } else {
        Some(operator.operator.id)
    }
}

fn job_approval_from_row(row: PgRow) -> Result<JobApprovalView> {
    Ok(JobApprovalView {
        id: row.try_get("id")?,
        status: row.try_get("status")?,
        job_id: row.try_get("job_id")?,
        command_type: row.try_get("command_type")?,
        selector_expression: row.try_get("selector_expression")?,
        target_client_ids: row.try_get("target_client_ids")?,
        target_count: row.try_get::<i32, _>("target_count")?.max(0) as usize,
        privileged: row.try_get("privileged")?,
        destructive: row.try_get("destructive")?,
        force_unprivileged: row.try_get("force_unprivileged")?,
        max_timeout_secs: row.try_get::<i64, _>("max_timeout_secs")?.max(1) as u64,
        payload_hash: row.try_get("payload_hash")?,
        request_fingerprint: row.try_get("request_fingerprint")?,
        requester_id: row.try_get("requester_id")?,
        requester_username: row.try_get("requester_username")?,
        requester_role: row.try_get("requester_role")?,
        requested_at: row.try_get("requested_at")?,
        request_reason: row.try_get("request_reason")?,
        risk: row.try_get("risk")?,
        decision_by: row.try_get("decision_by")?,
        decision_username: row.try_get("decision_username")?,
        decision_reason: row.try_get("decision_reason")?,
        decided_at: row.try_get("decided_at")?,
    })
}

fn audit_log_from_row(row: PgRow) -> Result<AuditLogView> {
    let metadata: sqlx::types::Json<serde_json::Value> = row.try_get("metadata")?;
    Ok(AuditLogView {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        action: row.try_get("action")?,
        target: row.try_get("target")?,
        command_hash: row.try_get("command_hash")?,
        metadata: metadata.0,
        created_at: row.try_get("created_at")?,
    })
}

fn job_approval_audit(
    approval: &JobApprovalView,
    action: &'static str,
    operator: &AuthContext,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: job_actor_id(operator),
        action: action.to_string(),
        target: format!("job_approval:{}", approval.id),
        command_hash: Some(approval.payload_hash.clone()),
        metadata: json!({
            "approval_id": approval.id,
            "status": approval.status,
            "job_id": approval.job_id,
            "command_type": approval.command_type,
            "selector_expression": approval.selector_expression,
            "target_client_ids": approval.target_client_ids,
            "target_count": approval.target_count,
            "destructive": approval.destructive,
            "privileged": approval.privileged,
            "force_unprivileged": approval.force_unprivileged,
            "request_fingerprint": approval.request_fingerprint,
            "requester_id": approval.requester_id,
            "requester_username": approval.requester_username,
            "requester_role": approval.requester_role,
            "request_reason": approval.request_reason,
            "risk": approval.risk,
            "decision_by": approval.decision_by,
            "decision_username": approval.decision_username,
            "decision_reason": approval.decision_reason,
            "operator_id": operator.operator.id,
            "operator_username": operator.operator.username,
            "operator_role": operator.operator.role,
            "operator_session_id": operator.audit_session_id(),
            "result": approval.status,
            "origin_kind": if job_actor_id(operator).is_some() { "operator_request" } else { "control_plane" },
            "component": "job-approval-controller",
        }),
        created_at: unix_now().to_string(),
    }
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
                        source_schedule_id,
                        causation_id,
                        schedule_lineage,
                        privileged,
                        status,
                        target_count,
                        payload_hash,
                        max_timeout_secs,
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
                    source_schedule_id: row.try_get("source_schedule_id")?,
                    causation_id: row.try_get("causation_id")?,
                    schedule_lineage: row.try_get("schedule_lineage")?,
                    privileged: row.try_get("privileged")?,
                    status: row.try_get("status")?,
                    target_count: row.try_get("target_count")?,
                    payload_hash: row.try_get("payload_hash")?,
                    max_timeout_secs: row.try_get::<i64, _>("max_timeout_secs")?.max(1) as u64,
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
                let operation: Option<sqlx::types::Json<Value>> = row.try_get("operation")?;
                let operation = decode_persisted_job_operation(operation)
                    .map_err(|error| anyhow::anyhow!("invalid_job_operation: {error}"))?;
                Ok(Some(JobCompletionContext {
                    actor_id: row.try_get("actor_id")?,
                    payload_hash: row.try_get("payload_hash")?,
                    operation,
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

    pub(crate) async fn query_job_approvals(
        &self,
        query: &ListQuery,
    ) -> Result<Vec<JobApprovalView>> {
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
                let mut approvals = memory
                    .job_approvals
                    .read()
                    .await
                    .iter()
                    .filter(|approval| {
                        q.as_deref()
                            .map(|needle| job_approval_matches_search(approval, needle))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                approvals.sort_by(|left, right| {
                    compare_job_approval(left, right, query.sort.as_deref())
                        .then_with(|| left.id.cmp(&right.id))
                });
                if descending {
                    approvals.reverse();
                }
                Ok(approvals
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect())
            }
            Self::Postgres(pool) => {
                let order_by = job_approval_order_by(query.sort.as_deref(), descending);
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT
                        id,
                        status,
                        job_id,
                        command_type,
                        selector_expression,
                        target_client_ids,
                        target_count,
                        privileged,
                        destructive,
                        force_unprivileged,
                        max_timeout_secs,
                        payload_hash,
                        request_fingerprint,
                        requester_id,
                        requester_username,
                        requester_role,
                        requested_at::text AS requested_at,
                        request_reason,
                        risk,
                        decision_by,
                        decision_username,
                        decision_reason,
                        decided_at::text AS decided_at
                    FROM job_approvals
                    WHERE (
                        $3::text IS NULL
                        OR id::text ILIKE $3 ESCAPE '\'
                        OR job_id::text ILIKE $3 ESCAPE '\'
                        OR status ILIKE $3 ESCAPE '\'
                        OR command_type ILIKE $3 ESCAPE '\'
                        OR selector_expression ILIKE $3 ESCAPE '\'
                        OR payload_hash ILIKE $3 ESCAPE '\'
                        OR request_fingerprint ILIKE $3 ESCAPE '\'
                        OR requester_username ILIKE $3 ESCAPE '\'
                        OR risk ILIKE $3 ESCAPE '\'
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
                rows.into_iter().map(job_approval_from_row).collect()
            }
        }
    }

    pub(crate) async fn get_job_approval_request(
        &self,
        approval_id: Uuid,
    ) -> Result<Option<(JobApprovalView, CreateJobRequest)>> {
        match self {
            Self::Memory(memory) => {
                let approval = memory
                    .job_approvals
                    .read()
                    .await
                    .iter()
                    .find(|approval| approval.id == approval_id)
                    .cloned();
                let Some(approval) = approval else {
                    return Ok(None);
                };
                let request = memory
                    .job_approval_requests
                    .read()
                    .await
                    .get(&approval_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("job_approval_request_missing"))?;
                Ok(Some((approval, request)))
            }
            Self::Postgres(pool) => {
                let Some(row) = sqlx::query(
                    r#"
                    SELECT
                        id,
                        status,
                        job_id,
                        command_type,
                        selector_expression,
                        target_client_ids,
                        target_count,
                        privileged,
                        destructive,
                        force_unprivileged,
                        max_timeout_secs,
                        payload_hash,
                        request_fingerprint,
                        requester_id,
                        requester_username,
                        requester_role,
                        requested_at::text AS requested_at,
                        request_reason,
                        risk,
                        decision_by,
                        decision_username,
                        decision_reason,
                        decided_at::text AS decided_at,
                        job_request
                    FROM job_approvals
                    WHERE id = $1
                    "#,
                )
                .bind(approval_id)
                .fetch_optional(pool)
                .await?
                else {
                    return Ok(None);
                };
                let request: sqlx::types::Json<CreateJobRequest> = row.try_get("job_request")?;
                Ok(Some((job_approval_from_row(row)?, request.0)))
            }
        }
    }

    pub(crate) async fn record_job_approval(
        &self,
        approval: JobApprovalView,
        request: &CreateJobRequest,
        operator: &AuthContext,
    ) -> Result<JobApprovalView> {
        match self {
            Self::Memory(memory) => {
                if memory
                    .job_approvals
                    .read()
                    .await
                    .iter()
                    .any(|existing| existing.id == approval.id)
                {
                    bail!("job_approval_id_reused");
                }
                memory.job_approvals.write().await.push(approval.clone());
                memory
                    .job_approval_requests
                    .write()
                    .await
                    .insert(approval.id, request.clone());
                memory.audits.write().await.push(job_approval_audit(
                    &approval,
                    "job.approval_requested",
                    operator,
                ));
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let inserted = sqlx::query(
                    r#"
                    INSERT INTO job_approvals (
                        id,
                        status,
                        job_id,
                        command_type,
                        selector_expression,
                        target_client_ids,
                        target_count,
                        privileged,
                        destructive,
                        force_unprivileged,
                        max_timeout_secs,
                        payload_hash,
                        request_fingerprint,
                        requester_id,
                        requester_username,
                        requester_role,
                        request_reason,
                        risk,
                        job_request
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                        $11, $12, $13, $14, $15, $16, $17, $18, $19
                    )
                    ON CONFLICT (id) DO NOTHING
                    "#,
                )
                .bind(approval.id)
                .bind(&approval.status)
                .bind(approval.job_id)
                .bind(&approval.command_type)
                .bind(&approval.selector_expression)
                .bind(&approval.target_client_ids)
                .bind(approval.target_count as i32)
                .bind(approval.privileged)
                .bind(approval.destructive)
                .bind(approval.force_unprivileged)
                .bind(approval.max_timeout_secs as i64)
                .bind(&approval.payload_hash)
                .bind(&approval.request_fingerprint)
                .bind(approval.requester_id)
                .bind(&approval.requester_username)
                .bind(&approval.requester_role)
                .bind(&approval.request_reason)
                .bind(&approval.risk)
                .bind(sqlx::types::Json(request))
                .execute(&mut *tx)
                .await?;
                if inserted.rows_affected() == 0 {
                    bail!("job_approval_id_reused");
                }
                let audit = job_approval_audit(&approval, "job.approval_requested", operator);
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(audit.id)
                .bind(audit.actor_id)
                .bind(&audit.action)
                .bind(&audit.target)
                .bind(&audit.command_hash)
                .bind(audit.metadata)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        Ok(approval)
    }

    pub(crate) async fn decide_job_approval(
        &self,
        approval_id: Uuid,
        status: &str,
        operator: &AuthContext,
        reason: Option<&str>,
    ) -> Result<JobApprovalView> {
        if !matches!(status, "approved" | "rejected") {
            bail!("job_approval_decision_invalid");
        }
        match self {
            Self::Memory(memory) => {
                let mut approvals = memory.job_approvals.write().await;
                let approval = approvals
                    .iter_mut()
                    .find(|approval| approval.id == approval_id)
                    .ok_or_else(|| anyhow::anyhow!("job_approval_not_found"))?;
                if approval.status != "pending" {
                    if approval.status == status {
                        return Ok(approval.clone());
                    }
                    bail!("job_approval_not_pending");
                }
                approval.status = status.to_string();
                approval.decision_by = job_actor_id(operator);
                approval.decision_username = Some(operator.operator.username.clone());
                approval.decision_reason = reason.map(str::to_string);
                approval.decided_at = Some(unix_now().to_string());
                let updated = approval.clone();
                drop(approvals);
                memory.audits.write().await.push(job_approval_audit(
                    &updated,
                    if status == "approved" {
                        "job.approval_approved"
                    } else {
                        "job.approval_rejected"
                    },
                    operator,
                ));
                Ok(updated)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let updated = sqlx::query(
                    r#"
                    UPDATE job_approvals
                    SET
                        status = $2,
                        decision_by = $3,
                        decision_username = $4,
                        decision_reason = $5,
                        decided_at = now(),
                        updated_at = now()
                    WHERE id = $1 AND status = 'pending'
                    RETURNING
                        id,
                        status,
                        job_id,
                        command_type,
                        selector_expression,
                        target_client_ids,
                        target_count,
                        privileged,
                        destructive,
                        force_unprivileged,
                        max_timeout_secs,
                        payload_hash,
                        request_fingerprint,
                        requester_id,
                        requester_username,
                        requester_role,
                        requested_at::text AS requested_at,
                        request_reason,
                        risk,
                        decision_by,
                        decision_username,
                        decision_reason,
                        decided_at::text AS decided_at
                    "#,
                )
                .bind(approval_id)
                .bind(status)
                .bind(job_actor_id(operator))
                .bind(&operator.operator.username)
                .bind(reason)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = updated else {
                    tx.rollback().await?;
                    let Some((existing, _)) = self.get_job_approval_request(approval_id).await?
                    else {
                        bail!("job_approval_not_found");
                    };
                    if existing.status == status {
                        return Ok(existing);
                    }
                    bail!("job_approval_not_pending");
                };
                let updated = job_approval_from_row(row)?;
                let audit = job_approval_audit(
                    &updated,
                    if status == "approved" {
                        "job.approval_approved"
                    } else {
                        "job.approval_rejected"
                    },
                    operator,
                );
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(audit.id)
                .bind(audit.actor_id)
                .bind(&audit.action)
                .bind(&audit.target)
                .bind(&audit.command_hash)
                .bind(audit.metadata)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(updated)
            }
        }
    }

    #[cfg(test)]
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
                        source_schedule_id,
                        causation_id,
                        schedule_lineage,
                        privileged,
                        status,
                        target_count,
                        payload_hash,
                        max_timeout_secs,
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
                            source_schedule_id: row.try_get("source_schedule_id")?,
                            causation_id: row.try_get("causation_id")?,
                            schedule_lineage: row.try_get("schedule_lineage")?,
                            privileged: row.try_get("privileged")?,
                            status: row.try_get("status")?,
                            target_count: row.try_get("target_count")?,
                            payload_hash: row.try_get("payload_hash")?,
                            max_timeout_secs: row.try_get::<i64, _>("max_timeout_secs")?.max(1)
                                as u64,
                            created_at: row.try_get("created_at")?,
                            completed_at: row.try_get("completed_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_dashboard_running_jobs(
        &self,
        client_ids: &[String],
        limit: i64,
    ) -> Result<Vec<JobHistoryView>> {
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        // Dashboard callers request one sentinel row beyond the visible page
        // so count saturation can be disclosed instead of shown as exact.
        let limit = limit.clamp(1, 201);
        match self {
            Self::Memory(memory) => {
                let client_ids = client_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<HashSet<_>>();
                let scoped_job_ids = memory
                    .job_targets
                    .read()
                    .await
                    .iter()
                    .filter(|target| client_ids.contains(target.client_id.as_str()))
                    .map(|target| target.job_id)
                    .collect::<HashSet<_>>();
                let mut jobs = memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .filter(|job| {
                        matches!(job.status.as_str(), JOB_STATUS_QUEUED | JOB_STATUS_RUNNING)
                            && scoped_job_ids.contains(&job.id)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                jobs.sort_by(|left, right| {
                    compare_timestamps_desc(&left.created_at, &right.created_at)
                        .then_with(|| right.id.cmp(&left.id))
                });
                jobs.truncate(limit as usize);
                Ok(jobs)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        j.id,
                        j.actor_id,
                        j.command_type,
                        j.source_schedule_id,
                        j.causation_id,
                        j.schedule_lineage,
                        j.privileged,
                        j.status,
                        j.target_count,
                        j.payload_hash,
                        j.max_timeout_secs,
                        j.created_at::text AS created_at,
                        j.completed_at::text AS completed_at
                    FROM jobs AS j
                    WHERE j.status IN ('queued', 'running')
                      AND EXISTS (
                          SELECT 1
                          FROM job_targets AS target
                          WHERE target.job_id = j.id
                            AND target.client_id = ANY($1::text[])
                      )
                    ORDER BY j.created_at DESC, j.id DESC
                    LIMIT $2
                    "#,
                )
                .bind(client_ids)
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(JobHistoryView {
                            id: row.try_get("id")?,
                            actor_id: row.try_get("actor_id")?,
                            command_type: row.try_get("command_type")?,
                            source_schedule_id: row.try_get("source_schedule_id")?,
                            causation_id: row.try_get("causation_id")?,
                            schedule_lineage: row.try_get("schedule_lineage")?,
                            privileged: row.try_get("privileged")?,
                            status: row.try_get("status")?,
                            target_count: row.try_get("target_count")?,
                            payload_hash: row.try_get("payload_hash")?,
                            max_timeout_secs: row.try_get::<i64, _>("max_timeout_secs")?.max(1)
                                as u64,
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
                        source_schedule_id,
                        causation_id,
                        schedule_lineage,
                        privileged,
                        status,
                        target_count,
                        payload_hash,
                        max_timeout_secs,
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
                        OR causation_id::text ILIKE $3 ESCAPE '\'
                        OR array_to_string(schedule_lineage, ' ') ILIKE $3 ESCAPE '\'
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
                            source_schedule_id: row.try_get("source_schedule_id")?,
                            causation_id: row.try_get("causation_id")?,
                            schedule_lineage: row.try_get("schedule_lineage")?,
                            privileged: row.try_get("privileged")?,
                            status: row.try_get("status")?,
                            target_count: row.try_get("target_count")?,
                            payload_hash: row.try_get("payload_hash")?,
                            max_timeout_secs: row.try_get::<i64, _>("max_timeout_secs")?.max(1)
                                as u64,
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

    pub(crate) async fn list_dashboard_job_targets(
        &self,
        job_ids: &[Uuid],
        client_ids: &[String],
    ) -> Result<Vec<JobTargetView>> {
        if job_ids.is_empty() || client_ids.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => {
                let job_ids = job_ids.iter().copied().collect::<HashSet<_>>();
                let client_ids = client_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<HashSet<_>>();
                Ok(memory
                    .job_targets
                    .read()
                    .await
                    .iter()
                    .filter(|target| {
                        job_ids.contains(&target.job_id)
                            && client_ids.contains(target.client_id.as_str())
                    })
                    .cloned()
                    .collect())
            }
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
                    WHERE job_id = ANY($1::uuid[])
                      AND client_id = ANY($2::text[])
                    ORDER BY job_id, client_id
                    "#,
                )
                .bind(job_ids)
                .bind(client_ids)
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
                rows.into_iter().map(audit_log_from_row).collect()
            }
        }
    }

    pub(crate) async fn get_audit_log(&self, audit_id: Uuid) -> Result<Option<AuditLogView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .audits
                .read()
                .await
                .iter()
                .find(|audit| audit.id == audit_id)
                .cloned()),
            Self::Postgres(pool) => {
                let row = sqlx::query(
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
                    WHERE id = $1
                    "#,
                )
                .bind(audit_id)
                .fetch_optional(pool)
                .await?;
                row.map(audit_log_from_row).transpose()
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
                rows.into_iter().map(audit_log_from_row).collect()
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
            "job_id": job_id,
            "selector_expression": request.selector_expression,
            "target_client_ids": &resolved_targets,
            "destructive": request.destructive,
            "confirmed": request.confirmed,
            "privileged": request.privileged,
            "force_unprivileged": request.force_unprivileged,
            "operator_id": operator.operator.id,
            "operator_username": operator.operator.username,
            "operator_role": operator.operator.role,
            "operator_session_id": operator.audit_session_id(),
            "status": status,
            "reason": reason,
            "result": status,
            "origin_kind": if job_actor_id(operator).is_some() { "operator_request" } else { "control_plane" },
            "component": "job-submission-controller",
        });
        let operation = request.job_command().ok();
        let actor_id = job_actor_id(operator);
        match self {
            Self::Memory(memory) => {
                let created_at = unix_now().to_string();
                memory.jobs.write().await.push(JobHistoryView {
                    id: job_id,
                    actor_id,
                    command_type: "api_job_request".to_string(),
                    source_schedule_id: None,
                    causation_id: None,
                    schedule_lineage: Vec::new(),
                    privileged: request.privileged,
                    status: status.to_string(),
                    target_count: resolved_targets.len() as i32,
                    payload_hash: command_hash.to_string(),
                    max_timeout_secs: request
                        .max_timeout_secs
                        .unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS)
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
                    actor_id,
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
                        max_timeout_secs, completed_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now())
                    "#,
                )
                .bind(job_id)
                .bind(actor_id)
                .bind("api_job_request")
                .bind(request.privileged)
                .bind(status)
                .bind(resolved_targets.len() as i32)
                .bind(command_hash)
                .bind(operation.clone().map(sqlx::types::Json))
                .bind(request_fingerprint)
                .bind(
                    request
                        .max_timeout_secs
                        .unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS) as i64,
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
                .bind(actor_id)
                .bind(format!("job.{status}"))
                .bind("api:/api/v1/jobs")
                .bind(command_hash)
                .bind(metadata)
                .execute(&mut *tx)
                .await?;
                crate::repository_operational_alerts::reconcile_postgres_job_event_sources_in_tx(
                    &mut tx, job_id,
                )
                .await?;
                record_job_created_webhook_event_in_tx(
                    &mut tx,
                    JobCreatedWebhookEvent {
                        job_id,
                        command_type: "api_job_request",
                        status,
                        privileged: request.privileged,
                        command_hash,
                        resolved_targets: &resolved_targets,
                        actor_id,
                        source_schedule_id: None,
                        operation: operation.as_ref(),
                    },
                )
                .await?;
                tx.commit().await?;
            }
        }
        if matches!(self, Self::Memory(_)) {
            self.reconcile_memory_job_event_sources(job_id).await?;
            self.record_job_created_webhook_event(JobCreatedWebhookEvent {
                job_id,
                command_type: "api_job_request",
                status,
                privileged: request.privileged,
                command_hash,
                resolved_targets: &resolved_targets,
                actor_id,
                source_schedule_id: None,
                operation: operation.as_ref(),
            })
            .await?;
        }
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
            None,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn record_dispatching_job_for_approval(
        &self,
        job_id: Uuid,
        request: &CreateJobRequest,
        command_hash: &str,
        request_fingerprint: &str,
        operator: &AuthContext,
        resolved_targets: &[String],
        approval_id: Uuid,
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
            Some(approval_id),
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
        approval_id: Option<Uuid>,
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
            approval_id,
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
            None,
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
            None,
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
        approval_id: Option<Uuid>,
    ) -> Result<Uuid> {
        let command_type = request.command_type_label().to_string();
        let actor_id = job_actor_id(operator);
        let mut metadata = json!({
            "job_id": job_id,
            "command_type": &command_type,
            "selector_expression": request.selector_expression,
            "target_client_ids": resolved_targets,
            "target_count": resolved_targets.len(),
            "destructive": request.destructive,
            "confirmed": request.confirmed,
            "privileged": request.privileged,
            "force_unprivileged": request.force_unprivileged,
            "rollout": request.rollout,
            "source_schedule_id": source_schedule_id,
            "approval_id": approval_id,
            "request_fingerprint": request_fingerprint,
            "result": "requested",
        });
        if actor_id.is_some() {
            metadata["origin_kind"] = json!("operator_request");
            metadata["component"] = json!("job-submission-controller");
            metadata["operator_id"] = json!(operator.operator.id);
            metadata["operator_username"] = json!(&operator.operator.username);
            metadata["operator_role"] = json!(&operator.operator.role);
            metadata["operator_session_id"] = json!(operator.audit_session_id());
        } else {
            metadata["origin_kind"] = json!("control_plane");
            metadata["component"] = json!(&operator.operator.username);
        }
        let operation = request
            .job_command()
            .map_err(|error| anyhow::anyhow!(error.code))?;
        let precompleted_by_client =
            precompleted_targets_by_client(resolved_targets, precompleted_targets)?;
        let required_live_targets = resolved_targets
            .iter()
            .filter(|client_id| !precompleted_by_client.contains_key(client_id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let capability_degraded_by_client = precompleted_targets
            .iter()
            .filter_map(|target| {
                capability_degraded_outcome_metadata(
                    &target.outcome,
                    &command_type,
                    &target.client_id,
                )
                .map(|metadata| (target.client_id.clone(), metadata))
            })
            .collect::<HashMap<_, _>>();
        let pending_runtime_config = pending_runtime_config_apply(&operation, resolved_targets)?;
        let prepared_rollout = prepare_job_rollout(request.rollout.as_ref(), resolved_targets)?;
        let mut finished_status = None::<String>;
        match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                require_visible_memory_clients(
                    memory,
                    &required_live_targets,
                    "job_target_no_longer_available",
                )
                .await?;
                let agents = memory.agents.read().await;
                anyhow::ensure!(
                    required_live_targets
                        .iter()
                        .all(|client_id| agents.iter().any(|agent| {
                            agent.id == *client_id
                                && !matches!(
                                    agent.status.as_str(),
                                    "suspended" | "revoked" | "deleted"
                                )
                        })),
                    "job_target_no_longer_available"
                );
                drop(agents);
                let created_at = unix_now().to_string();
                memory.jobs.write().await.push(JobHistoryView {
                    id: job_id,
                    actor_id,
                    command_type: command_type.clone(),
                    source_schedule_id,
                    causation_id: None,
                    schedule_lineage: Vec::new(),
                    privileged: request.privileged,
                    status: JOB_STATUS_QUEUED.to_string(),
                    target_count: resolved_targets.len() as i32,
                    payload_hash: command_hash.to_string(),
                    max_timeout_secs: request
                        .max_timeout_secs
                        .unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS)
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
                        .max_timeout_secs
                        .unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS)
                        .max(1),
                );
                if let Some(schedule_id) = source_schedule_id {
                    memory
                        .job_source_schedule_ids
                        .write()
                        .await
                        .insert(job_id, schedule_id);
                }
                if let Some(approval_id) = approval_id {
                    memory
                        .job_approval_ids
                        .write()
                        .await
                        .insert(job_id, approval_id);
                }
                if let Some(pending) = pending_runtime_config.as_ref().filter(|pending| {
                    !precompleted_by_client.contains_key(pending.client_id.as_str())
                }) {
                    queue_runtime_config_apply_memory_state(
                        memory,
                        &pending.client_id,
                        pending.version,
                        &pending.content_hash,
                        &pending.config,
                        job_id,
                        &pending.reason,
                    )
                    .await;
                }
                if let Some(rollout) = prepared_rollout.as_ref() {
                    memory
                        .job_rollouts
                        .write()
                        .await
                        .push(MemoryJobRolloutRecord {
                            job_id,
                            status: "running".to_string(),
                            policy: rollout.policy.clone(),
                            current_batch: 0,
                            total_batches: rollout.total_batches,
                            failure_baseline: 0,
                            pause_reason: None,
                            next_batch_unix: unix_now(),
                            created_at: created_at.clone(),
                            updated_at: created_at.clone(),
                            completed_at: None,
                        });
                    let mut assignments = memory.job_rollout_targets.write().await;
                    assignments.extend(rollout.target_batches.iter().map(
                        |(client_id, batch_index)| ((job_id, client_id.clone()), *batch_index),
                    ));
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
                if !capability_degraded_by_client.is_empty() {
                    memory.capability_degraded_job_targets.write().await.extend(
                        capability_degraded_by_client
                            .iter()
                            .map(|(client_id, metadata)| {
                                ((job_id, client_id.clone()), metadata.clone())
                            }),
                    );
                }
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id,
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
                            command_hash: Some(command_hash.to_string()),
                            metadata: json!({
                                "job_id": job_id,
                                "status": target.outcome.status,
                                "result": target.outcome.status,
                                "exit_code": target.outcome.exit_code,
                                "accepted": target.outcome.accepted,
                                "message": target.outcome.message,
                                "received_at": target.outcome.received_at,
                                "origin_kind": "control_plane",
                                "component": "job-dispatch-validation",
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
                require_visible_postgres_clients_in_tx(
                    &mut tx,
                    &required_live_targets,
                    "job_target_no_longer_available",
                )
                .await?;
                let unavailable_target_exists = sqlx::query_scalar::<_, bool>(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM visible_clients
                        WHERE id = ANY($1::text[])
                          AND status IN ('suspended', 'revoked', 'deleted')
                    )
                    "#,
                )
                .bind(&required_live_targets)
                .fetch_one(&mut *tx)
                .await?;
                anyhow::ensure!(!unavailable_target_exists, "job_target_no_longer_available");
                sqlx::query(
                    r#"
                    INSERT INTO jobs (
                        id, actor_id, command_type, privileged, status,
                        target_count, payload_hash, operation, source_schedule_id, request_fingerprint,
                        max_timeout_secs, approval_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                    "#,
                )
                .bind(job_id)
                .bind(actor_id)
                .bind(&command_type)
                .bind(request.privileged)
                .bind(JOB_STATUS_QUEUED)
                .bind(resolved_targets.len() as i32)
                .bind(command_hash)
                .bind(sqlx::types::Json(operation.clone()))
                .bind(source_schedule_id)
                .bind(request_fingerprint)
                .bind(request.max_timeout_secs.unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS) as i64)
                .bind(approval_id)
                .execute(&mut *tx)
                .await?;
                if let Some(pending) = pending_runtime_config.as_ref().filter(|pending| {
                    !precompleted_by_client.contains_key(pending.client_id.as_str())
                }) {
                    queue_runtime_config_apply_postgres_in_tx(
                        &mut tx,
                        &pending.client_id,
                        pending.version,
                        &pending.content_hash,
                        &pending.config,
                        job_id,
                        &pending.reason,
                    )
                    .await?;
                }
                for client_id in resolved_targets {
                    if let Some(outcome) = precompleted_by_client.get(client_id.as_str()) {
                        let capability_degraded =
                            capability_degraded_by_client.get(client_id.as_str());
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
                                result_received_at,
                                capability_degraded_reason,
                                capability_degraded_hint
                            )
                            VALUES (
                                $1, $2, $3, $4, $5, now(), now(),
                                COALESCE($6::timestamptz, now()), $7, $8
                            )
                            "#,
                        )
                        .bind(job_id)
                        .bind(client_id)
                        .bind(&outcome.status)
                        .bind(&outcome.message)
                        .bind(outcome.exit_code)
                        .bind(outcome.received_at.as_deref())
                        .bind(capability_degraded.map(|(reason, _)| reason))
                        .bind(capability_degraded.map(|(_, hint)| hint))
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
                        enqueue_target_terminal_event_in_tx(&mut tx, job_id, client_id, outcome)
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
                if let Some(rollout) = prepared_rollout.as_ref() {
                    sqlx::query(
                        r#"
                        INSERT INTO job_rollouts (
                            job_id,
                            status,
                            canary_client_ids,
                            batch_size,
                            max_failures,
                            pause_after_canary,
                            batch_delay_secs,
                            current_batch,
                            total_batches,
                            failure_baseline,
                            next_batch_at
                        )
                        VALUES ($1, 'running', $2, $3, $4, $5, $6, 0, $7, 0, now())
                        "#,
                    )
                    .bind(job_id)
                    .bind(&rollout.policy.canary_client_ids)
                    .bind(i32::from(rollout.policy.batch_size))
                    .bind(i32::from(rollout.policy.max_failures))
                    .bind(rollout.policy.pause_after_canary)
                    .bind(i64::from(rollout.policy.batch_delay_secs))
                    .bind(i32::from(rollout.total_batches))
                    .execute(&mut *tx)
                    .await?;
                    for (client_id, batch_index) in &rollout.target_batches {
                        sqlx::query(
                            r#"
                            INSERT INTO job_rollout_targets (job_id, client_id, batch_index)
                            VALUES ($1, $2, $3)
                            "#,
                        )
                        .bind(job_id)
                        .bind(client_id)
                        .bind(i32::from(*batch_index))
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
                .bind(actor_id)
                .bind("job.dispatch_requested")
                .bind("api:/api/v1/jobs")
                .bind(command_hash)
                .bind(metadata)
                .execute(&mut *tx)
                .await?;
                finished_status =
                    finish_job_in_tx_if_all_targets_terminal_and_enqueue_event(&mut tx, job_id)
                        .await?;
                crate::repository_operational_alerts::reconcile_postgres_job_event_sources_in_tx(
                    &mut tx, job_id,
                )
                .await?;
                record_job_created_webhook_event_in_tx(
                    &mut tx,
                    JobCreatedWebhookEvent {
                        job_id,
                        command_type: &command_type,
                        status: finished_status.as_deref().unwrap_or(JOB_STATUS_QUEUED),
                        privileged: request.privileged,
                        command_hash,
                        resolved_targets,
                        actor_id,
                        source_schedule_id,
                        operation: Some(&operation),
                    },
                )
                .await?;
                tx.commit().await?;
            }
        }
        if matches!(self, Self::Memory(_)) {
            self.record_job_created_webhook_event(JobCreatedWebhookEvent {
                job_id,
                command_type: &command_type,
                status: finished_status.as_deref().unwrap_or(JOB_STATUS_QUEUED),
                privileged: request.privileged,
                command_hash,
                resolved_targets,
                actor_id,
                source_schedule_id,
                operation: Some(&operation),
            })
            .await?;
            self.reconcile_memory_job_event_sources(job_id).await?;
            for target in precompleted_targets {
                self.record_runtime_config_apply_terminal_for_target_status(
                    job_id,
                    &target.client_id,
                    &target.outcome.status,
                    Some(target.outcome.message.as_str()),
                )
                .await?;
                self.record_job_target_webhook_event(job_id, &target.client_id, &target.outcome)
                    .await?;
            }
            if let Some(status) = finished_status {
                self.record_job_terminal_side_effects(job_id, &status)
                    .await?;
            }
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
                let _lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                let now_unix = unix_now();
                let now = now_unix.to_string();
                let dispatchable_clients = {
                    let hidden = memory.hidden_clients.read().await;
                    memory
                        .agents
                        .read()
                        .await
                        .iter()
                        .filter(|agent| {
                            !hidden.contains(&agent.id)
                                && !matches!(
                                    agent.status.as_str(),
                                    "suspended" | "revoked" | "deleted"
                                )
                        })
                        .map(|agent| agent.id.clone())
                        .collect::<HashSet<_>>()
                };
                let operations = memory.job_operations.read().await.clone();
                let timeouts = memory.job_timeouts.read().await.clone();
                let jobs = memory.jobs.read().await.clone();
                let approvals = memory.job_approvals.read().await.clone();
                let approval_ids = memory.job_approval_ids.read().await.clone();
                let request_fingerprints = memory.job_request_fingerprints.read().await.clone();
                let target_snapshot = memory.job_targets.read().await.clone();
                let rollouts = memory.job_rollouts.read().await.clone();
                let rollout_targets = memory.job_rollout_targets.read().await.clone();
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
                let mut selected = 0_usize;
                let mut invalid_operations = Vec::new();
                for target in targets.iter_mut().filter(|target| {
                    target.completed_at.is_none() && target.status == TARGET_STATUS_QUEUED
                }) {
                    if selected >= limit.clamp(1, 500) as usize {
                        break;
                    }
                    if !dispatchable_clients.contains(&target.client_id) {
                        continue;
                    }
                    let Some(job) = jobs.iter().find(|job| job.id == target.job_id) else {
                        continue;
                    };
                    if !job_approval_allows_dispatch(
                        &approvals,
                        approval_ids.get(&job.id).copied(),
                        job.id,
                        &job.payload_hash,
                        request_fingerprints.get(&job.id).map(String::as_str),
                    ) {
                        continue;
                    }
                    if let Some(rollout) = rollouts.iter().find(|rollout| {
                        rollout.job_id == target.job_id && rollout.completed_at.is_none()
                    }) {
                        let Some(batch_index) =
                            rollout_targets.get(&(target.job_id, target.client_id.clone()))
                        else {
                            continue;
                        };
                        if rollout.status != "running"
                            || rollout.next_batch_unix > now_unix
                            || *batch_index > rollout.current_batch
                        {
                            continue;
                        }
                    }
                    let Some(operation) = operations.get(&target.job_id).cloned() else {
                        selected += 1;
                        let decode_error =
                            "operation is missing from the in-memory job record".to_string();
                        let message = invalid_job_operation_message(
                            "stored job operation is invalid; target was not dispatched",
                            &decode_error,
                        );
                        let output_data = invalid_job_operation_status_output_value(
                            target.job_id,
                            &target.client_id,
                            TARGET_STATUS_FAILED,
                            &InvalidJobOperationEvidence {
                                phase: "dispatch_claim",
                                message: &message,
                                decode_error: &decode_error,
                                process_incarnation_id: None,
                            },
                        )
                        .to_string()
                        .into_bytes();
                        target.status = TARGET_STATUS_FAILED.to_string();
                        target.message = Some(message.clone());
                        target.exit_code = None;
                        target.started_at = Some(now.clone());
                        target.completed_at = Some(now.clone());
                        invalid_operations.push((
                            InvalidJobOperationTarget {
                                job_id: target.job_id,
                                client_id: target.client_id.clone(),
                                message,
                                decode_error,
                                process_incarnation_id: None,
                            },
                            output_data,
                        ));
                        continue;
                    };
                    let is_exclusive =
                        job_command_safety(&operation) == JobCommandSafety::Exclusive;
                    if (is_exclusive && active_clients.contains(&target.client_id))
                        || (!is_exclusive && active_exclusive_clients.contains(&target.client_id))
                    {
                        continue;
                    }
                    selected += 1;
                    let max_timeout_secs = timeouts
                        .get(&target.job_id)
                        .copied()
                        .unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS)
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
                        source_schedule_id: job.source_schedule_id,
                        causation_id: job.causation_id,
                        schedule_lineage: job.schedule_lineage.clone(),
                        max_timeout_secs,
                    });
                }
                let claimed_job_ids = claimed
                    .iter()
                    .map(|target| target.job_id)
                    .collect::<std::collections::HashSet<_>>();
                let invalid_job_ids = invalid_operations
                    .iter()
                    .map(|(target, _)| target.job_id)
                    .collect::<std::collections::HashSet<_>>();
                drop(targets);
                if !claimed_job_ids.is_empty() || !invalid_job_ids.is_empty() {
                    let target_snapshot = memory.job_targets.read().await.clone();
                    let mut jobs = memory.jobs.write().await;
                    for job in jobs.iter_mut().filter(|job| {
                        claimed_job_ids.contains(&job.id)
                            && job.completed_at.is_none()
                            && job.status == JOB_STATUS_QUEUED
                    }) {
                        job.status = JOB_STATUS_RUNNING.to_string();
                    }
                    for job_id in &invalid_job_ids {
                        let job_targets = target_snapshot
                            .iter()
                            .filter(|target| target.job_id == *job_id)
                            .cloned()
                            .collect::<Vec<_>>();
                        if job_targets.is_empty()
                            || job_targets
                                .iter()
                                .any(|target| target_status_is_active(&target.status))
                        {
                            continue;
                        }
                        if let Some(job) = jobs
                            .iter_mut()
                            .find(|job| job.id == *job_id && job.completed_at.is_none())
                        {
                            job.status =
                                aggregate_job_status_from_targets(&job_targets).to_string();
                            job.completed_at = Some(now.clone());
                        }
                    }
                }
                if !invalid_operations.is_empty() {
                    {
                        let mut outputs = memory.job_outputs.write().await;
                        for (target, data) in &invalid_operations {
                            let seq = outputs
                                .iter()
                                .filter(|output| {
                                    output.job_id == target.job_id
                                        && output.client_id == target.client_id
                                })
                                .map(|output| output.seq)
                                .max()
                                .unwrap_or(-1)
                                .saturating_add(1);
                            outputs.push(JobOutputView {
                                job_id: target.job_id,
                                client_id: target.client_id.clone(),
                                seq,
                                stream: "status".to_string(),
                                data_base64: base64::engine::general_purpose::STANDARD.encode(data),
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
                    {
                        let mut audits = memory.audits.write().await;
                        for (target, _) in &invalid_operations {
                            audits.push(AuditLogView {
                                id: Uuid::new_v4(),
                                actor_id: None,
                                action: "job.target_result".to_string(),
                                target: format!("client:{}", target.client_id),
                                command_hash: jobs
                                    .iter()
                                    .find(|job| job.id == target.job_id)
                                    .map(|job| job.payload_hash.clone()),
                                metadata: json!({
                                    "job_id": target.job_id,
                                    "status": TARGET_STATUS_FAILED,
                                    "result": TARGET_STATUS_FAILED,
                                    "message": target.message,
                                    "reason": INVALID_JOB_OPERATION_CODE,
                                    "phase": "dispatch_claim",
                                    "decode_error": target.decode_error,
                                    "origin_kind": "control_plane",
                                    "component": "job-dispatcher",
                                }),
                                created_at: now.clone(),
                            });
                        }
                    }
                    for (target, _) in &invalid_operations {
                        warn!(
                            job_id = %target.job_id,
                            client_id = %target.client_id,
                            error = %target.decode_error,
                            "terminalized dispatch target with invalid stored job operation"
                        );
                    }
                }
                for job_id in invalid_job_ids {
                    self.reconcile_memory_job_event_sources(job_id).await?;
                }
                Ok(claimed)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_agent_identity_lifecycle(&mut tx).await?;
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
                            job.causation_id,
                            job.schedule_lineage,
                            job.max_timeout_secs,
                            clients.process_incarnation_id AS client_process_incarnation_id
                        FROM job_targets target
                        JOIN jobs job ON job.id = target.job_id
                        JOIN visible_clients clients ON clients.id = target.client_id
                        WHERE target.completed_at IS NULL
                              AND target.cancel_requested_at IS NULL
                              AND target.status IN ('queued', 'dispatching')
                              AND job.completed_at IS NULL
                              AND job.status IN ('queued', 'running')
                              AND (
                                job.approval_id IS NULL
                                OR EXISTS (
                                  SELECT 1
                                  FROM job_approvals approval
                                  WHERE approval.id = job.approval_id
                                    AND approval.job_id = job.id
                                    AND approval.status = 'approved'
                                    AND approval.payload_hash = job.payload_hash
                                    AND approval.request_fingerprint = job.request_fingerprint
                                )
                              )
                              AND (
                                NOT EXISTS (
                                  SELECT 1
                                  FROM job_rollouts rollout
                                  WHERE rollout.job_id = target.job_id
                                )
                                OR EXISTS (
                                  SELECT 1
                                  FROM job_rollouts rollout
                                  JOIN job_rollout_targets rollout_target
                                    ON rollout_target.job_id = rollout.job_id
                                   AND rollout_target.client_id = target.client_id
                                  WHERE rollout.job_id = target.job_id
                                    AND rollout_target.batch_index <= rollout.current_batch
                                    AND (
                                      (
                                        target.status = 'queued'
                                        AND rollout.status = 'running'
                                        AND rollout.next_batch_at <= now()
                                      )
                                      OR (
                                        target.status = 'dispatching'
                                        AND rollout.status IN ('running', 'paused')
                                      )
                                    )
                                )
                              )
                              AND clients.hidden_at IS NULL
                              AND clients.status NOT IN ('suspended', 'revoked', 'deleted')
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
                                      AND (
                                        earlier_job.approval_id IS NULL
                                        OR EXISTS (
                                          SELECT 1
                                          FROM job_approvals earlier_approval
                                          WHERE earlier_approval.id = earlier_job.approval_id
                                            AND earlier_approval.job_id = earlier_job.id
                                            AND earlier_approval.status = 'approved'
                                            AND earlier_approval.payload_hash = earlier_job.payload_hash
                                            AND earlier_approval.request_fingerprint
                                              = earlier_job.request_fingerprint
                                        )
                                      )
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
                                        earlier_job.approval_id IS NULL
                                        OR EXISTS (
                                          SELECT 1
                                          FROM job_approvals earlier_approval
                                          WHERE earlier_approval.id = earlier_job.approval_id
                                            AND earlier_approval.job_id = earlier_job.id
                                            AND earlier_approval.status = 'approved'
                                            AND earlier_approval.payload_hash = earlier_job.payload_hash
                                            AND earlier_approval.request_fingerprint
                                              = earlier_job.request_fingerprint
                                        )
                                      )
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
                        FOR UPDATE OF target, job SKIP LOCKED
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
                                    + make_interval(secs => (due.max_timeout_secs + $5)::integer)
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
                            due.causation_id,
                            due.schedule_lineage,
                            due.max_timeout_secs
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
                        updated_targets.causation_id,
                        updated_targets.schedule_lineage,
                        updated_targets.max_timeout_secs,
                        (SELECT count(*) FROM promoted_jobs) AS promoted_jobs
                    FROM updated_targets
                    "#,
                )
                .bind(limit.clamp(1, 500))
                .bind(lease_secs.clamp(1, 7200) as i32)
                .bind(exclusive_operation_types())
                .bind(EXCLUSIVE_DISPATCH_ADVISORY_LOCK_CLASS)
                .bind(control_deadline_extra_secs.min(i32::MAX as u64) as i32)
                .fetch_all(&mut *tx)
                .await?;
                let mut claimed = Vec::with_capacity(rows.len());
                let mut invalid_operations = Vec::new();
                for row in rows {
                    let job_id: Uuid = row.try_get("job_id")?;
                    let client_id: String = row.try_get("client_id")?;
                    let process_incarnation_id: Uuid = row.try_get("process_incarnation_id")?;
                    let raw_operation: Option<sqlx::types::Json<Value>> =
                        row.try_get("operation")?;
                    let operation = match decode_persisted_job_operation(raw_operation) {
                        Ok(operation) => operation,
                        Err(decode_error) => {
                            let message = invalid_job_operation_message(
                                "stored job operation is invalid; target was not dispatched",
                                &decode_error,
                            );
                            sqlx::query(
                                r#"
                                UPDATE job_targets
                                SET last_dispatch_error = $3
                                WHERE job_id = $1
                                  AND client_id = $2
                                  AND completed_at IS NULL
                                  AND status = 'dispatching'
                                "#,
                            )
                            .bind(job_id)
                            .bind(&client_id)
                            .bind(format!("{INVALID_JOB_OPERATION_RETRY_MARKER} {message}"))
                            .execute(&mut *tx)
                            .await?;
                            invalid_operations.push(InvalidJobOperationTarget {
                                job_id,
                                client_id,
                                message,
                                decode_error,
                                process_incarnation_id: Some(process_incarnation_id),
                            });
                            continue;
                        }
                    };
                    let max_timeout_secs = row.try_get::<i64, _>("max_timeout_secs")?.max(1) as u64;
                    claimed.push(ClaimedJobTarget {
                        job_id,
                        client_id,
                        actor_id: row.try_get("actor_id")?,
                        command_type: row.try_get("command_type")?,
                        payload_hash: row.try_get("payload_hash")?,
                        process_incarnation_id,
                        operation,
                        source_schedule_id: row.try_get("source_schedule_id")?,
                        causation_id: row.try_get("causation_id")?,
                        schedule_lineage: row.try_get("schedule_lineage")?,
                        max_timeout_secs,
                    });
                }
                tx.commit().await?;
                for target in invalid_operations {
                    match terminalize_invalid_job_operation_target(
                        pool,
                        &target,
                        TARGET_STATUS_FAILED,
                        "dispatch_claim",
                        false,
                        None,
                    )
                    .await
                    {
                        Ok(true) => warn!(
                            job_id = %target.job_id,
                            client_id = %target.client_id,
                            error = %target.decode_error,
                            "terminalized dispatch target with invalid stored job operation"
                        ),
                        Ok(false) => warn!(
                            job_id = %target.job_id,
                            client_id = %target.client_id,
                            "invalid stored job operation target changed before terminalization"
                        ),
                        Err(error) => warn!(
                            job_id = %target.job_id,
                            client_id = %target.client_id,
                            decode_error = %target.decode_error,
                            error = %error,
                            "failed to terminalize dispatch target with invalid stored job operation"
                        ),
                    }
                }
                Ok(claimed)
            }
        }
    }

    /// Final durable check immediately before the gateway enqueue. Claim and
    /// client suspension share the identity lifecycle lock, but a claim is
    /// returned after that transaction commits; this check closes the gap
    /// between claim return and the central gateway's suspension fence.
    pub(crate) async fn claimed_job_target_dispatchable(
        &self,
        claimed: &ClaimedJobTarget,
    ) -> Result<bool> {
        const SUSPENSION_REASON: &str = "target_suspended";
        const SUSPENSION_MESSAGE: &str =
            "target_suspended: target skipped because VPS is suspended";
        match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                let suspended = memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .find(|agent| agent.id == claimed.client_id)
                    .is_some_and(|agent| agent.status == "suspended");
                if suspended {
                    self.skip_suspended_undelivered_targets_for_client(
                        &claimed.client_id,
                        SUSPENSION_REASON,
                        SUSPENSION_MESSAGE,
                    )
                    .await?;
                    return Ok(false);
                }
                Ok(memory.job_targets.read().await.iter().any(|target| {
                    target.job_id == claimed.job_id
                        && target.client_id == claimed.client_id
                        && target.status == TARGET_STATUS_DISPATCHING
                        && target.completed_at.is_none()
                        && target
                            .process_incarnation_id
                            .is_none_or(|process_incarnation_id| {
                                process_incarnation_id == claimed.process_incarnation_id
                            })
                }))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_agent_identity_lifecycle(&mut tx).await?;
                let row = sqlx::query(
                    r#"
                    SELECT client.status AS client_status,
                           client.hidden_at IS NOT NULL AS client_hidden,
                           client.process_incarnation_id AS client_process_incarnation_id,
                           target.status AS target_status,
                           target.completed_at,
                           target.process_incarnation_id AS target_process_incarnation_id
                    FROM job_targets target
                    LEFT JOIN clients client ON client.id=target.client_id
                    WHERE target.job_id=$1 AND target.client_id=$2
                    FOR UPDATE OF target
                    "#,
                )
                .bind(claimed.job_id)
                .bind(&claimed.client_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    tx.commit().await?;
                    return Ok(false);
                };
                let client_status: Option<String> = row.try_get("client_status")?;
                if client_status.as_deref() == Some("suspended") {
                    let job_ids = skip_suspended_undelivered_targets_for_client_in_tx(
                        &mut tx,
                        &claimed.client_id,
                        SUSPENSION_REASON,
                        SUSPENSION_MESSAGE,
                    )
                    .await?;
                    finish_jobs_in_tx_and_reconcile_event_sources(&mut tx, &job_ids).await?;
                    tx.commit().await?;
                    return Ok(false);
                }
                let dispatchable = !row
                    .try_get::<Option<bool>, _>("client_hidden")?
                    .unwrap_or(true)
                    && client_status.as_deref().is_some_and(|status| {
                        !matches!(status, "suspended" | "revoked" | "deleted")
                    })
                    && row.try_get::<Option<Uuid>, _>("client_process_incarnation_id")?
                        == Some(claimed.process_incarnation_id)
                    && row.try_get::<String, _>("target_status")? == TARGET_STATUS_DISPATCHING
                    && row
                        .try_get::<Option<chrono::DateTime<Utc>>, _>("completed_at")?
                        .is_none()
                    && row.try_get::<Option<Uuid>, _>("target_process_incarnation_id")?
                        == Some(claimed.process_incarnation_id);
                tx.commit().await?;
                Ok(dispatchable)
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
        self.skip_undelivered_targets_for_client(client_id, reason_code, message, false, &[])
            .await
    }

    pub(crate) async fn skip_suspended_undelivered_targets_for_client(
        &self,
        client_id: &str,
        reason_code: &str,
        message: &str,
    ) -> Result<Vec<Uuid>> {
        self.skip_suspended_undelivered_targets_for_client_except(
            client_id,
            reason_code,
            message,
            &[],
        )
        .await
    }

    pub(crate) async fn skip_suspended_undelivered_targets_for_client_except(
        &self,
        client_id: &str,
        reason_code: &str,
        message: &str,
        protected_enqueued_job_ids: &[Uuid],
    ) -> Result<Vec<Uuid>> {
        self.skip_undelivered_targets_for_client(
            client_id,
            reason_code,
            message,
            true,
            protected_enqueued_job_ids,
        )
        .await
    }

    async fn skip_undelivered_targets_for_client(
        &self,
        client_id: &str,
        reason_code: &str,
        message: &str,
        include_claimed_dispatching: bool,
        protected_enqueued_job_ids: &[Uuid],
    ) -> Result<Vec<Uuid>> {
        let job_ids = match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut changed = Vec::new();
                {
                    let mut targets = memory.job_targets.write().await;
                    for target in targets.iter_mut().filter(|target| {
                        target.client_id == client_id
                            && !protected_enqueued_job_ids.contains(&target.job_id)
                            && target.completed_at.is_none()
                            && ((target.status == TARGET_STATUS_QUEUED
                                && target.started_at.is_none()
                                && target.process_incarnation_id.is_none())
                                || (include_claimed_dispatching
                                    && target.status == TARGET_STATUS_DISPATCHING
                                    && target.started_at.is_some()))
                    }) {
                        target.status = TARGET_STATUS_SKIPPED.to_string();
                        target.message = Some(message.to_string());
                        target.exit_code = Some(0);
                        target.started_at = Some(now.clone());
                        target.completed_at = Some(now.clone());
                        target.deadline_at = None;
                        changed.push((target.job_id, target.client_id.clone()));
                    }
                }
                if !changed.is_empty() {
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
                if !changed.is_empty() {
                    let jobs = memory.jobs.read().await;
                    let mut audits = memory.audits.write().await;
                    for (job_id, target_client_id) in &changed {
                        audits.push(AuditLogView {
                            id: Uuid::new_v4(),
                            actor_id: None,
                            action: "job.target_result".to_string(),
                            target: format!("client:{target_client_id}"),
                            command_hash: jobs
                                .iter()
                                .find(|job| job.id == *job_id)
                                .map(|job| job.payload_hash.clone()),
                            metadata: json!({
                                "job_id": job_id,
                                "status": TARGET_STATUS_SKIPPED,
                                "result": TARGET_STATUS_SKIPPED,
                                "exit_code": 0,
                                "accepted": false,
                                "message": message,
                                "reason": reason_code,
                                "origin_kind": "control_plane",
                                "component": "client-lifecycle",
                            }),
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
                let job_ids = skip_undelivered_targets_for_client_in_tx(
                    &mut tx,
                    client_id,
                    reason_code,
                    message,
                    include_claimed_dispatching,
                    protected_enqueued_job_ids,
                )
                .await?;
                finish_jobs_in_tx_and_reconcile_event_sources(&mut tx, &job_ids).await?;
                tx.commit().await?;
                return Ok(job_ids);
            }
        };
        let mut unique_job_ids = job_ids;
        unique_job_ids.sort();
        unique_job_ids.dedup();
        for job_id in &unique_job_ids {
            self.refresh_job_status_from_targets(*job_id).await?;
            self.record_backup_request_terminal_for_target_status(
                *job_id,
                client_id,
                TARGET_STATUS_SKIPPED,
                None,
            )
            .await?;
            self.record_runtime_config_apply_terminal_for_target_status(
                *job_id,
                client_id,
                TARGET_STATUS_SKIPPED,
                Some(message),
            )
            .await?;
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
                if !changed.is_empty() {
                    let jobs = memory.jobs.read().await;
                    let mut audits = memory.audits.write().await;
                    for (job_id, target_client_id) in &changed {
                        audits.push(AuditLogView {
                            id: Uuid::new_v4(),
                            actor_id: None,
                            action: "job.target_result".to_string(),
                            target: format!("client:{target_client_id}"),
                            command_hash: jobs
                                .iter()
                                .find(|job| job.id == *job_id)
                                .map(|job| job.payload_hash.clone()),
                            metadata: json!({
                                "job_id": job_id,
                                "status": TARGET_STATUS_AGENT_LOST,
                                "result": TARGET_STATUS_AGENT_LOST,
                                "message": message,
                                "reason": code,
                                "expected_process_incarnation_id": expected_process_incarnation_id,
                                "current_process_incarnation_id": current_process_incarnation_id,
                                "origin_kind": "control_plane",
                                "component": "client-lifecycle",
                            }),
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
                finish_jobs_in_tx_and_reconcile_event_sources(&mut tx, &job_ids).await?;
                tx.commit().await?;
                return Ok(job_ids);
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
                let target_updated = if let Some(target) =
                    memory.job_targets.write().await.iter_mut().find(|target| {
                        target.job_id == job_id
                            && target.client_id == client_id
                            && target.completed_at.is_none()
                            && target_status_is_active(&target.status)
                    }) {
                    target.status = TARGET_STATUS_RUNNING.to_string();
                    target.message = Some(message.to_string());
                    target
                        .started_at
                        .get_or_insert_with(|| unix_now().to_string());
                    true
                } else {
                    false
                };
                if target_updated {
                    memory
                        .network_traffic_import_retry_not_before
                        .write()
                        .await
                        .remove(&(job_id, client_id.to_string()));
                    if let Some(job) = memory
                        .jobs
                        .write()
                        .await
                        .iter_mut()
                        .find(|job| job.id == job_id && job.completed_at.is_none())
                    {
                        job.status = "running".to_string();
                    }
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
                let command_hash = memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .find(|job| job.id == job_id)
                    .map(|job| job.payload_hash.clone());
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: None,
                    action: "job.target_result".to_string(),
                    target: format!("client:{client_id}"),
                    command_hash,
                    metadata: json!({
                        "job_id": job_id,
                        "status": TARGET_STATUS_AGENT_LOST,
                        "result": TARGET_STATUS_AGENT_LOST,
                        "message": message,
                        "expected_process_incarnation_id": expected_process_incarnation_id,
                        "current_process_incarnation_id": observed_process_incarnation_id,
                        "reason": "agent_process_restarted",
                        "origin_kind": "control_plane",
                        "component": "job-dispatcher",
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
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES (
                        $1,
                        NULL,
                        $2,
                        $3,
                        (SELECT payload_hash FROM jobs WHERE id = $5),
                        $4
                    )
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind("job.target_result")
                .bind(format!("client:{client_id}"))
                .bind(json!({
                    "job_id": job_id,
                    "status": TARGET_STATUS_AGENT_LOST,
                    "result": TARGET_STATUS_AGENT_LOST,
                    "message": message,
                    "expected_process_incarnation_id": expected_process_incarnation_id,
                    "target_process_incarnation_id": current_process_incarnation_id,
                    "current_process_incarnation_id": evidence_process_incarnation_id,
                    "reason": "agent_process_restarted",
                    "origin_kind": "control_plane",
                    "component": "job-dispatcher",
                }))
                .bind(job_id)
                .execute(&mut *tx)
                .await?;
                enqueue_target_terminal_event_in_tx(&mut tx, job_id, client_id, &outcome).await?;
                let completed_status =
                    finish_job_in_tx_if_all_targets_terminal_and_enqueue_event(&mut tx, job_id)
                        .await?;
                crate::repository_operational_alerts::reconcile_postgres_job_event_sources_in_tx(
                    &mut tx, job_id,
                )
                .await?;
                tx.commit().await?;
                return Ok(completed_status);
            }
        }
        let status = self.refresh_job_status_from_targets(job_id).await?;
        self.record_backup_request_terminal_for_target_status(
            job_id,
            client_id,
            TARGET_STATUS_AGENT_LOST,
            None,
        )
        .await?;
        self.record_runtime_config_apply_terminal_for_target_status(
            job_id,
            client_id,
            TARGET_STATUS_AGENT_LOST,
            Some(outcome.message.as_str()),
        )
        .await?;
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
                let jobs = memory.jobs.read().await.clone();
                let stored_outputs = memory.job_outputs.read().await;
                let awaiting_network_traffic_import = stored_outputs
                    .iter()
                    .filter(|output| {
                        output.done
                            && matches!(
                                operations.get(&output.job_id),
                                Some(JobCommand::NetworkTrafficImportVnstat { .. })
                            )
                            && job_output_sequence_contiguous_in_views(
                                &stored_outputs,
                                output.job_id,
                                &output.client_id,
                                output.seq,
                            )
                    })
                    .map(|output| (output.job_id, output.client_id.clone()))
                    .collect::<HashSet<_>>();
                drop(stored_outputs);
                let mut expired = Vec::new();
                let mut synthetic_outputs = Vec::new();
                let mut deadline_audit_evidence = Vec::new();
                let mut targets = memory.job_targets.write().await;
                for target in targets
                    .iter_mut()
                    .filter(|target| {
                        target.completed_at.is_none()
                            && matches!(
                                target.status.as_str(),
                                TARGET_STATUS_DISPATCHING | TARGET_STATUS_RUNNING
                            )
                            && !awaiting_network_traffic_import
                                .contains(&(target.job_id, target.client_id.clone()))
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
                    let max_timeout_secs = timeouts
                        .get(&target.job_id)
                        .copied()
                        .unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS)
                        .max(1)
                        .saturating_add(control_deadline_extra_secs);
                    if now.saturating_sub(started_at) < max_timeout_secs {
                        continue;
                    }
                    let (status, message, output_value, exit_code, invalid_decode_error) =
                        match operations.get(&target.job_id) {
                            None => {
                                let decode_error =
                                    "operation is missing from the in-memory job record"
                                        .to_string();
                                let message = invalid_job_operation_message(
                                    "control deadline elapsed and stored job operation is invalid",
                                    &decode_error,
                                );
                                (
                                    TARGET_STATUS_CONTROL_TIMEOUT,
                                    message.clone(),
                                    invalid_job_operation_status_output_value(
                                        target.job_id,
                                        &target.client_id,
                                        TARGET_STATUS_CONTROL_TIMEOUT,
                                        &InvalidJobOperationEvidence {
                                            phase: "control_deadline_expiry",
                                            message: &message,
                                            decode_error: &decode_error,
                                            process_incarnation_id: target.process_incarnation_id,
                                        },
                                    ),
                                    None,
                                    Some(decode_error),
                                )
                            }
                            Some(JobCommand::AgentUpdateActivate {
                                restart_agent: true,
                                ..
                            }) => {
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
                                    None,
                                )
                            }
                            Some(_) => {
                                let message =
                                    "control deadline elapsed before final command output"
                                        .to_string();
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
                                    None,
                                )
                            }
                        };
                    let reason = if invalid_decode_error.is_some() {
                        INVALID_JOB_OPERATION_CODE
                    } else if status == TARGET_STATUS_AGENT_LOST {
                        "agent_update_restart_missing_heartbeat"
                    } else {
                        "control_deadline_elapsed"
                    };
                    deadline_audit_evidence.push((
                        target.job_id,
                        target.client_id.clone(),
                        status.to_string(),
                        message.clone(),
                        reason,
                        invalid_decode_error,
                    ));
                    let output_data = output_value.to_string().into_bytes();
                    target.status = status.to_string();
                    target.message = Some(message.clone());
                    target.completed_at = Some(completed_at.clone());
                    synthetic_outputs.push((
                        target.job_id,
                        target.client_id.clone(),
                        output_data,
                        exit_code,
                    ));
                    expired.push(DeadlineExpiredJobTarget {
                        job_id: target.job_id,
                        client_id: target.client_id.clone(),
                        status: status.to_string(),
                    });
                }
                drop(targets);
                if !synthetic_outputs.is_empty() {
                    let mut outputs = memory.job_outputs.write().await;
                    for (job_id, client_id, data, exit_code) in synthetic_outputs {
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
                if !deadline_audit_evidence.is_empty() {
                    let mut audits = memory.audits.write().await;
                    for (job_id, client_id, status, message, reason, decode_error) in
                        &deadline_audit_evidence
                    {
                        audits.push(AuditLogView {
                            id: Uuid::new_v4(),
                            actor_id: None,
                            action: "job.target_result".to_string(),
                            target: format!("client:{client_id}"),
                            command_hash: jobs
                                .iter()
                                .find(|job| job.id == *job_id)
                                .map(|job| job.payload_hash.clone()),
                            metadata: json!({
                                "job_id": job_id,
                                "status": status,
                                "result": status,
                                "message": message,
                                "reason": reason,
                                "phase": "control_deadline_expiry",
                                "decode_error": decode_error,
                                "origin_kind": "control_plane",
                                "component": "job-deadline-reconciler",
                            }),
                            created_at: completed_at.clone(),
                        });
                    }
                    drop(audits);
                    for (job_id, client_id, _, _, _, decode_error) in deadline_audit_evidence {
                        if let Some(decode_error) = decode_error {
                            warn!(
                                %job_id,
                                client_id,
                                error = %decode_error,
                                "terminalized expired target with invalid stored job operation"
                            );
                        }
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
                      AND NOT (
                        job.command_type = 'network_traffic_import_vnstat'
                        AND EXISTS (
                          SELECT 1
                          FROM job_outputs final_output
                          WHERE final_output.job_id = target.job_id
                            AND final_output.client_id = target.client_id
                            AND final_output.done = TRUE
                            AND final_output.seq >= 0
                            AND (
                              SELECT COUNT(DISTINCT chunk.seq)
                              FROM job_outputs chunk
                              WHERE chunk.job_id = final_output.job_id
                                AND chunk.client_id = final_output.client_id
                                AND chunk.seq BETWEEN 0 AND final_output.seq
                            ) = final_output.seq::bigint + 1
                        )
                      )
                      AND NOT (
                        COALESCE(target.last_dispatch_error LIKE ($3 || '%'), false)
                        AND COALESCE(target.dispatch_lease_until > now(), false)
                      )
                      AND target.deadline_at IS NOT NULL
                      AND target.deadline_at <= now()
                      AND target.started_at IS NOT NULL
                      AND target.started_at + make_interval(secs => (job.max_timeout_secs + $2)::integer) <= now()
                    ORDER BY target.deadline_at ASC, target.job_id, target.client_id
                    LIMIT $1
                    FOR UPDATE SKIP LOCKED
                    "#,
                )
                .bind(limit.clamp(1, 500))
                .bind(control_deadline_extra_secs.min(i32::MAX as u64) as i32)
                .bind(INVALID_JOB_OPERATION_RETRY_MARKER)
                .fetch_all(&mut *tx)
                .await?;
                let mut expired = Vec::new();
                let mut invalid_operations = Vec::new();
                for row in rows {
                    let job_id: Uuid = row.try_get("job_id")?;
                    let client_id: String = row.try_get("client_id")?;
                    let process_incarnation_id: Option<Uuid> =
                        row.try_get("process_incarnation_id")?;
                    let last_dispatch_error: Option<String> = row.try_get("last_dispatch_error")?;
                    let raw_operation: Option<sqlx::types::Json<Value>> =
                        row.try_get("operation")?;
                    let operation = match decode_persisted_job_operation(raw_operation) {
                        Ok(operation) => operation,
                        Err(decode_error) => {
                            let message = invalid_job_operation_message(
                                "control deadline elapsed and stored job operation is invalid",
                                &decode_error,
                            );
                            sqlx::query(
                                r#"
                                UPDATE job_targets
                                SET
                                    dispatch_lease_until =
                                        now() + make_interval(secs => $3::integer),
                                    last_dispatch_error = $4
                                WHERE job_id = $1
                                  AND client_id = $2
                                  AND completed_at IS NULL
                                  AND status IN ('dispatching', 'running')
                                "#,
                            )
                            .bind(job_id)
                            .bind(&client_id)
                            .bind(INVALID_JOB_OPERATION_RETRY_DEFER_SECS)
                            .bind(format!("{INVALID_JOB_OPERATION_RETRY_MARKER} {message}"))
                            .execute(&mut *tx)
                            .await?;
                            invalid_operations.push(InvalidJobOperationTarget {
                                job_id,
                                client_id,
                                message,
                                decode_error,
                                process_incarnation_id,
                            });
                            continue;
                        }
                    };
                    let missing_update_heartbeat = matches!(
                        operation,
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
                          AND NOT (
                            job.command_type = 'network_traffic_import_vnstat'
                            AND EXISTS (
                              SELECT 1
                              FROM job_outputs final_output
                              WHERE final_output.job_id = target.job_id
                                AND final_output.client_id = target.client_id
                                AND final_output.done = TRUE
                                AND final_output.seq >= 0
                                AND (
                                  SELECT COUNT(DISTINCT chunk.seq)
                                  FROM job_outputs chunk
                                  WHERE chunk.job_id = final_output.job_id
                                    AND chunk.client_id = final_output.client_id
                                    AND chunk.seq BETWEEN 0 AND final_output.seq
                                ) = final_output.seq::bigint + 1
                            )
                          )
                          AND target.deadline_at IS NOT NULL
                          AND target.deadline_at <= now()
                          AND target.started_at IS NOT NULL
                          AND target.started_at + make_interval(secs => (job.max_timeout_secs + $5)::integer) <= now()
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
                    sqlx::query(
                        r#"
                        INSERT INTO audit_logs (
                            id, actor_id, action, target, command_hash, metadata
                        )
                        VALUES (
                            $1,
                            NULL,
                            $2,
                            $3,
                            (SELECT payload_hash FROM jobs WHERE id = $5),
                            $4
                        )
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind("job.target_result")
                    .bind(format!("client:{client_id}"))
                    .bind(json!({
                        "job_id": job_id,
                        "status": status,
                        "result": status,
                        "message": message,
                        "reason": if missing_update_heartbeat {
                            "agent_update_restart_missing_heartbeat"
                        } else {
                            "control_deadline_elapsed"
                        },
                        "process_incarnation_id": process_incarnation_id,
                        "origin_kind": "control_plane",
                        "component": "job-deadline-reconciler",
                    }))
                    .bind(job_id)
                    .execute(&mut *tx)
                    .await?;
                    let outcome = synthetic_terminal_outcome(status, &message, None, false);
                    enqueue_target_terminal_event_in_tx(&mut tx, job_id, &client_id, &outcome)
                        .await?;
                    expired.push(DeadlineExpiredJobTarget {
                        job_id,
                        client_id,
                        status: status.to_string(),
                    });
                }
                let changed_job_ids = expired
                    .iter()
                    .map(|target| target.job_id)
                    .collect::<Vec<_>>();
                finish_jobs_in_tx_and_reconcile_event_sources(&mut tx, &changed_job_ids).await?;
                tx.commit().await?;
                let control_deadline_extra_secs =
                    control_deadline_extra_secs.min(i32::MAX as u64) as i32;
                for target in invalid_operations {
                    match terminalize_invalid_job_operation_target(
                        pool,
                        &target,
                        TARGET_STATUS_CONTROL_TIMEOUT,
                        "control_deadline_expiry",
                        true,
                        Some(control_deadline_extra_secs),
                    )
                    .await
                    {
                        Ok(true) => {
                            warn!(
                                job_id = %target.job_id,
                                client_id = %target.client_id,
                                error = %target.decode_error,
                                "terminalized expired target with invalid stored job operation"
                            );
                            expired.push(DeadlineExpiredJobTarget {
                                job_id: target.job_id,
                                client_id: target.client_id,
                                status: TARGET_STATUS_CONTROL_TIMEOUT.to_string(),
                            });
                        }
                        Ok(false) => warn!(
                            job_id = %target.job_id,
                            client_id = %target.client_id,
                            "invalid stored job operation target no longer meets deadline terminalization conditions"
                        ),
                        Err(error) => warn!(
                            job_id = %target.job_id,
                            client_id = %target.client_id,
                            decode_error = %target.decode_error,
                            error = %error,
                            "failed to terminalize expired target with invalid stored job operation"
                        ),
                    }
                }
                Ok(expired)
            }
        }
    }

    pub(crate) async fn request_job_cancel(
        &self,
        job_id: Uuid,
        operator: &AuthContext,
        reason: Option<&str>,
    ) -> Result<JobCancelPlan> {
        let actor_id = operator.operator.id;
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
                let pending_canceled = canceled_targets.len();
                if let Some(rollout) = memory
                    .job_rollouts
                    .write()
                    .await
                    .iter_mut()
                    .find(|rollout| rollout.job_id == job_id && rollout.completed_at.is_none())
                {
                    rollout.status = "aborted".to_string();
                    rollout.pause_reason = Some(message.to_string());
                    rollout.updated_at = now.clone();
                    rollout.completed_at = Some(now.clone());
                }
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
                        "result": "requested",
                        "operator_id": operator.operator.id,
                        "operator_username": &operator.operator.username,
                        "operator_role": &operator.operator.role,
                        "operator_session_id": operator.audit_session_id(),
                        "origin_kind": "operator_request",
                        "component": "job-cancel-controller",
                    }),
                    created_at: now,
                });
                for client_id in &canceled_targets {
                    self.record_backup_request_terminal_for_target_status(
                        job_id,
                        client_id,
                        TARGET_STATUS_CANCELED,
                        None,
                    )
                    .await?;
                    self.record_runtime_config_apply_terminal_for_target_status(
                        job_id,
                        client_id,
                        TARGET_STATUS_CANCELED,
                        Some(message),
                    )
                    .await?;
                }
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
                    let outcome =
                        synthetic_terminal_outcome(TARGET_STATUS_CANCELED, message, None, false);
                    enqueue_target_terminal_event_in_tx(&mut tx, job_id, &client_id, &outcome)
                        .await?;
                }
                finish_job_in_tx_if_all_targets_terminal_and_enqueue_event(&mut tx, job_id).await?;
                sqlx::query(
                    r#"
                    UPDATE job_rollouts
                    SET
                        status = 'aborted',
                        pause_reason = $2,
                        updated_at = now(),
                        completed_at = now()
                    WHERE job_id = $1
                      AND completed_at IS NULL
                    "#,
                )
                .bind(job_id)
                .bind(message)
                .execute(&mut *tx)
                .await?;
                let cancel_targets = active_rows
                    .into_iter()
                    .map(|row| row.try_get("client_id").map_err(Into::into))
                    .collect::<Result<Vec<String>>>()?;
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
                    "result": "requested",
                    "operator_id": operator.operator.id,
                    "operator_username": &operator.operator.username,
                    "operator_role": &operator.operator.role,
                    "operator_session_id": operator.audit_session_id(),
                    "origin_kind": "operator_request",
                    "component": "job-cancel-controller",
                }))
                .execute(&mut *tx)
                .await?;
                crate::repository_operational_alerts::reconcile_postgres_job_event_sources_in_tx(
                    &mut tx, job_id,
                )
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
        accepted: bool,
        acked: bool,
        applied: bool,
        message: &str,
    ) -> Result<()> {
        let mut terminalized = false;
        match self {
            Self::Memory(memory) => {
                if applied {
                    let now = unix_now().to_string();
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
                        OR (
                          NOT $4
                          AND status IN ('control_timeout', 'canceled')
                        )
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
                    terminalized = true;
                    let outcome =
                        synthetic_terminal_outcome(TARGET_STATUS_CANCELED, message, None, accepted);
                    enqueue_target_terminal_event_in_tx(&mut tx, job_id, client_id, &outcome)
                        .await?;
                    finish_job_in_tx_if_all_targets_terminal_and_enqueue_event(&mut tx, job_id)
                        .await?;
                    crate::repository_operational_alerts::reconcile_postgres_job_event_sources_in_tx(
                        &mut tx, job_id,
                    )
                    .await?;
                }
                tx.commit().await?;
            }
        }
        if terminalized && matches!(self, Self::Memory(_)) {
            self.record_backup_request_terminal_for_target_status(
                job_id,
                client_id,
                TARGET_STATUS_CANCELED,
                None,
            )
            .await?;
            self.record_runtime_config_apply_terminal_for_target_status(
                job_id,
                client_id,
                TARGET_STATUS_CANCELED,
                Some(message),
            )
            .await?;
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
                    memory
                        .network_traffic_import_retry_not_before
                        .write()
                        .await
                        .remove(&(job_id, client_id.to_string()));
                    let command_hash = memory
                        .jobs
                        .read()
                        .await
                        .iter()
                        .find(|job| job.id == job_id)
                        .map(|job| job.payload_hash.clone());
                    memory.audits.write().await.push(AuditLogView {
                        id: Uuid::new_v4(),
                        actor_id: None,
                        action: "job.target_result".to_string(),
                        target: format!("client:{client_id}"),
                        command_hash,
                        metadata: json!({
                            "job_id": job_id,
                            "status": outcome.status,
                            "result": outcome.status,
                            "exit_code": outcome.exit_code,
                            "accepted": outcome.accepted,
                            "message": outcome.message,
                            "received_at": outcome.received_at,
                            "origin_kind": "control_plane",
                            "component": "job-dispatcher",
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
                    sqlx::query(
                        r#"
                        INSERT INTO audit_logs (
                            id, actor_id, action, target, command_hash, metadata
                        )
                        VALUES (
                            $1, NULL, $2, $3,
                            (SELECT payload_hash FROM jobs WHERE id = $4),
                            $5
                        )
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind("job.target_result")
                    .bind(format!("client:{client_id}"))
                    .bind(job_id)
                    .bind(json!({
                        "job_id": job_id,
                        "status": outcome.status,
                        "result": outcome.status,
                        "exit_code": outcome.exit_code,
                        "accepted": outcome.accepted,
                        "message": outcome.message,
                        "received_at": outcome.received_at,
                        "origin_kind": "control_plane",
                        "component": "job-dispatcher",
                    }))
                    .execute(&mut *tx)
                    .await?;
                }
                enqueue_target_terminal_event_in_tx(&mut tx, job_id, client_id, outcome).await?;
                insert_agent_update_lifecycle_for_stored_job_in_tx(
                    &mut tx, job_id, client_id, outcome,
                )
                .await?;
                finish_jobs_in_tx_and_reconcile_event_sources(&mut tx, &[job_id]).await?;
                tx.commit().await?;
                Ok(true)
            }
        }
    }

    pub(crate) async fn finish_job(&self, job_id: Uuid, status: &str) -> Result<bool> {
        let finished = match self {
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
                true
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE jobs
                    SET status = $2, completed_at = now()
                    WHERE id = $1
                      AND completed_at IS NULL
                    RETURNING id
                    "#,
                )
                .bind(job_id)
                .bind(status)
                .fetch_optional(&mut *tx)
                .await?;
                let finished = row.is_some();
                if finished {
                    enqueue_job_terminal_event_in_tx(&mut tx, job_id, status).await?;
                    crate::repository_operational_alerts::reconcile_postgres_job_event_sources_in_tx(
                        &mut tx,
                        job_id,
                    )
                    .await?;
                }
                tx.commit().await?;
                finished
            }
        };
        if finished && matches!(self, Self::Memory(_)) {
            self.record_job_terminal_side_effects(job_id, status)
                .await?;
        }
        Ok(finished)
    }

    pub(crate) async fn process_pending_job_terminal_events(
        &self,
        limit: i64,
        lease_secs: i64,
    ) -> Result<TerminalizationBatch> {
        let mut remaining = limit.clamp(1, 1000);
        let mut batch = TerminalizationBatch::default();
        loop {
            let events = match self {
                Self::Memory(_) => Vec::new(),
                Self::Postgres(pool) => {
                    let lease_id = Uuid::new_v4();
                    let rows = sqlx::query(
                        r#"
                    WITH claim AS (
                        SELECT event.id
                        FROM job_terminal_events event
                        WHERE (
                            (
                                event.processing_status IN ('queued', 'failed')
                                AND (
                                    event.next_attempt_at IS NULL
                                    OR event.next_attempt_at <= now()
                                )
                            )
                            OR (
                                event.processing_status = 'processing'
                                AND event.lease_until IS NOT NULL
                                AND event.lease_until <= now()
                            )
                        )
                        AND (
                            event.event_kind <> 'job_terminalized'
                            OR NOT EXISTS (
                                SELECT 1
                                FROM job_terminal_events target_event
                                WHERE target_event.job_id = event.job_id
                                  AND target_event.event_kind = 'target_terminalized'
                                  AND target_event.processing_status <> 'processed'
                            )
                        )
                        ORDER BY
                            event.created_at ASC,
                            CASE event.event_kind
                                WHEN 'target_terminalized' THEN 0
                                ELSE 1
                            END ASC,
                            event.id ASC
                        LIMIT $1
                        FOR UPDATE SKIP LOCKED
                    )
                    UPDATE job_terminal_events event
                    SET
                        processing_status = 'processing',
                        lease_id = $2,
                        lease_until = now() + ($3::bigint * interval '1 second'),
                        attempt_count = attempt_count + 1,
                        last_error = NULL
                    FROM claim
                    WHERE event.id = claim.id
                    RETURNING
                        event.id,
                        event.event_kind,
                        event.job_id,
                        event.client_id,
                        event.status,
                        event.outcome
                    "#,
                    )
                    .bind(remaining)
                    .bind(lease_id)
                    .bind(lease_secs.clamp(1, 3600))
                    .fetch_all(pool)
                    .await?;
                    rows.into_iter()
                        .map(|row| {
                            let outcome: Option<sqlx::types::Json<Value>> =
                                row.try_get("outcome")?;
                            Ok::<_, anyhow::Error>(ClaimedJobTerminalEvent {
                                id: row.try_get("id")?,
                                event_kind: row.try_get("event_kind")?,
                                job_id: row.try_get("job_id")?,
                                client_id: row.try_get("client_id")?,
                                status: row.try_get("status")?,
                                outcome: outcome.map(|value| value.0),
                            })
                        })
                        .collect::<Result<Vec<_>>>()?
                }
            };
            if events.is_empty() {
                break;
            }
            remaining = remaining.saturating_sub(events.len() as i64);
            let processed = self.process_claimed_job_terminal_events(events).await?;
            batch.extend(processed);
            if remaining <= 0 {
                break;
            }
        }
        Ok(batch)
    }

    async fn process_claimed_job_terminal_events(
        &self,
        events: Vec<ClaimedJobTerminalEvent>,
    ) -> Result<TerminalizationBatch> {
        let mut batch = TerminalizationBatch::default();
        for event in events {
            let result = self.process_claimed_job_terminal_event(&event).await;
            match result {
                Ok(Some(processed)) => {
                    if event.event_kind == "target_terminalized" {
                        self.mark_job_terminal_event_repository_side_effects_processed(event.id)
                            .await?;
                    } else {
                        self.mark_job_terminal_event_processed(event.id).await?;
                    }
                    batch.extend(processed);
                }
                Ok(None) => {
                    self.mark_job_terminal_event_processed(event.id).await?;
                }
                Err(error) => {
                    let message = error.to_string();
                    warn!(
                        %message,
                        event_id = %event.id,
                        job_id = %event.job_id,
                        event_kind = %event.event_kind,
                        "job terminal event processing failed"
                    );
                    self.mark_job_terminal_event_failed(event.id, &message)
                        .await?;
                }
            }
        }
        Ok(batch)
    }

    async fn process_claimed_job_terminal_event(
        &self,
        event: &ClaimedJobTerminalEvent,
    ) -> Result<Option<TerminalizationBatch>> {
        let mut batch = TerminalizationBatch::default();
        match event.event_kind.as_str() {
            "target_terminalized" => {
                let Some(client_id) = event.client_id.as_deref() else {
                    bail!("target terminal event missing client_id");
                };
                let outcome =
                    target_outcome_from_event_payload(&event.status, event.outcome.clone());
                let repository_side_effects_processed =
                    event.outcome.as_ref().is_some_and(|outcome| {
                        outcome.get("repository_side_effects_processed") == Some(&Value::Bool(true))
                    });
                if !repository_side_effects_processed {
                    self.repair_persisted_job_output_derivations_for_target(
                        event.job_id,
                        client_id,
                    )
                    .await?;
                    self.record_backup_request_terminal_for_target_status(
                        event.job_id,
                        client_id,
                        &event.status,
                        None,
                    )
                    .await?;
                    self.record_runtime_config_apply_terminal_for_target_status(
                        event.job_id,
                        client_id,
                        &event.status,
                        Some(outcome.message.as_str()),
                    )
                    .await?;
                    self.record_job_target_webhook_event(event.job_id, client_id, &outcome)
                        .await?;
                }
                batch.push_target(event.id, event.job_id, client_id, outcome);
                Ok(Some(batch))
            }
            "job_terminalized" => {
                self.record_job_terminal_side_effects(event.job_id, &event.status)
                    .await?;
                batch.push_job(event.job_id, event.status.clone());
                Ok(Some(batch))
            }
            _ => bail!("unknown job terminal event kind: {}", event.event_kind),
        }
    }

    async fn mark_job_terminal_event_repository_side_effects_processed(
        &self,
        event_id: Uuid,
    ) -> Result<()> {
        match self {
            Self::Memory(_) => {}
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE job_terminal_events
                    SET outcome = jsonb_set(
                        outcome,
                        '{repository_side_effects_processed}',
                        'true'::jsonb,
                        TRUE
                    )
                    WHERE id = $1
                      AND event_kind = 'target_terminalized'
                    "#,
                )
                .bind(event_id)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn mark_job_terminal_event_processed(&self, event_id: Uuid) -> Result<()> {
        match self {
            Self::Memory(_) => {}
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE job_terminal_events
                    SET
                        processing_status = 'processed',
                        processed_at = COALESCE(processed_at, now()),
                        lease_id = NULL,
                        lease_until = NULL,
                        next_attempt_at = NULL,
                        last_error = NULL
                    WHERE id = $1
                    "#,
                )
                .bind(event_id)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn mark_job_terminal_event_failed(
        &self,
        event_id: Uuid,
        error: &str,
    ) -> Result<()> {
        let error = error.chars().take(4096).collect::<String>();
        match self {
            Self::Memory(_) => {}
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE job_terminal_events
                    SET
                        processing_status = 'failed',
                        lease_id = NULL,
                        lease_until = NULL,
                        next_attempt_at = now() + (
                            LEAST(3600, GREATEST(5, attempt_count * attempt_count * 5))
                            * interval '1 second'
                        ),
                        last_error = $2
                    WHERE id = $1
                    "#,
                )
                .bind(event_id)
                .bind(error)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_job_terminal_side_effects(
        &self,
        job_id: Uuid,
        status: &str,
    ) -> Result<()> {
        let _memory_side_effect_guard = match self {
            Self::Memory(memory) => Some(memory.job_terminal_side_effects.lock().await),
            Self::Postgres(_) => None,
        };
        if matches!(self, Self::Memory(_)) {
            self.reconcile_memory_job_event_sources(job_id).await?;
        }
        self.record_job_status_webhook_event(job_id, status).await?;
        self.record_schedule_job_outcome(job_id, status).await?;
        Ok(())
    }

    pub(crate) async fn record_backup_request_terminal_for_target_status(
        &self,
        job_id: Uuid,
        client_id: &str,
        target_status: &str,
        operator: Option<&AuthContext>,
    ) -> Result<()> {
        let Some(backup_status) = backup_request_terminal_status_for_target(target_status) else {
            return Ok(());
        };
        // The linked backup request is the authoritative relation here. Avoid
        // decoding jobs.operation while consuming a terminal event: a corrupt
        // legacy operation must not poison terminal side-effect delivery.
        self.mark_open_backup_request_execution_terminal(
            job_id,
            client_id,
            backup_status,
            operator,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn active_agent_update_check_target_matches(
        &self,
        job_id: Uuid,
        client_id: &str,
        process_incarnation_id: Uuid,
    ) -> Result<bool> {
        match self {
            Self::Memory(memory) => {
                let operations = memory.job_operations.read().await;
                if !matches!(
                    operations.get(&job_id),
                    Some(JobCommand::AgentUpdateCheck { .. })
                ) {
                    return Ok(false);
                }
                Ok(memory.job_targets.read().await.iter().any(|target| {
                    target.job_id == job_id
                        && target.client_id == client_id
                        && target.completed_at.is_none()
                        && matches!(
                            target.status.as_str(),
                            TARGET_STATUS_DISPATCHING | TARGET_STATUS_RUNNING
                        )
                }))
            }
            Self::Postgres(pool) => {
                let matches: bool = sqlx::query_scalar(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM job_targets target
                        JOIN jobs job ON job.id = target.job_id
                        WHERE target.job_id = $1
                          AND target.client_id = $2
                          AND target.completed_at IS NULL
                          AND target.status IN ('dispatching', 'running')
                          AND target.process_incarnation_id = $3
                          AND COALESCE(job.operation ->> 'type', '') = 'agent_update_check'
                    )
                    "#,
                )
                .bind(job_id)
                .bind(client_id)
                .bind(process_incarnation_id)
                .fetch_one(pool)
                .await?;
                Ok(matches)
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
                        } else if let Some(retry_delay_secs) = schedule.retry_delay_secs {
                            schedule.next_run_at = Some(
                                (Utc::now() + Duration::seconds(retry_delay_secs.max(0)))
                                    .to_rfc3339(),
                            );
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
                          AND NOT EXISTS (
                              SELECT 1
                              FROM schedule_event_receipts receipt
                              WHERE receipt.job_id = $2
                                AND receipt.definition_revision <> schedules.definition_revision
                          )
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
                          AND NOT EXISTS (
                              SELECT 1
                              FROM schedule_event_receipts receipt
                              WHERE receipt.job_id = $2
                                AND receipt.definition_revision <> schedules.definition_revision
                          )
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
                          AND NOT EXISTS (
                              SELECT 1
                              FROM schedule_event_receipts receipt
                              WHERE receipt.job_id = $2
                                AND receipt.definition_revision <> schedules.definition_revision
                          )
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
                            "result": "failed",
                            "operator_id": summary.actor_id,
                            "operator_username": &summary.actor_username,
                            "operator_role": &summary.actor_role,
                            "origin_kind": "control_plane",
                            "component": "schedule-job-observer",
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
                    "result": "failed",
                    "operator_id": summary.actor_id,
                    "operator_username": &summary.actor_username,
                    "operator_role": &summary.actor_role,
                    "origin_kind": "control_plane",
                    "component": "schedule-job-observer",
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

    pub(crate) async fn record_job_created_webhook_event(
        &self,
        event: JobCreatedWebhookEvent<'_>,
    ) -> Result<()> {
        self.record_webhook_event(job_created_webhook_event_candidate(event))
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
                let actor = match job.actor_id {
                    Some(actor_id) => memory
                        .operators
                        .read()
                        .await
                        .iter()
                        .find(|operator| operator.id == actor_id)
                        .map(|operator| (operator.username.clone(), operator.role.clone())),
                    None => None,
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
                    actor_username: actor.as_ref().map(|actor| actor.0.clone()),
                    actor_role: actor.map(|actor| actor.1),
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
                        operator.username AS actor_username,
                        operator.role AS actor_role,
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
                    LEFT JOIN operators operator ON operator.id = job.actor_id
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
                    actor_username: row.try_get("actor_username")?,
                    actor_role: row.try_get("actor_role")?,
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
