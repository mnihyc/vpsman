use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    time::Duration,
};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::{
    error::ApiError,
    job_request::{fixed_target_selection, normalized_target_client_ids},
    lifecycle_outcome::{gateway_disconnect_outcome, terminal_reconciliation_outcome},
    model::{
        AgentSuspensionAction, AgentSuspensionMutationResponse, AgentView, AssignTagRequest,
        BulkAgentSuspensionOutcome, BulkAgentSuspensionRequest, BulkAgentSuspensionResponse,
        BulkDeleteAgentOutcome, BulkDeleteAgentsRequest, BulkDeleteAgentsResponse,
        BulkResolveManyRequest, BulkResolveManyResponse, BulkResolveRequest, BulkResolveResponse,
        BulkTagMutationRequest, CreateTagRequest, DeleteAgentRequest, DeleteAgentResponse,
        DeleteRuntimeConfigPatchGeneratorRequest, DeleteTagRequest, FleetSummary,
        GatewaySessionView, HistoryQuery, RenderRuntimeConfigPatchGeneratorRequest,
        RuntimeConfigApplyStateView, RuntimeConfigPatchGeneratorRenderView,
        RuntimeConfigPatchGeneratorView, SuspendAgentRequest, TagMutationResponse, TagOrderState,
        TagView, TelemetryNetworkRateQuery, TelemetryNetworkRateView, TelemetryRollupQuery,
        TelemetryRollupView, TelemetrySampleQuery, TelemetrySampleView, TelemetryTunnelQuery,
        TelemetryTunnelView, UnsuspendAgentRequest, UpdateAgentAliasRequest, UpdateTagOrderRequest,
        UpsertRuntimeConfigPatchGeneratorRequest, WsEvent,
    },
    privilege::{verify_privilege_intent, DbPrivilegeIntent},
    runtime_config::dispatch_runtime_config_for_clients,
    security::{
        operator_has_scope, require_vps_rule_selector_scope, SCOPE_CONFIG_READ, SCOPE_FLEET_READ,
    },
    selector_expression::parse_selector_expression,
    state::AppState,
    util::limit_or_default,
};

use vpsman_common::{
    GatewayClientSuspensionFenceClear, GatewayClientSuspensionFencePrepare,
    GatewayClientSuspensionFencePromote, GatewayPrivilegeVerification,
    GatewayPrivilegeVerificationBatchItem, GatewaySessionDisconnect,
    GATEWAY_CLIENT_SUSPENSION_FENCE_BATCH_MAX_ITEMS, GATEWAY_CONTROL_BATCH_MAX_ITEMS,
    MAX_RUNTIME_CONFIG_FIELD_BYTES,
};

const BULK_RESOLVE_MANY_ITEM_LIMIT: usize = 500;
const AGENT_SUSPENSION_FENCE_LEASE_SECS: u64 = 60;
const AGENT_SUSPENSION_DB_BUDGET_SECS: u64 = 30;
const AGENT_SUSPENSION_FENCE_CONTROL_ATTEMPT_SECS: u64 = 5;
const AGENT_DELETE_DB_BUDGET_SECS: u64 = 60;

const MAX_PATCH_GENERATOR_BODY_BYTES: usize = 16 * 1024;
const TELEMETRY_NETWORK_RATE_LIMIT_MAX: i64 = 5_000;

pub(crate) async fn fleet_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<FleetSummary>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(state.repo.fleet_summary().await.map_err(
        ApiError::internal_mapper(
            "fleet_summary_unavailable",
            "The fleet summary could not be loaded.",
        ),
    )?))
}

pub(crate) async fn list_agents(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AgentView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(state.repo.list_agents().await.map_err(
        ApiError::internal_mapper(
            "vps_inventory_unavailable",
            "The VPS inventory could not be loaded.",
        ),
    )?))
}

pub(crate) async fn update_agent_alias(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<UpdateAgentAliasRequest>,
) -> Result<Json<AgentView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_agent_alias(&request.display_name)?;
    validate_agent_alias_confirmation(&request)?;
    let agent = state
        .repo
        .update_agent_alias(&client_id, request.display_name.trim(), &operator)
        .await
        .map_err(agent_mutation_error)?;
    state.publish(WsEvent::AgentUpdated {
        client_id,
        gateway_id: "inventory_alias".to_string(),
    });
    Ok(Json(agent))
}

pub(crate) async fn delete_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<DeleteAgentRequest>,
) -> Result<Json<DeleteAgentResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    let response = mutate_delete_agents(
        &state,
        &operator,
        BulkDeleteAgentsRequest {
            items: vec![crate::model::BulkDeleteAgentItem {
                client_id,
                privilege_assertion: request.privilege_assertion,
            }],
            confirmed: request.confirmed,
            reason: request.reason,
        },
    )
    .await?;
    singleton_delete_agent_response(response)
}

pub(crate) async fn bulk_delete_agents(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkDeleteAgentsRequest>,
) -> Result<Json<BulkDeleteAgentsResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    Ok(Json(
        mutate_delete_agents(&state, &operator, request).await?,
    ))
}

