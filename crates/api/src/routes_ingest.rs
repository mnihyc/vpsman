use std::net::IpAddr;

use axum::{extract::State, http::HeaderMap, Json};
use chrono::{TimeZone, Utc};
use serde::Serialize;
use tracing::warn;
use vpsman_common::{
    is_terminal_command_type, CommandOutput, GatewayAgentHelloIngest, GatewayCommandOutputIngest,
    GatewaySessionLifecycleIngest, GatewayTelemetryIngest, GatewayTerminalOutputIngest, JobCommand,
    OutputStream,
};
use vpsman_server_core::{target_status_is_active, TARGET_STATUS_COMPLETED, TARGET_STATUS_RUNNING};

use crate::{
    backup_auto_artifacts::try_auto_record_backup_artifact,
    error::ApiError,
    model::{
        AuthContext, GatewayIdentityValidationRequest, GatewayIdentityValidationResponse, WsEvent,
    },
    repository_job_outputs::{JobOutputPersistConfig, JobOutputWriteResult},
    state::AppState,
    TargetDispatchOutcome,
};

pub(crate) async fn validate_agent_identity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<GatewayIdentityValidationRequest>,
) -> Result<Json<GatewayIdentityValidationResponse>, ApiError> {
    state.require_internal_gateway(&headers)?;
    let accepted = state
        .repo
        .validate_agent_public_key(&request.client_id, &request.noise_public_key_hex)
        .await?;
    Ok(Json(GatewayIdentityValidationResponse {
        accepted,
        message: if accepted {
            "client identity accepted".to_string()
        } else {
            "client identity rejected".to_string()
        },
    }))
}

pub(crate) async fn ingest_agent_hello(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<GatewayAgentHelloIngest>,
) -> Result<Json<IngestResponse>, ApiError> {
    state.require_internal_gateway(&headers)?;
    validate_gateway_agent_hello(&event)?;
    let accepted = state.repo.upsert_agent_hello(&event).await?;
    if !accepted {
        return Ok(Json(IngestResponse {
            accepted: false,
            message: "agent hello ignored".to_string(),
        }));
    }
    state.publish(WsEvent::AgentUpdated {
        client_id: event.hello.client_id,
        gateway_id: event.gateway_id,
    });
    Ok(Json(IngestResponse {
        accepted: true,
        message: "agent hello recorded".to_string(),
    }))
}

pub(crate) async fn ingest_gateway_session_started(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<GatewaySessionLifecycleIngest>,
) -> Result<Json<IngestResponse>, ApiError> {
    state.require_internal_gateway(&headers)?;
    validate_gateway_session_event(&event)?;
    state.repo.record_gateway_session_started(&event).await?;
    state.publish(WsEvent::AgentUpdated {
        client_id: event.client_id,
        gateway_id: event.gateway_id,
    });
    Ok(Json(IngestResponse {
        accepted: true,
        message: "gateway session start recorded".to_string(),
    }))
}

pub(crate) async fn ingest_gateway_session_ended(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<GatewaySessionLifecycleIngest>,
) -> Result<Json<IngestResponse>, ApiError> {
    state.require_internal_gateway(&headers)?;
    validate_gateway_session_event(&event)?;
    state.repo.record_gateway_session_ended(&event).await?;
    state.publish(WsEvent::AgentUpdated {
        client_id: event.client_id,
        gateway_id: event.gateway_id,
    });
    Ok(Json(IngestResponse {
        accepted: true,
        message: "gateway session end recorded".to_string(),
    }))
}

pub(crate) async fn ingest_telemetry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<GatewayTelemetryIngest>,
) -> Result<Json<IngestResponse>, ApiError> {
    state.require_internal_gateway(&headers)?;
    validate_gateway_telemetry_event(&event)?;
    let client_id = event.telemetry.client_id.clone();
    let observed_unix = event.telemetry.metrics.observed_unix;
    let gateway_id = event.gateway_id.clone();
    if !state
        .repo
        .active_gateway_session_matches(
            &event.gateway_id,
            &event.telemetry.client_id,
            event.gateway_session_id,
            event.process_incarnation_id,
        )
        .await?
    {
        return Ok(Json(IngestResponse {
            accepted: false,
            message: "gateway session not active".to_string(),
        }));
    }
    state.repo.record_telemetry(&event).await?;
    state.publish(WsEvent::TelemetryUpdated {
        client_id,
        observed_unix,
        gateway_id,
    });
    Ok(Json(IngestResponse {
        accepted: true,
        message: "telemetry recorded".to_string(),
    }))
}

