use std::net::IpAddr;

use axum::{extract::State, http::HeaderMap, Json};
use serde::Serialize;
use vpsman_common::{
    CommandOutput, GatewayAgentHelloIngest, GatewayCommandOutputIngest,
    GatewaySessionLifecycleIngest, GatewayTelemetryIngest, GatewayTerminalOutputIngest,
    OutputStream,
};

use crate::{
    error::ApiError,
    model::{GatewayIdentityValidationRequest, GatewayIdentityValidationResponse, WsEvent},
    repository_job_outputs::JobOutputPersistConfig,
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
    validate_gateway_remote_ip(event.remote_ip.as_deref())?;
    state.repo.upsert_agent_hello(&event).await?;
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
    validate_gateway_remote_ip(event.remote_ip.as_deref())?;
    let client_id = event.telemetry.client_id.clone();
    let observed_unix = event.telemetry.metrics.observed_unix;
    let gateway_id = event.gateway_id.clone();
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
    let targets = state.repo.list_job_targets(event.job_id).await?;
    if !targets
        .iter()
        .any(|target| target.client_id == event.client_id)
    {
        return Err(ApiError::not_found("job_target_not_found"));
    }
    state
        .repo
        .record_job_output_chunk_with_config(
            event.job_id,
            &event.client_id,
            event.seq,
            &event.output,
            JobOutputPersistConfig {
                object_store: state.backup_object_store.as_ref(),
                artifact_min_bytes: state.job_output_artifact_min_bytes,
            },
        )
        .await?;
    state.publish(WsEvent::JobOutputRecorded {
        job_id: event.job_id,
        client_id: event.client_id.clone(),
        seq: event.seq,
        done: event.output.done,
    });
    if event.output.done {
        let outcome = target_outcome_from_done_output(event.job_id, &event.output);
        state
            .repo
            .update_job_target_result(event.job_id, &event.client_id, &outcome)
            .await?;
        if let Some((status, accepted_targets)) = state
            .repo
            .refresh_job_status_from_targets(event.job_id)
            .await?
        {
            if !matches!(status.as_str(), "queued" | "dispatching" | "running") {
                state.publish(WsEvent::JobFinished {
                    job_id: event.job_id,
                    accepted_targets,
                    status,
                });
            }
        }
    }
    Ok(Json(IngestResponse {
        accepted: true,
        message: "command output recorded".to_string(),
    }))
}

fn target_outcome_from_done_output(
    job_id: uuid::Uuid,
    output: &CommandOutput,
) -> TargetDispatchOutcome {
    let timed_out = crate::routes_jobs::output_indicates_timeout(output);
    let exit_code = output.exit_code;
    let status = if timed_out {
        "timed_out"
    } else if exit_code.unwrap_or(0) == 0 {
        "completed"
    } else {
        "failed"
    };
    TargetDispatchOutcome {
        status: status.to_string(),
        exit_code,
        command_version: None,
        accepted: true,
        message: status_output_message(output).unwrap_or_else(|| status.to_string()),
        outputs: vec![CommandOutput {
            job_id,
            stream: output.stream,
            data: output.data.clone(),
            exit_code: output.exit_code,
            done: output.done,
        }],
    }
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
    if !targets
        .iter()
        .any(|target| target.client_id == event.client_id)
    {
        return Err(ApiError::not_found("job_target_not_found"));
    }
    let seq = state
        .repo
        .append_job_output_chunk_with_config(
            event.output.job_id,
            &event.client_id,
            &event.output.output,
            JobOutputPersistConfig {
                object_store: state.backup_object_store.as_ref(),
                artifact_min_bytes: state.job_output_artifact_min_bytes,
            },
        )
        .await?;
    state.publish(WsEvent::TerminalOutputRecorded {
        job_id: event.output.job_id,
        client_id: event.client_id.clone(),
        session_id: event.output.session_id,
        terminal_seq: event.output.terminal_seq,
        seq,
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
        || event.client_id.is_empty()
        || event.client_id.len() > 128
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
        || event.client_id.is_empty()
        || event.client_id.len() > 128
        || event.output.output.job_id != event.output.job_id
        || event.output.output_next_seq == 0
        || event
            .output
            .terminal_seq
            .is_some_and(|seq| seq == 0 || seq >= event.output.output_next_seq)
    {
        return Err(ApiError::bad_request("invalid_terminal_output_event"));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub(crate) struct IngestResponse {
    accepted: bool,
    message: String,
}