async fn mutate_delete_agents(
    state: &AppState,
    operator: &crate::model::AuthContext,
    request: BulkDeleteAgentsRequest,
) -> Result<BulkDeleteAgentsResponse, ApiError> {
    validate_bulk_delete_agents_request(&request)?;
    if !state.gateway.privilege_configured() {
        return Err(ApiError::conflict("gateway_control_url_missing"));
    }

    let mut public_by_client = HashMap::new();
    let mut verification_items = Vec::new();
    let mut verification_client_ids = Vec::new();
    for item in &request.items {
        let Some(assertion) = item.privilege_assertion.clone() else {
            public_by_client.insert(
                item.client_id.clone(),
                rejected_delete_outcome(item.client_id.clone(), "privilege_assertion_required"),
            );
            continue;
        };
        let targets = vec![item.client_id.clone()];
        let intent =
            DbPrivilegeIntent::new("agent.delete", &item.client_id, None, &targets, true, None);
        let intent = serde_json::to_string(&intent).map_err(|error| {
            ApiError::internal(
                "privilege_intent_invalid",
                "The VPS deletion privilege intent could not be prepared.",
                error.into(),
            )
        })?;
        verification_client_ids.push(item.client_id.clone());
        verification_items.push(GatewayPrivilegeVerificationBatchItem {
            request_id: item.client_id.clone(),
            verification: GatewayPrivilegeVerification { intent, assertion },
        });
    }

    let mut approved_client_ids = Vec::new();
    if !verification_items.is_empty() {
        state.refresh_gateway_dispatch_timeouts();
        let verification = state
            .gateway
            .verify_privileges(verification_items)
            .await
            .map_err(bulk_delete_privilege_error)?;
        if verification.results.len() != verification_client_ids.len()
            || verification
                .results
                .iter()
                .zip(&verification_client_ids)
                .any(|(result, client_id)| &result.request_id != client_id)
        {
            return Err(ApiError::internal(
                "privilege_verification_result_invalid",
                "The gateway returned an invalid VPS deletion privilege result set.",
                anyhow::anyhow!("bulk privilege results did not preserve request order"),
            ));
        }
        for result in verification.results {
            if result.approved {
                approved_client_ids.push(result.request_id);
            } else {
                public_by_client.insert(
                    result.request_id.clone(),
                    rejected_delete_outcome(result.request_id, "privilege_verification_failed"),
                );
            }
        }
    }

    let repository_outcomes = if approved_client_ids.is_empty() {
        Vec::new()
    } else {
        tokio::time::timeout(
            Duration::from_secs(AGENT_DELETE_DB_BUDGET_SECS),
            state
                .repo
                .delete_agents(&approved_client_ids, request.reason.as_deref(), operator),
        )
        .await
        .map_err(|error| {
            ApiError::internal(
                "agent_delete_timeout",
                "The VPS deletions did not commit within their transaction budget.",
                error.into(),
            )
        })?
        .map_err(agent_mutation_error)?
    };

    let mut deleted_by_client = HashMap::new();
    let mut affected_client_ids = Vec::new();
    for outcome in repository_outcomes {
        match outcome {
            crate::repository_inventory::DeleteAgentRepositoryOutcome::Applied(result) => {
                affected_client_ids.push(result.client_id.clone());
                deleted_by_client.insert(result.client_id.clone(), result);
            }
            crate::repository_inventory::DeleteAgentRepositoryOutcome::Rejected {
                client_id,
                code,
            } => {
                public_by_client
                    .insert(client_id.clone(), rejected_delete_outcome(client_id, code));
            }
        }
    }

    if !affected_client_ids.is_empty() {
        let disconnect_items = affected_client_ids
            .iter()
            .map(|client_id| GatewaySessionDisconnect {
                client_id: client_id.clone(),
                reason: "vps_deleted".to_string(),
            })
            .collect::<Vec<_>>();
        state.refresh_gateway_dispatch_timeouts();
        let mut gateway_outcomes = HashMap::new();
        match state.gateway.disconnect_sessions(disconnect_items).await {
            Ok(batch)
                if batch.results.len() == affected_client_ids.len()
                    && batch
                        .results
                        .iter()
                        .zip(&affected_client_ids)
                        .all(|(result, client_id)| &result.client_id == client_id) =>
            {
                for result in batch.results {
                    let outcome = if result.accepted {
                        gateway_disconnect_outcome(Ok(()), &result.client_id, "VPS deletion")
                    } else {
                        gateway_disconnect_outcome(
                            Err(ApiError::conflict("gateway_session_disconnect_failed")),
                            &result.client_id,
                            "VPS deletion",
                        )
                    };
                    gateway_outcomes.insert(result.client_id, outcome);
                }
            }
            Ok(_) => {
                for client_id in &affected_client_ids {
                    gateway_outcomes.insert(
                        client_id.clone(),
                        gateway_disconnect_outcome(
                            Err(ApiError::conflict(
                                "gateway_session_disconnect_result_invalid",
                            )),
                            client_id,
                            "VPS deletion",
                        ),
                    );
                }
            }
            Err(error) => {
                tracing::warn!(%error, "bulk VPS deletion gateway disconnect failed");
                for client_id in &affected_client_ids {
                    gateway_outcomes.insert(
                        client_id.clone(),
                        gateway_disconnect_outcome(
                            Err(ApiError::conflict("gateway_session_disconnect_failed")),
                            client_id,
                            "VPS deletion",
                        ),
                    );
                }
            }
        }

        let deleted_set = affected_client_ids.iter().cloned().collect::<HashSet<_>>();
        let mut peers_by_deleted_client = HashMap::new();
        let mut surviving_peers = BTreeSet::new();
        for (client_id, deleted) in &deleted_by_client {
            let mut peers = peer_client_ids_for_deleted_agent(
                client_id,
                deleted.retired_tunnel_endpoint_pairs.clone(),
            );
            peers.retain(|peer_client_id| !deleted_set.contains(peer_client_id));
            surviving_peers.extend(peers.iter().cloned());
            peers_by_deleted_client.insert(client_id.clone(), peers);
        }
        let runtime_sync = dispatch_runtime_config_for_clients(
            state,
            operator,
            surviving_peers,
            "agent_deleted_tunnel_peer_cleanup",
        )
        .await;
        let runtime_sync_by_client = runtime_sync
            .into_iter()
            .map(|outcome| (outcome.client_id.clone(), outcome))
            .collect::<HashMap<_, _>>();
        crate::job_dispatcher::wake_job_terminal_event_consumer();
        let terminal_reconciliation = terminal_reconciliation_outcome(Ok(()), "VPS deletion");
        for client_id in &affected_client_ids {
            let deleted = deleted_by_client
                .remove(client_id)
                .ok_or_else(|| agent_delete_result_missing(client_id))?;
            let per_client_runtime_sync = peers_by_deleted_client
                .remove(client_id)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|peer_client_id| runtime_sync_by_client.get(&peer_client_id).cloned())
                .collect();
            public_by_client.insert(
                client_id.clone(),
                BulkDeleteAgentOutcome {
                    client_id: client_id.clone(),
                    status: "succeeded".to_string(),
                    result: Some(DeleteAgentResponse {
                        client_id: deleted.client_id,
                        deleted: true,
                        deleted_at: deleted.deleted_at,
                        post_commit: vec![
                            gateway_outcomes
                                .remove(client_id)
                                .ok_or_else(|| agent_delete_result_missing(client_id))?,
                            terminal_reconciliation.clone(),
                        ],
                        runtime_sync: per_client_runtime_sync,
                    }),
                    error_code: None,
                    error_message: None,
                },
            );
        }
        if let Some(event) = agent_delete_invalidation_event(&affected_client_ids) {
            state.events.invalidate_fleet_telemetry_read_cache();
            state.publish(event);
        }
    }

    let mut outcomes = Vec::with_capacity(request.items.len());
    for item in &request.items {
        outcomes.push(
            public_by_client
                .remove(&item.client_id)
                .ok_or_else(|| agent_delete_result_missing(&item.client_id))?,
        );
    }
    Ok(BulkDeleteAgentsResponse { outcomes })
}

fn agent_delete_invalidation_event(affected_client_ids: &[String]) -> Option<WsEvent> {
    (!affected_client_ids.is_empty()).then_some(WsEvent::FleetStateInvalidated)
}