pub(crate) async fn ingest_command_output(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<GatewayCommandOutputIngest>,
) -> Result<Json<IngestResponse>, ApiError> {
    state.require_internal_gateway(&headers)?;
    validate_command_output_event(&event)?;
    let Some(job) = state.repo.get_job(event.job_id).await? else {
        return Err(ApiError::not_found("job_not_found"));
    };
    if !event.payload_hash.eq_ignore_ascii_case(&job.payload_hash) {
        return Err(ApiError::conflict("job_output_payload_hash_mismatch"));
    }
    let targets = state.repo.list_job_targets(event.job_id).await?;
    let Some(target) = targets
        .iter()
        .find(|target| target.client_id == event.client_id)
    else {
        return Err(ApiError::not_found("job_target_not_found"));
    };
    ensure_command_output_gateway_session(&state, &event, target.process_incarnation_id).await?;
    let persist_config = JobOutputPersistConfig {
        object_store: state.backup_object_store.as_ref(),
        artifact_min_bytes: state.job_output_artifact_min_bytes(),
    };
    if target.completed_at.is_some() || !target_status_is_active(&target.status) {
        match state
            .repo
            .classify_existing_job_output_chunk_with_config(
                event.job_id,
                &event.client_id,
                event.seq,
                &event.output,
                persist_config,
            )
            .await?
        {
            Some(JobOutputWriteResult::DuplicateIdentical) => {
                return Ok(Json(IngestResponse {
                    accepted: true,
                    message: "duplicate command output already recorded".to_string(),
                }));
            }
            Some(JobOutputWriteResult::DuplicateConflict) => {
                state
                    .repo
                    .record_job_output_sequence_conflict_audit(
                        event.job_id,
                        &event.client_id,
                        event.seq,
                    )
                    .await?;
                return Err(ApiError::conflict("job_output_sequence_conflict"));
            }
            Some(JobOutputWriteResult::Inserted) => {
                return Err(ApiError::conflict("job_target_not_active"));
            }
            None => return Err(ApiError::conflict("job_target_not_active")),
        }
    }
    let received_at = command_output_received_at(event.received_unix);
    if event.output.done {
        let outcome = target_outcome_from_done_output(event.job_id, &event.output, received_at);
        let record_result = match state
            .repo
            .record_active_final_job_output_and_target_result_with_config(
                event.job_id,
                &event.client_id,
                event.seq,
                &event.output,
                outcome.received_at.clone(),
                persist_config,
                &outcome,
            )
            .await
        {
            Ok(result) => result,
            Err(error) if error.to_string().contains("job_target_not_active") => {
                return Err(ApiError::conflict("job_target_not_active"));
            }
            Err(error) if error.to_string().contains("job_target_not_found") => {
                return Err(ApiError::not_found("job_target_not_found"));
            }
            Err(error) => return Err(ApiError::from(error)),
        };
        if record_result.write_result == JobOutputWriteResult::DuplicateConflict {
            return Err(ApiError::conflict("job_output_sequence_conflict"));
        }
        state.publish(WsEvent::JobOutputRecorded {
            job_id: event.job_id,
            client_id: event.client_id.clone(),
            seq: event.seq,
            done: event.output.done,
        });
        if record_result.target_terminalized {
            let refreshed = state
                .repo
                .refresh_job_status_from_targets(event.job_id)
                .await?;
            state
                .publish_job_finished_after_refresh(event.job_id, refreshed)
                .await?;
            if outcome.status == TARGET_STATUS_COMPLETED {
                if let Err(error) =
                    try_auto_record_backup_artifact_from_ingest(&state, &event).await
                {
                    warn!(
                        ?error,
                        job_id = %event.job_id,
                        client_id = %event.client_id,
                        "backup artifact auto-record failed after command output ingest"
                    );
                }
            }
        }
    } else {
        let write_result = match state
            .repo
            .record_active_job_output_chunk_checked_with_config(
                event.job_id,
                &event.client_id,
                event.seq,
                &event.output,
                Some(received_at.clone()),
                persist_config,
            )
            .await
        {
            Ok(result) => result,
            Err(error) if error.to_string().contains("job_target_not_active") => {
                return Err(ApiError::conflict("job_target_not_active"));
            }
            Err(error) if error.to_string().contains("job_target_not_found") => {
                return Err(ApiError::not_found("job_target_not_found"));
            }
            Err(error) => return Err(ApiError::from(error)),
        };
        if write_result == JobOutputWriteResult::DuplicateConflict {
            return Err(ApiError::conflict("job_output_sequence_conflict"));
        }
        state.publish(WsEvent::JobOutputRecorded {
            job_id: event.job_id,
            client_id: event.client_id.clone(),
            seq: event.seq,
            done: event.output.done,
        });
        let message = status_output_message(&event.output)
            .unwrap_or_else(|| TARGET_STATUS_RUNNING.to_string());
        state
            .repo
            .mark_job_target_running(event.job_id, &event.client_id, &message)
            .await?;
    }
    if event.output.stream == OutputStream::Status && is_terminal_command_type(&job.command_type) {
        state
            .repo
            .record_terminal_command_replay_chunks(event.job_id, &event.client_id)
            .await?;
    }
    Ok(Json(IngestResponse {
        accepted: true,
        message: "command output recorded".to_string(),
    }))
}

