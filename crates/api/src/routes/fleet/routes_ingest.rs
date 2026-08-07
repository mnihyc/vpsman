use std::net::IpAddr;

use axum::{extract::State, http::HeaderMap, Json};
use chrono::{TimeZone, Utc};
use serde::Serialize;
use tracing::warn;
use vpsman_common::{
    is_terminal_command_type, AgentUpdateVerificationResult, CommandOutput,
    GatewayAgentHelloIngest, GatewayAgentUpdateVerificationIngest, GatewayCommandOutputIngest,
    GatewayRuntimeConfigReloadRequest, GatewaySessionLifecycleIngest, GatewayTelemetryIngest,
    GatewayTerminalOutputIngest, JobCommand, OutputStream,
    RoutingCostAdapterJobResult,
    RoutingCostAdapterOperation, MAX_RUNTIME_CONFIG_REASON_BYTES, MAX_TELEMETRY_DISKS,
    MAX_TELEMETRY_NETWORKS, MAX_TELEMETRY_PING_RESULTS, MAX_TELEMETRY_TUNNELS,
};
use vpsman_server_core::{
    target_status_is_active, TARGET_STATUS_AGENT_LOST, TARGET_STATUS_AGENT_TIMEOUT,
    TARGET_STATUS_CANCELED, TARGET_STATUS_COMPLETED, TARGET_STATUS_CONTROL_TIMEOUT,
    TARGET_STATUS_FAILED, TARGET_STATUS_REJECTED, TARGET_STATUS_RUNNING,
};