fn singleton_delete_agent_response(
    mut response: BulkDeleteAgentsResponse,
) -> Result<Json<DeleteAgentResponse>, ApiError> {
    let outcome = response
        .outcomes
        .pop()
        .ok_or_else(|| agent_delete_result_missing("singleton"))?;
    match outcome.result {
        Some(result) => Ok(Json(result)),
        None => Err(delete_agent_rejection_error(
            outcome
                .error_code
                .as_deref()
                .unwrap_or("agent_delete_rejected"),
        )),
    }
}

fn rejected_delete_outcome(client_id: String, code: &'static str) -> BulkDeleteAgentOutcome {
    BulkDeleteAgentOutcome {
        client_id,
        status: "rejected".to_string(),
        result: None,
        error_code: Some(code.to_string()),
        error_message: Some(delete_agent_rejection_message(code).to_string()),
    }
}

fn delete_agent_rejection_message(code: &str) -> &'static str {
    match code {
        "agent_not_found" => "The VPS does not exist.",
        "agent_port_forwarding_cleanup_required" => {
            "Remove or confirm removal of active port forwarding before deleting this VPS."
        }
        "privilege_assertion_required" => "A privilege assertion is required for this VPS.",
        "privilege_verification_failed" => "The privilege assertion for this VPS was rejected.",
        _ => "The VPS deletion was rejected.",
    }
}

fn delete_agent_rejection_error(code: &str) -> ApiError {
    match code {
        "agent_not_found" => ApiError::not_found("agent_not_found"),
        "agent_port_forwarding_cleanup_required" => {
            ApiError::conflict("agent_port_forwarding_cleanup_required")
        }
        "privilege_assertion_required" => ApiError::forbidden("privilege_assertion_required"),
        "privilege_verification_failed" => ApiError::forbidden("privilege_verification_failed"),
        _ => ApiError::conflict("agent_delete_rejected"),
    }
}

fn agent_delete_result_missing(client_id: &str) -> ApiError {
    ApiError::internal(
        "agent_delete_result_missing",
        "The VPS deletion result set was incomplete.",
        anyhow::anyhow!("missing deletion outcome for {client_id}"),
    )
}

fn bulk_delete_privilege_error(error: anyhow::Error) -> ApiError {
    let code = if error.to_string().contains("ReplayProtectionSaturated") {
        "privilege_replay_protection_saturated"
    } else {
        "privilege_verification_unavailable"
    };
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code,
        error,
        public_message: Some(
            "The gateway could not verify all VPS deletion privileges; no VPS was changed."
                .to_string(),
        ),
    }
}

pub(crate) async fn suspend_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<SuspendAgentRequest>,
) -> Result<Json<AgentSuspensionMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    let response = mutate_agent_suspensions(
        &state,
        &operator,
        BulkAgentSuspensionRequest {
            action: AgentSuspensionAction::Suspend,
            client_ids: vec![client_id],
            confirmed: request.confirmed,
            reason: request.reason,
        },
    )
    .await?;
    singleton_agent_suspension_response(response)
}

pub(crate) async fn unsuspend_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<UnsuspendAgentRequest>,
) -> Result<Json<AgentSuspensionMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    let response = mutate_agent_suspensions(
        &state,
        &operator,
        BulkAgentSuspensionRequest {
            action: AgentSuspensionAction::Unsuspend,
            client_ids: vec![client_id],
            confirmed: request.confirmed,
            reason: None,
        },
    )
    .await?;
    singleton_agent_suspension_response(response)
}

pub(crate) async fn bulk_agent_suspensions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkAgentSuspensionRequest>,
) -> Result<Json<BulkAgentSuspensionResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    Ok(Json(
        mutate_agent_suspensions(&state, &operator, request).await?,
    ))
}

async fn mutate_agent_suspensions(
    state: &AppState,
    operator: &crate::model::AuthContext,
    request: BulkAgentSuspensionRequest,
) -> Result<BulkAgentSuspensionResponse, ApiError> {
    validate_bulk_agent_suspension_request(&request)?;

    let mut gateway_rejections = HashMap::new();
    let mut prepared_tokens = BTreeMap::new();
    let mut protected_job_ids = HashMap::new();
    let mut database_client_ids = request.client_ids.clone();
    if request.action == AgentSuspensionAction::Suspend && state.gateway.configured() {
        let prepare_items = request
            .client_ids
            .iter()
            .map(|client_id| {
                let token = uuid::Uuid::new_v4();
                prepared_tokens.insert(client_id.clone(), token);
                GatewayClientSuspensionFencePrepare {
                    client_id: client_id.clone(),
                    token,
                    lease_secs: AGENT_SUSPENSION_FENCE_LEASE_SECS,
                }
            })
            .collect::<Vec<_>>();
        let results = tokio::time::timeout(
            Duration::from_secs(AGENT_SUSPENSION_FENCE_CONTROL_ATTEMPT_SECS),
            state
                .gateway
                .prepare_client_suspension_fences(prepare_items),
        )
        .await
        .map_err(|error| {
            ApiError::internal(
                "agent_suspend_gateway_fence_timeout",
                "The gateway dispatch fences could not be prepared within their lease budget.",
                error.into(),
            )
        })?
        .map_err(ApiError::internal_mapper(
            "agent_suspend_gateway_fence_unavailable",
            "The gateway dispatch fences could not be prepared.",
        ))?
        .results;
        validate_gateway_fence_results(&request.client_ids, &results)?;
        database_client_ids.clear();
        for result in results {
            if result.accepted && result.fenced {
                protected_job_ids.insert(result.client_id.clone(), result.enqueued_job_ids);
                database_client_ids.push(result.client_id);
            } else {
                prepared_tokens.remove(&result.client_id);
                gateway_rejections.insert(result.client_id, "agent_suspend_gateway_fence_conflict");
            }
        }
    }

    let repository_outcomes = if database_client_ids.is_empty() {
        Vec::new()
    } else {
        match tokio::time::timeout(
            Duration::from_secs(AGENT_SUSPENSION_DB_BUDGET_SECS),
            state.repo.mutate_agent_suspensions(
                request.action,
                &database_client_ids,
                request.reason.as_deref(),
                operator,
                &protected_job_ids,
            ),
        )
        .await
        {
            Ok(Ok(outcomes)) => outcomes,
            Ok(Err(error)) => {
                compensate_agent_suspension_fences(state, &prepared_tokens).await;
                return Err(agent_mutation_error(error));
            }
            Err(error) => {
                compensate_agent_suspension_fences(state, &prepared_tokens).await;
                return Err(ApiError::internal(
                    "agent_suspension_timeout",
                    "The VPS suspension changes did not commit within their dispatch-fence budget.",
                    error.into(),
                ));
            }
        }
    };

    let mut public_by_client = HashMap::new();
    let mut affected_client_ids = Vec::new();
    let mut committed_tokens = BTreeMap::new();
    let mut rejected_tokens = BTreeMap::new();
    for outcome in repository_outcomes {
        match outcome {
            crate::repository_inventory::AgentSuspensionRepositoryOutcome::Applied {
                client_id,
                agent,
                mutation,
            } => {
                if let Some(token) = prepared_tokens.remove(&client_id) {
                    committed_tokens.insert(client_id.clone(), token);
                }
                affected_client_ids.push(client_id.clone());
                public_by_client.insert(
                    client_id.clone(),
                    BulkAgentSuspensionOutcome {
                        client_id,
                        status: "succeeded".to_string(),
                        result: Some(agent_suspension_mutation_response(*agent, mutation)),
                        error_code: None,
                        error_message: None,
                    },
                );
            }
            crate::repository_inventory::AgentSuspensionRepositoryOutcome::Rejected {
                client_id,
                code,
            } => {
                if let Some(token) = prepared_tokens.remove(&client_id) {
                    rejected_tokens.insert(client_id.clone(), token);
                }
                public_by_client.insert(
                    client_id.clone(),
                    rejected_suspension_outcome(client_id, code),
                );
            }
        }
    }

    compensate_agent_suspension_fences(state, &rejected_tokens).await;
    if request.action == AgentSuspensionAction::Suspend {
        promote_agent_suspension_fences(state, &committed_tokens).await;
    } else if state.gateway.configured() && !affected_client_ids.is_empty() {
        clear_agent_suspension_fences(state, &affected_client_ids, "operator_unsuspended").await;
    }

    for (client_id, code) in gateway_rejections {
        public_by_client.insert(
            client_id.clone(),
            rejected_suspension_outcome(client_id, code),
        );
    }
    let mut outcomes = Vec::with_capacity(request.client_ids.len());
    for client_id in &request.client_ids {
        outcomes.push(public_by_client.remove(client_id).ok_or_else(|| {
            ApiError::internal(
                "agent_suspension_result_missing",
                "The VPS suspension result set was incomplete.",
                anyhow::anyhow!("missing suspension outcome for {client_id}"),
            )
        })?);
    }
    if let Some(event) = agent_suspension_invalidation_event(&affected_client_ids) {
        state.events.invalidate_fleet_telemetry_read_cache();
        state.publish(event);
    }
    Ok(BulkAgentSuspensionResponse { outcomes })
}