async fn try_auto_record_backup_artifact_from_ingest(
    state: &AppState,
    event: &GatewayCommandOutputIngest,
) -> Result<(), ApiError> {
    let Some(context) = state.repo.get_job_completion_context(event.job_id).await? else {
        return Ok(());
    };
    if !matches!(context.operation, JobCommand::Backup { .. }) {
        return Ok(());
    }
    let Some(actor_id) = context.actor_id else {
        return Ok(());
    };
    if actor_id.is_nil() {
        return Ok(());
    }
    let Some(operator) = state.repo.operator_by_id(actor_id).await? else {
        return Ok(());
    };
    let operator = AuthContext {
        operator: operator.view(),
        session_id: uuid::Uuid::nil(),
    };
    try_auto_record_backup_artifact(
        state,
        &operator,
        &event.client_id,
        &context.payload_hash,
        event.job_id,
        &[],
    )
    .await
    .map_err(ApiError::from)?;
    Ok(())
}

fn target_outcome_from_done_output(
    job_id: uuid::Uuid,
    output: &CommandOutput,
    received_at: String,
) -> TargetDispatchOutcome {
    let outputs = vec![CommandOutput {
        job_id,
        stream: output.stream,
        data: output.data.clone(),
        exit_code: output.exit_code,
        done: output.done,
    }];
    let final_output = outputs.last();
    let (status, exit_code) = crate::routes_jobs::target_status_from_final_output(final_output);
    let message =
        crate::routes_jobs::target_message_for_status(&outputs, status, status, final_output);
    TargetDispatchOutcome {
        status: status.to_string(),
        exit_code,
        #[cfg(test)]
        command_version: None,
        accepted: true,
        message,
        received_at: Some(received_at),
        outputs,
    }
}

fn command_output_received_at(received_unix: Option<u64>) -> String {
    let now = Utc::now();
    let Some(received_unix) = received_unix else {
        return now.to_rfc3339();
    };
    if received_unix > i64::MAX as u64 {
        return now.to_rfc3339();
    }
    let Some(received_at) = Utc.timestamp_opt(received_unix as i64, 0).single() else {
        return now.to_rfc3339();
    };
    if received_at > now + chrono::Duration::seconds(300) {
        return now.to_rfc3339();
    }
    received_at.to_rfc3339()
}

fn status_output_message(output: &CommandOutput) -> Option<String> {
    if output.stream != OutputStream::Status {
        return None;
    }
    let value = serde_json::from_slice::<serde_json::Value>(&output.data).ok()?;
    let kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let primary = ["message", "error", "reason", "hint", "status"]
        .iter()
        .find_map(|field| {
            value
                .get(*field)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });
    match (kind, primary) {
        (Some(kind), Some(primary)) if kind != primary => Some(format!("{kind}: {primary}")),
        (Some(kind), _) => Some(kind.to_string()),
        (_, Some(primary)) => Some(primary.to_string()),
        _ => None,
    }
}

pub(crate) async fn ingest_terminal_output(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<GatewayTerminalOutputIngest>,
) -> Result<Json<IngestResponse>, ApiError> {
    state.require_internal_gateway(&headers)?;
    validate_terminal_output_event(&event)?;
    let targets = state.repo.list_job_targets(event.output.job_id).await?;
    let Some(target) = targets
        .iter()
        .find(|target| target.client_id == event.client_id)
    else {
        return Err(ApiError::not_found("job_target_not_found"));
    };
    ensure_terminal_output_gateway_session(&state, &event, target.process_incarnation_id).await?;
    match event.output.output.stream {
        OutputStream::Pty => match state
            .repo
            .record_terminal_stream_chunk(&event.client_id, &event.output)
            .await?
        {
            JobOutputWriteResult::DuplicateConflict => {
                return Err(ApiError::conflict("terminal_output_sequence_conflict"));
            }
            JobOutputWriteResult::Inserted | JobOutputWriteResult::DuplicateIdentical => {}
        },
        OutputStream::Status => {
            state
                .repo
                .record_terminal_stream_status(&event.client_id, &event.output)
                .await?;
        }
        OutputStream::Stdout | OutputStream::Stderr => {
            return Err(ApiError::bad_request("invalid_terminal_output_stream"));
        }
    }
    state.publish(WsEvent::TerminalOutputRecorded {
        job_id: event.output.job_id,
        client_id: event.client_id.clone(),
        session_id: event.output.session_id,
        terminal_seq: event.output.terminal_seq,
        done: event.output.output.done,
    });
    Ok(Json(IngestResponse {
        accepted: true,
        message: "terminal output recorded".to_string(),
    }))
}

