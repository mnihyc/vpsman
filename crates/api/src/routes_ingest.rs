use axum::{extract::State, http::HeaderMap, Json};
use serde::Serialize;
use vpsman_common::{
    GatewayAgentHelloIngest, GatewayCommandOutputIngest, GatewaySessionLifecycleIngest,
    GatewayTelemetryIngest, GatewayTerminalOutputIngest,
};

use crate::{
    error::ApiError,
    model::{GatewayIdentityValidationRequest, GatewayIdentityValidationResponse, WsEvent},
    repository_job_outputs::JobOutputPersistConfig,
    state::AppState,
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
        client_id: event.client_id,
        seq: event.seq,
        done: event.output.done,
    });
    Ok(Json(IngestResponse {
        accepted: true,
        message: "command output recorded".to_string(),
    }))
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