fn agent_suspension_invalidation_event(affected_client_ids: &[String]) -> Option<WsEvent> {
    (!affected_client_ids.is_empty()).then_some(WsEvent::FleetStateInvalidated)
}

fn singleton_agent_suspension_response(
    mut response: BulkAgentSuspensionResponse,
) -> Result<Json<AgentSuspensionMutationResponse>, ApiError> {
    let outcome = response.outcomes.pop().ok_or_else(|| {
        ApiError::internal(
            "agent_suspension_result_missing",
            "The VPS suspension result was missing.",
            anyhow::anyhow!("agent_suspension_result_missing"),
        )
    })?;
    match outcome.result {
        Some(result) => Ok(Json(result)),
        None => Err(agent_suspension_rejection_error(
            outcome
                .error_code
                .as_deref()
                .unwrap_or("agent_suspension_rejected"),
        )),
    }
}

fn agent_suspension_mutation_response(
    agent: AgentView,
    mutation: crate::model::AgentSuspensionMutationResult,
) -> AgentSuspensionMutationResponse {
    let (suspended_at, suspended_by, suspended_reason, suspended_from_status) = mutation
        .record
        .map(|record| {
            (
                Some(record.suspended_at),
                record.suspended_by,
                record.suspended_reason,
                Some(record.suspended_from_status),
            )
        })
        .unwrap_or((None, None, None, None));
    AgentSuspensionMutationResponse {
        agent,
        suspended_at,
        suspended_by,
        suspended_reason,
        suspended_from_status,
        skipped_unstarted_job_ids: mutation.skipped_unstarted_job_ids,
        resolved_alert_count: mutation.resolved_alert_count,
    }
}

fn rejected_suspension_outcome(
    client_id: String,
    code: &'static str,
) -> BulkAgentSuspensionOutcome {
    BulkAgentSuspensionOutcome {
        client_id,
        status: "rejected".to_string(),
        result: None,
        error_code: Some(code.to_string()),
        error_message: Some(agent_suspension_rejection_message(code).to_string()),
    }
}

fn agent_suspension_rejection_message(code: &str) -> &'static str {
    match code {
        "agent_not_found" => "The VPS does not exist or is no longer visible.",
        "agent_already_suspended" => "The VPS is already suspended.",
        "agent_not_suspended" => "The VPS is not suspended.",
        "agent_suspend_online" => "An online VPS cannot be suspended.",
        "agent_suspend_ineligible" => "The VPS lifecycle state is not eligible for suspension.",
        "agent_suspend_gateway_fence_conflict" => {
            "The gateway could not establish the VPS dispatch fence."
        }
        _ => "The VPS suspension change was rejected.",
    }
}

fn agent_suspension_rejection_error(code: &str) -> ApiError {
    match code {
        "agent_not_found" => ApiError::not_found("agent_not_found"),
        "agent_already_suspended" => ApiError::conflict("agent_already_suspended"),
        "agent_not_suspended" => ApiError::conflict("agent_not_suspended"),
        "agent_suspend_online" => ApiError::conflict("agent_suspend_online"),
        "agent_suspend_ineligible" => ApiError::conflict("agent_suspend_ineligible"),
        "agent_suspend_gateway_fence_conflict" => {
            ApiError::conflict("agent_suspend_gateway_fence_conflict")
        }
        _ => ApiError::conflict("agent_suspension_rejected"),
    }
}

fn validate_gateway_fence_results(
    client_ids: &[String],
    results: &[vpsman_common::GatewayClientSuspensionFenceResult],
) -> Result<(), ApiError> {
    if client_ids.len() != results.len()
        || client_ids
            .iter()
            .zip(results)
            .any(|(client_id, result)| client_id != &result.client_id)
    {
        return Err(ApiError::internal(
            "agent_suspend_gateway_fence_result_invalid",
            "The gateway returned an invalid dispatch-fence result set.",
            anyhow::anyhow!("gateway suspension fence results did not preserve request order"),
        ));
    }
    Ok(())
}