fn validate_gateway_session_event(event: &GatewaySessionLifecycleIngest) -> Result<(), ApiError> {
    if event.gateway_id.is_empty()
        || event.gateway_id.len() > 128
        || event.client_id.is_empty()
        || event.client_id.len() > 128
        || event.session_id == uuid::Uuid::nil()
        || event
            .reason
            .as_ref()
            .is_some_and(|reason| reason.len() > 1024)
    {
        return Err(ApiError::bad_request("invalid_gateway_session_event"));
    }
    if let Some(key) = event.noise_public_key_hex.as_deref() {
        if key.len() != 64
            || hex::decode(key)
                .map(|bytes| bytes.len() != 32)
                .unwrap_or(true)
        {
            return Err(ApiError::bad_request("invalid_gateway_session_key"));
        }
    }
    validate_gateway_remote_ip(event.remote_ip.as_deref())?;
    Ok(())
}

fn validate_gateway_agent_hello(event: &GatewayAgentHelloIngest) -> Result<(), ApiError> {
    if event.gateway_id.is_empty()
        || event.gateway_id.len() > 128
        || event.gateway_session_id == uuid::Uuid::nil()
        || event.hello.client_id.is_empty()
        || event.hello.client_id.len() > 128
        || event.hello.process_incarnation_id == uuid::Uuid::nil()
    {
        return Err(ApiError::bad_request("invalid_gateway_agent_hello"));
    }
    validate_gateway_remote_ip(event.remote_ip.as_deref())?;
    if let Some(key) = event.noise_public_key_hex.as_deref() {
        validate_noise_public_key(key)?;
    }
    Ok(())
}

fn validate_gateway_telemetry_event(event: &GatewayTelemetryIngest) -> Result<(), ApiError> {
    if event.gateway_id.is_empty()
        || event.gateway_id.len() > 128
        || event.gateway_session_id == uuid::Uuid::nil()
        || event.process_incarnation_id == uuid::Uuid::nil()
        || event.telemetry.client_id.is_empty()
        || event.telemetry.client_id.len() > 128
    {
        return Err(ApiError::bad_request("invalid_gateway_telemetry_event"));
    }
    validate_gateway_remote_ip(event.remote_ip.as_deref())?;
    Ok(())
}

fn validate_gateway_remote_ip(remote_ip: Option<&str>) -> Result<(), ApiError> {
    let Some(remote_ip) = remote_ip else {
        return Ok(());
    };
    if remote_ip.len() > 64 || remote_ip.parse::<IpAddr>().is_err() {
        return Err(ApiError::bad_request("invalid_gateway_remote_ip"));
    }
    Ok(())
}

fn validate_command_output_event(event: &GatewayCommandOutputIngest) -> Result<(), ApiError> {
    if event.gateway_id.is_empty()
        || event.gateway_id.len() > 128
        || event.gateway_session_id == uuid::Uuid::nil()
        || event.process_incarnation_id == uuid::Uuid::nil()
        || event.client_id.is_empty()
        || event.client_id.len() > 128
        || event.payload_hash.len() != 64
        || !event
            .payload_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || event.seq < 0
        || event.output.job_id != event.job_id
    {
        return Err(ApiError::bad_request("invalid_command_output_event"));
    }
    Ok(())
}