use crate::{
    backup_auto_artifacts::try_auto_record_backup_artifact,
    error::ApiError,
    job_traffic_import::{
        apply_network_traffic_import_if_ready, NetworkTrafficImportApply,
    },
    model::{
        AuthContext, GatewayIdentityValidationRequest, GatewayIdentityValidationResponse, WsEvent,
    },
    repository_job_outputs::{JobOutputPersistConfig, JobOutputWriteResult},
    runtime_config::request_runtime_config_reload_for_agent,
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

pub(crate) async fn verify_agent_update_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<GatewayAgentUpdateVerificationIngest>,
) -> Result<Json<AgentUpdateVerificationResult>, ApiError> {
    state.require_internal_gateway(&headers)?;
    validate_agent_update_verification_event(&event)?;
    let job_id = event.request.job_id;
    let reject = |message: &'static str| {
        Json(AgentUpdateVerificationResult {
            job_id,
            approved: false,
            message: message.to_string(),
        })
    };
    if !state
        .repo
        .active_gateway_session_matches(
            &event.gateway_id,
            &event.client_id,
            event.gateway_session_id,
            event.process_incarnation_id,
        )
        .await?
    {
        return Ok(reject("gateway_session_not_active"));
    }
    if !state
        .repo
        .active_agent_update_check_target_matches(
            event.request.job_id,
            &event.client_id,
            event.process_incarnation_id,
        )
        .await?
    {
        return Ok(reject("agent_update_check_target_not_active"));
    }
    if !state.require_registered_agent_updates() {
        return Ok(Json(AgentUpdateVerificationResult {
            job_id,
            approved: true,
            message: "registered agent update verification not required".to_string(),
        }));
    }
    let approved = state
        .repo
        .agent_update_release_exists_for_artifact(&event.request.sha256_hex)
        .await?;
    Ok(Json(AgentUpdateVerificationResult {
        job_id,
        approved,
        message: if approved {
            "registered agent update artifact accepted".to_string()
        } else {
            "registered agent update artifact missing".to_string()
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
    if let Err(error) = state.process_job_terminal_events(500).await {
        warn!(
            ?error,
            "agent hello was accepted, but terminal event reconciliation was deferred to the durable dispatcher"
        );
    }
    Ok(Json(IngestResponse {
        accepted: true,
        message: "agent hello recorded".to_string(),
    }))
}

pub(crate) async fn request_runtime_config_reload(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(event): Json<GatewayRuntimeConfigReloadRequest>,
) -> Result<Json<IngestResponse>, ApiError> {
    state.require_internal_gateway(&headers)?;
    validate_gateway_runtime_config_reload(&event)?;
    if !state
        .repo
        .active_gateway_session_matches(
            &event.gateway_id,
            &event.request.client_id,
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
    let reconcile_scope =
        vpsman_common::RuntimeConfigReconcileScope::from_reload_request(&event.request);
    let sync_jobs = request_runtime_config_reload_for_agent(
        &state,
        &event.request.client_id,
        &event.request.current_content_hash,
        event.request.reason.trim(),
        reconcile_scope,
    )
    .await?;
    Ok(Json(IngestResponse {
        accepted: true,
        message: if sync_jobs.is_empty() {
            "runtime config already current".to_string()
        } else {
            format!("runtime config sync queued: {}", sync_jobs.len())
        },
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
    let received_unix = crate::unix_now();
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
    let recorded = state.repo.record_telemetry(&event).await?;
    if recorded {
        state.publish(WsEvent::TelemetryUpdated {
            client_id,
            observed_unix: received_unix,
            gateway_id,
        });
    }
    Ok(Json(IngestResponse {
        accepted: true,
        message: if recorded {
            "telemetry recorded".to_string()
        } else {
            "telemetry already recorded".to_string()
        },
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
        let mut outcome = target_outcome_from_done_output(event.job_id, &event.output, received_at);
        apply_network_traffic_import_outcome(
            &state,
            event.job_id,
            &event.client_id,
            event.seq,
            &event.output,
            &[],
            &mut outcome,
        )
        .await?;
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
                .process_job_terminal_events_or_publish_refresh(500, event.job_id, refreshed)
                .await?;
            if let Err(error) = try_record_agent_update_lifecycle_for_job_target(
                &state,
                event.job_id,
                &event.client_id,
                &outcome,
            )
            .await
            {
                warn!(
                    ?error,
                    job_id = %event.job_id,
                    client_id = %event.client_id,
                    "agent update lifecycle audit failed after command output ingest"
                );
            }
            if let Err(error) = record_network_routing_terminal_result(
                &state,
                event.job_id,
                &event.client_id,
                &outcome.status,
                Some(&event.output),
            )
            .await
            {
                warn!(
                    ?error,
                    job_id = %event.job_id,
                    client_id = %event.client_id,
                    "network routing result validation failed after command output ingest"
                );
            }
            if outcome.status == TARGET_STATUS_COMPLETED {
                if let Err(error) = try_auto_record_backup_artifact_for_job_target(
                    &state,
                    event.job_id,
                    &event.client_id,
                )
                .await
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
        finalize_contiguous_final_job_output_if_ready(
            &state,
            event.job_id,
            &event.client_id,
            persist_config,
        )
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

async fn finalize_contiguous_final_job_output_if_ready(
    state: &AppState,
    job_id: uuid::Uuid,
    client_id: &str,
    persist_config: JobOutputPersistConfig<'_>,
) -> Result<(), ApiError> {
    let Some(candidate) = state
        .repo
        .contiguous_final_job_output_candidate(job_id, client_id)
        .await?
    else {
        return Ok(());
    };
    let received_at = candidate
        .received_at
        .clone()
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let mut outcome = target_outcome_from_done_output(job_id, &candidate.output, received_at);
    apply_network_traffic_import_outcome(
        state,
        job_id,
        client_id,
        candidate.seq,
        &candidate.output,
        &[],
        &mut outcome,
    )
    .await?;
    let record_result = match state
        .repo
        .record_active_final_job_output_and_target_result_with_config(
            job_id,
            client_id,
            candidate.seq,
            &candidate.output,
            outcome.received_at.clone(),
            persist_config,
            &outcome,
        )
        .await
    {
        Ok(result) => result,
        Err(error) if error.to_string().contains("job_target_not_active") => {
            return Ok(());
        }
        Err(error) if error.to_string().contains("job_target_not_found") => {
            return Err(ApiError::not_found("job_target_not_found"));
        }
        Err(error) => return Err(ApiError::from(error)),
    };
    if record_result.write_result == JobOutputWriteResult::DuplicateConflict {
        return Err(ApiError::conflict("job_output_sequence_conflict"));
    }
    if record_result.target_terminalized {
        let refreshed = state.repo.refresh_job_status_from_targets(job_id).await?;
        state
            .process_job_terminal_events_or_publish_refresh(500, job_id, refreshed)
            .await?;
        if let Err(error) =
            try_record_agent_update_lifecycle_for_job_target(state, job_id, client_id, &outcome)
                .await
        {
            warn!(
                ?error,
                job_id = %job_id,
                client_id,
                "agent update lifecycle audit failed after deferred command output finalization"
            );
        }
        if let Err(error) = record_network_routing_terminal_result(
            state,
            job_id,
            client_id,
            &outcome.status,
            Some(&candidate.output),
        )
        .await
        {
            warn!(
                ?error,
                job_id = %job_id,
                client_id,
                "network routing result validation failed after deferred output finalization"
            );
        }
        if outcome.status == TARGET_STATUS_COMPLETED {
            if let Err(error) =
                try_auto_record_backup_artifact_for_job_target(state, job_id, client_id).await
            {
                warn!(
                    ?error,
                    job_id = %job_id,
                    client_id,
                    "backup artifact auto-record failed after deferred command output finalization"
                );
            }
        }
    }
    Ok(())
}

async fn apply_network_traffic_import_outcome(
    state: &AppState,
    job_id: uuid::Uuid,
    client_id: &str,
    final_seq: i32,
    final_output: &CommandOutput,
    inline_outputs: &[(i32, CommandOutput)],
    outcome: &mut TargetDispatchOutcome,
) -> Result<(), ApiError> {
    if outcome.status != TARGET_STATUS_COMPLETED {
        return Ok(());
    }
    let applied = apply_network_traffic_import_if_ready(
        state,
        job_id,
        client_id,
        final_seq,
        final_output,
        inline_outputs,
    )
    .await
    .map_err(|error| {
        ApiError::internal(
            "network_traffic_import_failed",
            "The vnStat traffic history could not be imported.",
            error,
        )
    })?;
    match applied {
        NetworkTrafficImportApply::NotApplicable | NetworkTrafficImportApply::Pending => {}
        NetworkTrafficImportApply::Applied(message) => outcome.message = message,
        NetworkTrafficImportApply::Invalid(message) => {
            outcome.status = TARGET_STATUS_FAILED.to_string();
            outcome.exit_code = Some(1);
            outcome.message = message;
        }
    }
    Ok(())
}

async fn try_record_agent_update_lifecycle_for_job_target(
    state: &AppState,
    job_id: uuid::Uuid,
    client_id: &str,
    outcome: &TargetDispatchOutcome,
) -> Result<(), ApiError> {
    let Some(context) = state.repo.get_job_completion_context(job_id).await? else {
        return Ok(());
    };
    match context.operation {
        JobCommand::AgentUpdateActivate {
            staged_sha256_hex, ..
        } if outcome.status == TARGET_STATUS_COMPLETED => {
            state
                .repo
                .record_agent_update_activation_completed(client_id, job_id, &staged_sha256_hex)
                .await?;
        }
        JobCommand::AgentUpdateActivate {
            staged_sha256_hex, ..
        } if agent_update_lifecycle_failure_status(&outcome.status) => {
            state
                .repo
                .record_agent_update_activation_failed(
                    client_id,
                    job_id,
                    &staged_sha256_hex,
                    &outcome.status,
                    outcome.exit_code,
                    &outcome.message,
                )
                .await?;
        }
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex,
        } if outcome.status == TARGET_STATUS_COMPLETED => {
            state
                .repo
                .record_agent_update_rollback_completed(
                    client_id,
                    job_id,
                    rollback_sha256_hex.as_deref(),
                )
                .await?;
        }
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex,
        } if agent_update_lifecycle_failure_status(&outcome.status) => {
            state
                .repo
                .record_agent_update_rollback_failed(
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
    Ok(())
}

fn agent_update_lifecycle_failure_status(status: &str) -> bool {
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

pub(crate) async fn record_network_routing_terminal_result(
    state: &AppState,
    job_id: uuid::Uuid,
    client_id: &str,
    outcome_status: &str,
    output: Option<&CommandOutput>,
) -> Result<(), ApiError> {
    let Some(context) = state.repo.get_job_completion_context(job_id).await? else {
        return Ok(());
    };
    let (plan_id, side, adapter, expected_operation, expected_cost, desired_cost) =
        match &context.operation {
            JobCommand::NetworkRoutingStatus {
                plan_id,
                side,
                adapter,
                ..
            } => (
                plan_id,
                *side,
                adapter,
                RoutingCostAdapterOperation::Status,
                None,
                None,
            ),
            JobCommand::NetworkRoutingApply {
                plan_id,
                side,
                adapter,
                expected_current_cost,
                desired_cost,
                ..
            } => (
                plan_id,
                *side,
                adapter,
                RoutingCostAdapterOperation::Apply,
                *expected_current_cost,
                Some(*desired_cost),
            ),
            _ => return Ok(()),
        };
    let plan_id = uuid::Uuid::parse_str(plan_id)
        .map_err(|_| ApiError::conflict("network_routing_result_plan_id_invalid"))?;
    if outcome_status != TARGET_STATUS_COMPLETED {
        state
            .repo
            .record_tunnel_plan_ospf_job_result(plan_id, side, job_id, None, false)
            .await?;
        return Ok(());
    }
    let Some(output) = output else {
        state
            .repo
            .record_tunnel_plan_ospf_job_result(plan_id, side, job_id, None, false)
            .await?;
        return Err(ApiError::conflict("network_routing_result_missing"));
    };
    let result = serde_json::from_slice::<RoutingCostAdapterJobResult>(&output.data)
        .map_err(|_| ApiError::conflict("network_routing_result_invalid"));
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            state
                .repo
                .record_tunnel_plan_ospf_job_result(plan_id, side, job_id, None, false)
                .await?;
            return Err(error);
        }
    };
    let valid = output.stream == OutputStream::Status
        && result.contract_version == vpsman_common::ROUTING_COST_ADAPTER_CONTRACT_VERSION
        && result.operation == expected_operation
        && result.plan_id == plan_id.to_string()
        && result.endpoint_side == side
        && result.client_id == client_id
        && result.adapter_definition_id == adapter.definition_id
        && result.adapter_definition_hash == adapter.definition_hash
        && result.current_cost > 0
        && result.previous_cost.is_none_or(|cost| cost > 0)
        && match expected_operation {
            RoutingCostAdapterOperation::Status => result.previous_cost.is_none(),
            RoutingCostAdapterOperation::Apply => {
                result.previous_cost.is_some()
                    && expected_cost.is_none_or(|expected| result.previous_cost == Some(expected))
            }
        }
        && desired_cost.is_none_or(|desired| result.current_cost == desired);
    state
        .repo
        .record_tunnel_plan_ospf_job_result(
            plan_id,
            side,
            job_id,
            valid.then_some(result.current_cost),
            valid,
        )
        .await?;
    if !valid {
        return Err(ApiError::conflict(
            "network_routing_result_contract_mismatch",
        ));
    }
    Ok(())
}

async fn try_auto_record_backup_artifact_for_job_target(
    state: &AppState,
    job_id: uuid::Uuid,
    client_id: &str,
) -> Result<(), ApiError> {
    let Some(context) = state.repo.get_job_completion_context(job_id).await? else {
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
        session_id: None,
    };
    try_auto_record_backup_artifact(
        state,
        &operator,
        client_id,
        &context.payload_hash,
        job_id,
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

pub(crate) fn status_output_message(output: &CommandOutput) -> Option<String> {
    if output.stream != OutputStream::Status {
        return None;
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.data) else {
        let message = String::from_utf8_lossy(&output.data)
            .chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        return (!message.is_empty()).then_some(message);
    };
    let kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if kind == Some("command_timeout") {
        let operation = value
            .get("operation_type")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("command")
            .replace('_', " ");
        let duration = value
            .get("max_timeout_secs")
            .and_then(serde_json::Value::as_u64)
            .filter(|seconds| *seconds > 0)
            .map(|seconds| format!(" after {seconds} seconds"))
            .unwrap_or_default();
        return Some(format!(
            "{operation} exceeded its agent execution timeout{duration} (command_timeout)"
        ));
    }
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
    if event.output.output.done {
        if let Some(job) = state.repo.get_job(event.output.job_id).await? {
            state.publish(WsEvent::JobFinished {
                job_id: job.id,
                status: job.status,
            });
        }
    }
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
        || ![
            event.hello.cpu_model.as_deref(),
            event.hello.kernel_release.as_deref(),
            event.hello.virtualization.as_deref(),
        ]
        .into_iter()
        .all(valid_optional_agent_host_fact)
    {
        return Err(ApiError::bad_request("invalid_gateway_agent_hello"));
    }
    validate_gateway_remote_ip(event.remote_ip.as_deref())?;
    validate_noise_public_key(&event.noise_public_key_hex)?;
    Ok(())
}

fn valid_optional_agent_host_fact(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        let value = value.trim();
        !value.is_empty() && value.len() <= 255 && !value.chars().any(char::is_control)
    })
}

fn validate_gateway_runtime_config_reload(
    event: &GatewayRuntimeConfigReloadRequest,
) -> Result<(), ApiError> {
    if event.gateway_id.trim().is_empty()
        || event.request.client_id.trim().is_empty()
        || event.request.client_id.len() > 128
        || event.gateway_session_id == uuid::Uuid::nil()
        || event.process_incarnation_id == uuid::Uuid::nil()
    {
        return Err(ApiError::bad_request(
            "invalid_gateway_runtime_config_reload",
        ));
    }
    let hash = event.request.current_content_hash.trim();
    if hash.len() != 64 || !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request(
            "invalid_gateway_runtime_config_reload",
        ));
    }
    let reason = event.request.reason.trim();
    if reason.is_empty()
        || reason.len() > MAX_RUNTIME_CONFIG_REASON_BYTES
        || reason.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(
            "invalid_gateway_runtime_config_reload",
        ));
    }
    Ok(())
}

fn validate_gateway_telemetry_event(event: &GatewayTelemetryIngest) -> Result<(), ApiError> {
    if event.gateway_id.is_empty()
        || event.gateway_id.len() > 128
        || event.gateway_session_id == uuid::Uuid::nil()
        || event.process_incarnation_id == uuid::Uuid::nil()
        || event.telemetry_seq == 0
        || event.telemetry_seq > i64::MAX as u64
        || event.telemetry.client_id.is_empty()
        || event.telemetry.client_id.len() > 128
        || !valid_agent_metrics(&event.telemetry.metrics)
    {
        return Err(ApiError::bad_request("invalid_gateway_telemetry_event"));
    }
    validate_gateway_remote_ip(event.remote_ip.as_deref())?;
    Ok(())
}

fn valid_agent_metrics(metrics: &vpsman_common::AgentMetrics) -> bool {
    if metrics.hostname.is_empty()
        || metrics.hostname.len() > 255
        || metrics.hostname.chars().any(char::is_control)
        || metrics.disks.len() > MAX_TELEMETRY_DISKS
        || metrics.networks.len() > MAX_TELEMETRY_NETWORKS
        || metrics.tunnels.len() > MAX_TELEMETRY_TUNNELS
        || metrics.ping_results.len() > MAX_TELEMETRY_PING_RESULTS
        || metrics
            .cpu
            .utilization_ratio
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        || ![
            metrics.cpu.load.one,
            metrics.cpu.load.five,
            metrics.cpu.load.fifteen,
        ]
        .into_iter()
        .all(|value| value.is_finite() && (0.0..=1_000_000.0).contains(&value))
        || metrics.memory.available_bytes > metrics.memory.total_bytes
        || metrics.memory.swap_total_bytes.is_some()
            != metrics.memory.swap_available_bytes.is_some()
        || metrics
            .memory
            .swap_total_bytes
            .zip(metrics.memory.swap_available_bytes)
            .is_some_and(|(total, available)| available > total)
    {
        return false;
    }
    if metrics.disks.iter().any(|disk| {
        disk.mountpoint.is_empty()
            || disk.mountpoint.len() > 4096
            || disk.mountpoint.chars().any(char::is_control)
            || disk.available_bytes > disk.total_bytes
    }) {
        return false;
    }
    if metrics.port_forwarding.as_ref().is_some_and(|snapshot| {
        snapshot.rules.len() > vpsman_common::MAX_PORT_FORWARD_RULES
            || snapshot
                .desired_hash
                .as_deref()
                .is_some_and(|value| !valid_sha256(value))
            || snapshot
                .observed_hash
                .as_deref()
                .is_some_and(|value| !valid_sha256(value))
            || snapshot
                .error_code
                .as_deref()
                .is_some_and(|value| value.len() > 128)
            || snapshot
                .error_message
                .as_deref()
                .is_some_and(|value| value.len() > 1024)
    }) {
        return false;
    }
    let mut interfaces = std::collections::HashSet::new();
    if metrics.networks.iter().any(|network| {
        network.interface.is_empty()
            || network.interface.len() > 64
            || network.interface.chars().any(char::is_control)
            || !interfaces.insert(network.interface.as_str())
    }) {
        return false;
    }
    let mut ping_results = std::collections::HashSet::new();
    if metrics.ping_results.iter().any(|result| {
        uuid::Uuid::parse_str(result.target_id.trim()).is_err()
            || result.generation == 0
            || result.checked_unix == 0
            || result.checked_unix > metrics.observed_unix.saturating_add(300)
            || metrics.observed_unix.saturating_sub(result.checked_unix) > 3_900
            || !result.values_are_coherent()
            || !result.loss_ratio.is_finite()
            || !(0.0..=1.0).contains(&result.loss_ratio)
            || result
                .latency_avg_ms
                .is_some_and(|value| !value.is_finite() || !(0.0..=3_600_000.0).contains(&value))
            || result
                .reason
                .as_ref()
                .is_some_and(|reason| reason.len() > 512 || reason.chars().any(char::is_control))
            || !ping_results.insert((result.target_id.as_str(), result.generation))
    }) {
        return false;
    }
    metrics.tunnels.iter().all(|tunnel| {
        !tunnel.interface.is_empty()
            && tunnel.interface.len() <= 64
            && !tunnel.interface.chars().any(char::is_control)
            && tunnel
                .latency_avg_ms
                .is_none_or(|value| value.is_finite() && value >= 0.0)
            && tunnel
                .packet_loss_ratio
                .is_none_or(|value| value.is_finite() && (0.0..=1.0).contains(&value))
    })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_agent_update_verification_event(
    event: &GatewayAgentUpdateVerificationIngest,
) -> Result<(), ApiError> {
    if event.gateway_id.is_empty()
        || event.gateway_id.len() > 128
        || event.gateway_session_id == uuid::Uuid::nil()
        || event.process_incarnation_id == uuid::Uuid::nil()
        || event.client_id.is_empty()
        || event.client_id.len() > 128
        || event.request.job_id == uuid::Uuid::nil()
        || !is_bounded_non_empty(&event.request.version_url, 4096)
        || !is_bounded_non_empty(&event.request.artifact_url, 4096)
        || !is_bounded_non_empty(&event.request.asset_name, 256)
        || event.request.sha256_hex.len() != 64
        || !event
            .request
            .sha256_hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ApiError::bad_request(
            "invalid_agent_update_verification_event",
        ));
    }
    Ok(())
}

fn is_bounded_non_empty(value: &str, max_len: usize) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= max_len
        && !value.bytes().any(|byte| byte.is_ascii_control())
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
#[path = "tests_routes_ingest.rs"]
mod tests;