async fn compensate_agent_suspension_fences(
    state: &AppState,
    fences: &BTreeMap<String, uuid::Uuid>,
) {
    if fences.is_empty() || !state.gateway.configured() {
        return;
    }
    let items = fences
        .iter()
        .map(|(client_id, token)| GatewayClientSuspensionFenceClear {
            client_id: client_id.clone(),
            expected_token: Some(*token),
            reason: "suspension_not_committed".to_string(),
        })
        .collect();
    match tokio::time::timeout(
        Duration::from_secs(AGENT_SUSPENSION_FENCE_CONTROL_ATTEMPT_SECS),
        state.gateway.clear_client_suspension_fences(items),
    )
    .await
    {
        Ok(Ok(results))
            if results
                .results
                .iter()
                .all(|result| result.accepted && !result.fenced) => {}
        Ok(Ok(_)) => tracing::warn!(
            "some temporary suspension fence compensations were rejected; recovery is deferred to lease expiry"
        ),
        Ok(Err(error)) => tracing::warn!(
            %error,
            "temporary suspension fence compensation is deferred to lease expiry"
        ),
        Err(error) => tracing::warn!(
            %error,
            "temporary suspension fence compensation timed out and is deferred to lease expiry"
        ),
    }
}

async fn promote_agent_suspension_fences(state: &AppState, fences: &BTreeMap<String, uuid::Uuid>) {
    if fences.is_empty() || !state.gateway.configured() {
        return;
    }
    let items = fences
        .iter()
        .map(|(client_id, token)| GatewayClientSuspensionFencePromote {
            client_id: client_id.clone(),
            token: *token,
        })
        .collect::<Vec<_>>();
    let result = tokio::time::timeout(
        Duration::from_secs(AGENT_SUSPENSION_FENCE_CONTROL_ATTEMPT_SECS),
        state.gateway.promote_client_suspension_fences(items),
    )
    .await;
    if !matches!(&result, Ok(Ok(batch)) if batch.results.iter().all(|item| item.accepted && item.fenced))
    {
        tracing::error!(
            ?result,
            "committed suspension fences were not all promoted; durable dispatch rechecks remain active"
        );
    }
}

async fn clear_agent_suspension_fences(state: &AppState, client_ids: &[String], reason: &str) {
    let items = client_ids
        .iter()
        .map(|client_id| GatewayClientSuspensionFenceClear {
            client_id: client_id.clone(),
            expected_token: None,
            reason: reason.to_string(),
        })
        .collect::<Vec<_>>();
    let result = tokio::time::timeout(
        Duration::from_secs(AGENT_SUSPENSION_FENCE_CONTROL_ATTEMPT_SECS),
        state.gateway.clear_client_suspension_fences(items),
    )
    .await;
    if !matches!(&result, Ok(Ok(batch)) if batch.results.iter().all(|item| item.accepted && !item.fenced))
    {
        tracing::warn!(
            ?result,
            "committed unsuspensions could not all clear gateway fences; an accepted reconnect will reconcile the local route"
        );
    }
}

pub(crate) async fn list_gateway_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<GatewaySessionView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .list_gateway_sessions(limit_or_default(query.limit))
            .await
            .map_err(ApiError::internal_mapper(
                "gateway_sessions_unavailable",
                "Gateway sessions could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn list_telemetry_rollups(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TelemetryRollupQuery>,
) -> Result<Json<Vec<TelemetryRollupView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_telemetry_rollup_query(&query)?;
    let rows = if query.latest {
        state
            .repo
            .list_latest_telemetry_rollups(
                limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.bucket_secs,
            )
            .await
            .map_err(ApiError::internal_mapper(
                "telemetry_rollups_unavailable",
                "Telemetry rollups could not be loaded.",
            ))?
    } else {
        state
            .repo
            .list_telemetry_rollups(
                limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.bucket_secs,
                true,
            )
            .await
            .map_err(ApiError::internal_mapper(
                "telemetry_rollups_unavailable",
                "Telemetry rollups could not be loaded.",
            ))?
    };
    Ok(Json(rows))
}

pub(crate) async fn list_telemetry_samples(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TelemetrySampleQuery>,
) -> Result<Json<Vec<TelemetrySampleView>>, ApiError> {
    state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_telemetry_sample_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_telemetry_samples(
                limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.start_unix,
                query.end_unix,
                true,
            )
            .await
            .map_err(ApiError::internal_mapper(
                "telemetry_samples_unavailable",
                "Telemetry samples could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn list_telemetry_network_rates(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TelemetryNetworkRateQuery>,
) -> Result<Json<Vec<TelemetryNetworkRateView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_telemetry_network_rate_query(&query)?;
    let rows = if query.latest {
        state
            .repo
            .list_latest_telemetry_network_rates(
                telemetry_network_rate_limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.interface.as_deref(),
                query.bucket_secs,
            )
            .await
            .map_err(ApiError::internal_mapper(
                "telemetry_network_rates_unavailable",
                "Telemetry network rates could not be loaded.",
            ))?
    } else {
        state
            .repo
            .list_telemetry_network_rates(
                telemetry_network_rate_limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.interface.as_deref(),
                query.bucket_secs,
                true,
            )
            .await
            .map_err(ApiError::internal_mapper(
                "telemetry_network_rates_unavailable",
                "Telemetry network rates could not be loaded.",
            ))?
    };
    Ok(Json(rows))
}

pub(crate) async fn list_telemetry_tunnels(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TelemetryTunnelQuery>,
) -> Result<Json<Vec<TelemetryTunnelView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_telemetry_tunnel_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_telemetry_tunnels(
                limit_or_default(query.limit),
                query.client_id.as_deref(),
                query.interface.as_deref(),
            )
            .await
            .map_err(ApiError::internal_mapper(
                "telemetry_tunnels_unavailable",
                "Tunnel telemetry could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn list_tags(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<TagView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(state.repo.list_tags().await.map_err(
        ApiError::internal_mapper("tags_unavailable", "Tags could not be loaded."),
    )?))
}

pub(crate) async fn get_tag_order(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<TagOrderState>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(state.repo.tag_order_state().await.map_err(
        ApiError::internal_mapper(
            "tag_order_unavailable",
            "The tag order could not be loaded.",
        ),
    )?))
}

pub(crate) async fn update_tag_order(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateTagOrderRequest>,
) -> Result<Json<TagOrderState>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_tag_order_request(&request)?;
    Ok(Json(
        state
            .repo
            .update_tag_order(&request, operator.operator.id)
            .await
            .map_err(tag_order_error)?,
    ))
}

pub(crate) async fn list_runtime_config_patch_generators(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<RuntimeConfigPatchGeneratorView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .list_runtime_config_patch_generators()
            .await
            .map_err(ApiError::internal_mapper(
                "runtime_config_patch_generators_unavailable",
                "Runtime-config patch generators could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn upsert_runtime_config_patch_generator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpsertRuntimeConfigPatchGeneratorRequest>,
) -> Result<Json<RuntimeConfigPatchGeneratorView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    validate_runtime_config_patch_generator(&request)?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "runtime_config_patch_generator_confirmation_required",
        ));
    }
    Ok(Json(
        state
            .repo
            .upsert_runtime_config_patch_generator(&request, &operator)
            .await
            .map_err(runtime_config_patch_generator_error)?,
    ))
}