fn validate_terminal_output_event(event: &GatewayTerminalOutputIngest) -> Result<(), ApiError> {
    if event.gateway_id.is_empty()
        || event.gateway_id.len() > 128
        || event.gateway_session_id == uuid::Uuid::nil()
        || event.process_incarnation_id == uuid::Uuid::nil()
        || event.client_id.is_empty()
        || event.client_id.len() > 128
        || event.output.output.job_id != event.output.job_id
        || event.output.output_next_seq == 0
        || event.output.output.data.len() > vpsman_common::MAX_TERMINAL_FLOW_WINDOW_BYTES as usize
        || event
            .output
            .terminal_seq
            .is_some_and(|seq| seq == 0 || seq >= event.output.output_next_seq)
    {
        return Err(ApiError::bad_request("invalid_terminal_output_event"));
    }
    match event.output.output.stream {
        OutputStream::Pty if event.output.terminal_seq.is_none() => {
            return Err(ApiError::bad_request("invalid_terminal_output_event"));
        }
        OutputStream::Pty | OutputStream::Status => {}
        OutputStream::Stdout | OutputStream::Stderr => {
            return Err(ApiError::bad_request("invalid_terminal_output_stream"));
        }
    }
    Ok(())
}

async fn ensure_active_gateway_session(
    state: &AppState,
    gateway_id: &str,
    client_id: &str,
    session_id: uuid::Uuid,
    process_incarnation_id: uuid::Uuid,
) -> Result<(), ApiError> {
    if state
        .repo
        .active_gateway_session_matches(gateway_id, client_id, session_id, process_incarnation_id)
        .await?
    {
        Ok(())
    } else {
        Err(ApiError::conflict("gateway_session_not_active"))
    }
}

async fn ensure_command_output_gateway_session(
    state: &AppState,
    event: &GatewayCommandOutputIngest,
    target_process_incarnation_id: Option<uuid::Uuid>,
) -> Result<(), ApiError> {
    ensure_output_gateway_session(
        state,
        &event.gateway_id,
        &event.client_id,
        event.gateway_session_id,
        event.process_incarnation_id,
        event.spooled_replay,
        target_process_incarnation_id,
    )
    .await
}

async fn ensure_terminal_output_gateway_session(
    state: &AppState,
    event: &GatewayTerminalOutputIngest,
    target_process_incarnation_id: Option<uuid::Uuid>,
) -> Result<(), ApiError> {
    ensure_output_gateway_session(
        state,
        &event.gateway_id,
        &event.client_id,
        event.gateway_session_id,
        event.process_incarnation_id,
        event.spooled_replay,
        target_process_incarnation_id,
    )
    .await
}

async fn ensure_output_gateway_session(
    state: &AppState,
    gateway_id: &str,
    client_id: &str,
    session_id: uuid::Uuid,
    process_incarnation_id: uuid::Uuid,
    spooled_replay: bool,
    target_process_incarnation_id: Option<uuid::Uuid>,
) -> Result<(), ApiError> {
    if !spooled_replay {
        return ensure_active_gateway_session(
            state,
            gateway_id,
            client_id,
            session_id,
            process_incarnation_id,
        )
        .await;
    }
    if target_process_incarnation_id != Some(process_incarnation_id) {
        return Err(ApiError::conflict("gateway_session_not_active"));
    }
    if state
        .repo
        .gateway_session_was_seen(gateway_id, client_id, session_id)
        .await?
    {
        Ok(())
    } else {
        Err(ApiError::conflict("gateway_session_not_active"))
    }
}

fn validate_noise_public_key(key: &str) -> Result<(), ApiError> {
    if key.len() == 64
        && hex::decode(key)
            .map(|bytes| bytes.len() == 32)
            .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(ApiError::bad_request("invalid_gateway_session_key"))
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct IngestResponse {
    accepted: bool,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_unsupported_command_output_maps_to_rejected_target_status() {
        let job_id = uuid::Uuid::new_v4();
        let output = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "unsupported_command_version",
                "status": "rejected",
                "command_type": "shell_argv",
            }))
            .unwrap(),
            exit_code: Some(78),
            done: true,
        };

        let outcome =
            target_outcome_from_done_output(job_id, &output, "2026-06-13T00:00:00Z".to_string());

        assert_eq!(outcome.status, vpsman_server_core::TARGET_STATUS_REJECTED);
        assert_eq!(outcome.exit_code, Some(78));
        assert_eq!(outcome.message, "unsupported_command_version: rejected");
    }

    #[test]
    fn ingest_done_output_without_exit_code_maps_to_failed() {
        let job_id = uuid::Uuid::new_v4();
        let output = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: Vec::new(),
            exit_code: None,
            done: true,
        };

        let outcome =
            target_outcome_from_done_output(job_id, &output, "2026-06-13T00:00:00Z".to_string());

        assert_eq!(outcome.status, vpsman_server_core::TARGET_STATUS_FAILED);
        assert_eq!(outcome.exit_code, None);
        assert_eq!(
            outcome.message,
            crate::routes_jobs::COMMAND_COMPLETED_WITHOUT_EXIT_CODE_MESSAGE
        );
    }
}