pub(crate) async fn render_runtime_config_patch_generator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(generator_id): Path<uuid::Uuid>,
    Json(request): Json<RenderRuntimeConfigPatchGeneratorRequest>,
) -> Result<Json<RuntimeConfigPatchGeneratorRenderView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .render_runtime_config_patch_generator(generator_id, &request)
            .await
            .map_err(runtime_config_patch_generator_error)?,
    ))
}

pub(crate) async fn delete_runtime_config_patch_generator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(generator_id): Path<uuid::Uuid>,
    Json(request): Json<DeleteRuntimeConfigPatchGeneratorRequest>,
) -> Result<StatusCode, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "runtime_config_patch_generator_delete_confirmation_required",
        ));
    }
    validate_short_required_value(
        &request.reviewed_name,
        "runtime_config_patch_generator_delete_review_invalid",
    )?;
    state
        .repo
        .delete_runtime_config_patch_generator(generator_id, &request.reviewed_name, &operator)
        .await
        .map_err(runtime_config_patch_generator_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_runtime_config_apply_states(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<RuntimeConfigApplyStateView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .list_runtime_config_apply_states(None)
            .await
            .map_err(ApiError::internal_mapper(
                "runtime_config_apply_states_unavailable",
                "Runtime-config apply states could not be loaded.",
            ))?,
    ))
}

fn validate_client_id(client_id: &str) -> Result<(), ApiError> {
    if client_id.is_empty() || client_id.len() > 128 {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    Ok(())
}

pub(crate) async fn create_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateTagRequest>,
) -> Result<Json<TagView>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict("tag_mutation_confirmation_required"));
    }
    validate_persisted_tag_name(&request.name)?;
    let targets = Vec::<String>::new();
    let intent = DbPrivilegeIntent::new("tag.create", &request.name, None, &targets, true, None);
    verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    Ok(Json(state.repo.create_tag(request).await.map_err(
        ApiError::internal_mapper("tag_create_failed", "The tag could not be created."),
    )?))
}

pub(crate) async fn bulk_mutate_tags(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<BulkTagMutationRequest>,
) -> Result<Json<TagMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_persisted_tag_name(&request.tag)?;
    validate_bulk_selector_expression(&request.selector_expression)?;
    let allow_vps_rule_selectors = operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ);
    request.target_client_ids = normalized_target_client_ids(&request.target_client_ids)?;
    let fixed_targets = verified_fixed_target_ids(
        &state,
        &request.target_client_ids,
        "tag_fixed_targets_not_found",
    )
    .await?;
    if request.confirmed {
        let mut preview_request = request.clone();
        preview_request.confirmed = false;
        preview_request.preview_hash = None;
        preview_request.privilege_assertion = None;
        let preview = state
            .repo
            .bulk_mutate_tags(&preview_request, allow_vps_rule_selectors)
            .await
            .map_err(|error| {
                tag_mutation_error(
                    error,
                    "tag_mutation_preview_failed",
                    "The tag-mutation preview could not be prepared.",
                )
            })?;
        require_matching_preview_hash(
            request.preview_hash.as_deref(),
            &preview.preview_hash,
            "tag_mutation_preview_hash_required",
            "tag_mutation_preview_hash_mismatch",
        )?;
        let action = match request.action {
            crate::model::BulkTagMutationAction::Add => "tag.bulk_add",
            crate::model::BulkTagMutationAction::Remove => "tag.bulk_remove",
        };
        let intent = DbPrivilegeIntent::new(
            action,
            &request.tag,
            Some(&request.selector_expression),
            &fixed_targets,
            request.confirmed,
            None,
        );
        verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    }
    Ok(Json(
        state
            .repo
            .bulk_mutate_tags(&request, allow_vps_rule_selectors)
            .await
            .map_err(|error| {
                tag_mutation_error(
                    error,
                    "tag_mutation_failed",
                    "The tag mutation could not be completed.",
                )
            })?,
    ))
}

pub(crate) async fn delete_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(tag): Path<String>,
    Json(request): Json<DeleteTagRequest>,
) -> Result<Json<TagMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_persisted_tag_name(&tag)?;
    let allow_vps_rule_selectors = operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ);
    if request.confirmed {
        let preview = state
            .repo
            .delete_tag(&tag, false, allow_vps_rule_selectors)
            .await
            .map_err(|error| {
                tag_mutation_error(
                    error,
                    "tag_delete_preview_failed",
                    "The tag-deletion preview could not be prepared.",
                )
            })?;
        require_matching_preview_hash(
            request.preview_hash.as_deref(),
            &preview.preview_hash,
            "tag_delete_preview_hash_required",
            "tag_delete_preview_hash_mismatch",
        )?;
        let affected_targets = preview
            .affected
            .iter()
            .map(|client| client.id.clone())
            .collect::<Vec<_>>();
        let intent =
            DbPrivilegeIntent::new("tag.delete", &tag, None, &affected_targets, true, None);
        verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    }
    Ok(Json(
        state
            .repo
            .delete_tag(&tag, request.confirmed, allow_vps_rule_selectors)
            .await
            .map_err(|error| {
                tag_mutation_error(error, "tag_delete_failed", "The tag could not be deleted.")
            })?,
    ))
}

fn require_matching_preview_hash(
    submitted: Option<&str>,
    expected: &str,
    required_code: &'static str,
    mismatch_code: &'static str,
) -> Result<(), ApiError> {
    let Some(submitted) = submitted.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err(ApiError::conflict(required_code));
    };
    if submitted != expected {
        return Err(ApiError::conflict(mismatch_code));
    }
    Ok(())
}

fn validate_runtime_config_patch_generator(
    request: &UpsertRuntimeConfigPatchGeneratorRequest,
) -> Result<(), ApiError> {
    for value in [
        request.name.as_str(),
        request.category.as_str(),
        request.domain.as_str(),
        request.description.as_str(),
    ] {
        if value.trim().is_empty() || value.len() > MAX_RUNTIME_CONFIG_FIELD_BYTES {
            return Err(ApiError::bad_request(
                "runtime_config_patch_generator_invalid",
            ));
        }
    }
    if request.raw_generator_body.trim().is_empty()
        || request.raw_generator_body.len() > MAX_PATCH_GENERATOR_BODY_BYTES
    {
        return Err(ApiError::bad_request(
            "runtime_config_patch_generator_body_invalid",
        ));
    }
    if !request.field_schema.is_object() || !request.docs_metadata.is_object() {
        return Err(ApiError::bad_request(
            "runtime_config_patch_generator_metadata_invalid",
        ));
    }
    Ok(())
}

fn runtime_config_patch_generator_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("not_found") {
        ApiError::not_found("runtime_config_patch_generator_not_found")
    } else if message.contains("runtime_config_patch_generator_builtin_immutable") {
        ApiError::conflict("runtime_config_patch_generator_builtin_immutable")
    } else if message.contains("runtime_config_patch_generator_delete_review_stale") {
        ApiError::conflict("runtime_config_patch_generator_delete_review_stale")
    } else {
        ApiError::internal(
            "runtime_config_patch_generator_mutation_failed",
            "The runtime-config patch generator change could not be completed.",
            error,
        )
    }
}

fn validate_short_required_value(value: &str, error: &'static str) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 {
        return Err(ApiError::bad_request(error));
    }
    Ok(())
}

async fn verified_fixed_target_ids(
    state: &AppState,
    target_client_ids: &[String],
    error_code: &'static str,
) -> Result<Vec<String>, ApiError> {
    let target_client_ids = normalized_target_client_ids(target_client_ids)?;
    let resolved = state
        .repo
        .resolve_bulk_targets(&fixed_target_selection(&target_client_ids)?)
        .await
        .map_err(ApiError::internal_mapper(
            "fixed_targets_unavailable",
            "The selected VPS targets could not be loaded.",
        ))?
        .targets
        .into_iter()
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    let missing = target_client_ids
        .iter()
        .filter(|client_id| !resolved.iter().any(|resolved_id| resolved_id == *client_id))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(ApiError::conflict(error_code));
    }
    Ok(target_client_ids)
}

pub(crate) async fn assign_agent_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<AssignTagRequest>,
) -> Result<Json<TagMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_persisted_tag_name(&request.tag)?;
    let allow_vps_rule_selectors = operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ);
    if request.confirmed {
        let targets = vec![client_id.clone()];
        let intent = DbPrivilegeIntent::new("tag.assign", &request.tag, None, &targets, true, None);
        verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    }
    Ok(Json(
        state
            .repo
            .assign_agent_tag_mutation(
                &client_id,
                &request.tag,
                request.confirmed,
                allow_vps_rule_selectors,
            )
            .await
            .map_err(|error| {
                tag_mutation_error(
                    error,
                    "tag_assignment_failed",
                    "The VPS tag assignment could not be completed.",
                )
            })?,
    ))
}

pub(crate) async fn resolve_bulk_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkResolveRequest>,
) -> Result<Json<BulkResolveResponse>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    let expression = validate_bulk_selector_expression(&request.selector_expression)?;
    require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    Ok(Json(
        state
            .repo
            .resolve_bulk_targets(&request)
            .await
            .map_err(ApiError::internal_mapper(
                "selector_targets_unavailable",
                "Selector targets could not be resolved.",
            ))?,
    ))
}

pub(crate) async fn resolve_many_bulk_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkResolveManyRequest>,
) -> Result<Json<BulkResolveManyResponse>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    let selectors = validate_bulk_resolve_many_request(&request, &operator.operator.scopes)?;
    Ok(Json(
        state
            .repo
            .resolve_many_bulk_targets(&selectors)
            .await
            .map_err(ApiError::internal_mapper(
                "selector_targets_unavailable",
                "Selector targets could not be resolved.",
            ))?,
    ))
}

fn validate_bulk_resolve_many_request(
    request: &BulkResolveManyRequest,
    operator_scopes: &[String],
) -> Result<Vec<(String, vpsman_common::Expression)>, ApiError> {
    if !(1..=BULK_RESOLVE_MANY_ITEM_LIMIT).contains(&request.items.len()) {
        return Err(ApiError::bad_request("selector_batch_items_invalid"));
    }
    let mut seen = HashSet::with_capacity(request.items.len());
    let mut selectors = Vec::with_capacity(request.items.len());
    for item in &request.items {
        let selector_expression = item.selector_expression.trim().to_string();
        if !seen.insert(selector_expression.clone()) {
            return Err(ApiError::bad_request("selector_batch_duplicate_item"));
        }
        let expression = validate_bulk_selector_expression(&selector_expression)?;
        require_vps_rule_selector_scope(operator_scopes, &expression)?;
        selectors.push((selector_expression, expression));
    }
    Ok(selectors)
}

fn validate_bulk_selector_expression(
    selector_expression: &str,
) -> Result<vpsman_common::Expression, ApiError> {
    if selector_expression.trim().is_empty() {
        return Err(ApiError::bad_request("selector_expression_required"));
    }
    parse_selector_expression(selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        .ok_or_else(|| ApiError::bad_request("selector_expression_required"))
}

fn tag_mutation_error(error: anyhow::Error, code: &'static str, message: &'static str) -> ApiError {
    if error
        .to_string()
        .contains("vps_rule_selector_scope_required")
    {
        return ApiError::forbidden("operator_scope_insufficient");
    }
    ApiError::internal(code, message, error)
}

fn validate_telemetry_rollup_query(query: &TelemetryRollupQuery) -> Result<(), ApiError> {
    if query
        .client_id
        .as_ref()
        .is_some_and(|client_id| client_id.is_empty() || client_id.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    if query
        .bucket_secs
        .is_some_and(|bucket_secs| bucket_secs < 60 || bucket_secs % 60 != 0)
    {
        return Err(ApiError::bad_request("invalid_bucket_secs"));
    }
    Ok(())
}

fn validate_telemetry_sample_query(query: &TelemetrySampleQuery) -> Result<(), ApiError> {
    if query
        .client_id
        .as_ref()
        .is_some_and(|client_id| client_id.is_empty() || client_id.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    if query
        .start_unix
        .zip(query.end_unix)
        .is_some_and(|(start, end)| start > end)
    {
        return Err(ApiError::bad_request("invalid_telemetry_time_range"));
    }
    Ok(())
}

fn validate_persisted_tag_name(tag: &str) -> Result<(), ApiError> {
    if tag.is_empty() || tag.len() > 128 {
        return Err(ApiError::bad_request("invalid_tag_name"));
    }
    if tag.starts_with("id:") || tag.starts_with("name:") {
        return Err(ApiError::bad_request("reserved_inner_tag_selector"));
    }
    if !tag
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(ApiError::bad_request("invalid_tag_name"));
    }
    if tag.split(':').any(str::is_empty) {
        return Err(ApiError::bad_request("invalid_tag_name"));
    }
    Ok(())
}

fn validate_tag_order_request(request: &UpdateTagOrderRequest) -> Result<(), ApiError> {
    if request.ordered_tags.len() > 1000 {
        return Err(ApiError::bad_request("too_many_ordered_tags"));
    }
    let mut seen = HashSet::new();
    for tag in &request.ordered_tags {
        validate_persisted_tag_name(tag)?;
        if !seen.insert(tag.as_str()) {
            return Err(ApiError::bad_request("duplicate_tag"));
        }
    }
    Ok(())
}

fn tag_order_error(error: anyhow::Error) -> ApiError {
    match error.to_string().as_str() {
        "unknown_tag" => ApiError::bad_request("unknown_tag"),
        "duplicate_tag" => ApiError::bad_request("duplicate_tag"),
        _ => ApiError::internal(
            "tag_order_update_failed",
            "The tag order could not be updated.",
            error,
        ),
    }
}

fn validate_agent_alias(display_name: &str) -> Result<(), ApiError> {
    let display_name = display_name.trim();
    if display_name.is_empty()
        || display_name.len() > 160
        || display_name.chars().any(|character| character.is_control())
    {
        return Err(ApiError::bad_request("agent_alias_invalid"));
    }
    Ok(())
}

fn validate_agent_alias_confirmation(request: &UpdateAgentAliasRequest) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict("agent_alias_confirmation_required"));
    }
    Ok(())
}

fn validate_bulk_delete_agents_request(request: &BulkDeleteAgentsRequest) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict("agent_delete_confirmation_required"));
    }
    if request.items.is_empty() || request.items.len() > GATEWAY_CONTROL_BATCH_MAX_ITEMS {
        return Err(ApiError::bad_request("agent_delete_targets_invalid"));
    }
    let mut unique = HashSet::with_capacity(request.items.len());
    for item in &request.items {
        validate_client_id(&item.client_id)?;
        if !unique.insert(item.client_id.as_str()) {
            return Err(ApiError::bad_request("agent_delete_targets_duplicate"));
        }
    }
    if request
        .reason
        .as_deref()
        .is_some_and(|reason| reason.trim().len() > 240 || reason.chars().any(char::is_control))
    {
        return Err(ApiError::bad_request("agent_delete_reason_invalid"));
    }
    Ok(())
}

fn validate_bulk_agent_suspension_request(
    request: &BulkAgentSuspensionRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict(match request.action {
            AgentSuspensionAction::Suspend => "agent_suspend_confirmation_required",
            AgentSuspensionAction::Unsuspend => "agent_unsuspend_confirmation_required",
        }));
    }
    if request.client_ids.is_empty()
        || request.client_ids.len() > GATEWAY_CLIENT_SUSPENSION_FENCE_BATCH_MAX_ITEMS
    {
        return Err(ApiError::bad_request("agent_suspension_targets_invalid"));
    }
    let mut unique = HashSet::with_capacity(request.client_ids.len());
    for client_id in &request.client_ids {
        validate_client_id(client_id)?;
        if !unique.insert(client_id.as_str()) {
            return Err(ApiError::bad_request("agent_suspension_targets_duplicate"));
        }
    }
    if request.reason.as_deref().is_some_and(|reason| {
        reason.trim().chars().count() > 240 || reason.chars().any(char::is_control)
    }) {
        return Err(ApiError::bad_request("agent_suspend_reason_invalid"));
    }
    if request.action == AgentSuspensionAction::Unsuspend
        && request
            .reason
            .as_deref()
            .is_some_and(|reason| !reason.trim().is_empty())
    {
        return Err(ApiError::bad_request("agent_unsuspend_reason_invalid"));
    }
    Ok(())
}

#[cfg(test)]
fn validate_suspend_agent_status(status: &str) -> Result<(), ApiError> {
    match status {
        "never" | "disconnected" | "offline" | "stale" => Ok(()),
        "suspended" => Err(ApiError::conflict("agent_already_suspended")),
        "online" => Err(ApiError::conflict("agent_suspend_online")),
        _ => Err(ApiError::conflict("agent_suspend_ineligible")),
    }
}

fn peer_client_ids_for_deleted_agent(
    client_id: &str,
    endpoint_pairs: impl IntoIterator<Item = (String, String)>,
) -> BTreeSet<String> {
    let mut peers = BTreeSet::new();
    for (left_client_id, right_client_id) in endpoint_pairs {
        if left_client_id == client_id && right_client_id != client_id {
            peers.insert(right_client_id);
        } else if right_client_id == client_id && left_client_id != client_id {
            peers.insert(left_client_id);
        }
    }
    peers
}

fn agent_mutation_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("agent_not_found") {
        ApiError::not_found("agent_not_found")
    } else if message.contains("agent_already_suspended") {
        ApiError::conflict("agent_already_suspended")
    } else if message.contains("agent_not_suspended") {
        ApiError::conflict("agent_not_suspended")
    } else if message.contains("agent_suspend_online") {
        ApiError::conflict("agent_suspend_online")
    } else if message.contains("agent_suspend_ineligible") {
        ApiError::conflict("agent_suspend_ineligible")
    } else if message.contains("agent_suspend_reason_invalid") {
        ApiError::bad_request("agent_suspend_reason_invalid")
    } else if message.contains("agent_port_forwarding_cleanup_required") {
        ApiError::conflict("agent_port_forwarding_cleanup_required")
    } else if message.contains("display_name_already_exists")
        || message.contains("clients_visible_display_name_key_idx")
    {
        ApiError::conflict("display_name_already_exists")
    } else {
        ApiError::internal(
            "vps_identity_mutation_failed",
            "The VPS identity change could not be completed.",
            error,
        )
    }
}

fn validate_telemetry_network_rate_query(
    query: &TelemetryNetworkRateQuery,
) -> Result<(), ApiError> {
    if query
        .client_id
        .as_ref()
        .is_some_and(|client_id| client_id.is_empty() || client_id.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    if query
        .interface
        .as_ref()
        .is_some_and(|interface| interface.is_empty() || interface.len() > 64)
    {
        return Err(ApiError::bad_request("invalid_network_interface"));
    }
    if query
        .bucket_secs
        .is_some_and(|bucket_secs| bucket_secs < 60 || bucket_secs % 60 != 0)
    {
        return Err(ApiError::bad_request("invalid_bucket_secs"));
    }
    Ok(())
}

fn telemetry_network_rate_limit_or_default(limit: Option<i64>) -> i64 {
    limit
        .unwrap_or(100)
        .clamp(1, TELEMETRY_NETWORK_RATE_LIMIT_MAX)
}

fn validate_telemetry_tunnel_query(query: &TelemetryTunnelQuery) -> Result<(), ApiError> {
    if query
        .client_id
        .as_ref()
        .is_some_and(|client_id| client_id.is_empty() || client_id.len() > 128)
    {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    if query
        .interface
        .as_ref()
        .is_some_and(|interface| interface.is_empty() || interface.len() > 64)
    {
        return Err(ApiError::bad_request("invalid_tunnel_interface"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests_routes_inventory.rs"]
mod tests;
