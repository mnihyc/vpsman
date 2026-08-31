use std::{
    collections::{BTreeSet, HashMap, HashSet},
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
    GatewayClientDispatchFenceAcquire, GatewayClientDispatchFenceClear,
    GatewayClientDispatchFencePrepare, GatewayClientDispatchFencePromote,
    GatewayClientDispatchFencePurpose, GatewayPrivilegeVerification,
    GatewayPrivilegeVerificationBatchItem, GATEWAY_CLIENT_DISPATCH_FENCE_BATCH_MAX_ITEMS,
    GATEWAY_CONTROL_BATCH_MAX_ITEMS, MAX_RUNTIME_CONFIG_FIELD_BYTES,
};

const BULK_RESOLVE_MANY_ITEM_LIMIT: usize = 500;
const CLIENT_LIFECYCLE_FENCE_LEASE_SECS: u64 = 60;
const CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS: u64 = 5;
// Renewal is derived from the lease, leaving two complete intervals for a
// delayed attempt; it is lifecycle correctness, not a throughput throttle.
const CLIENT_LIFECYCLE_FENCE_RENEWAL_SECS: u64 = CLIENT_LIFECYCLE_FENCE_LEASE_SECS / 3;

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

struct ClientDispatchFenceLease {
    state: AppState,
    client_id: String,
    token: uuid::Uuid,
    gateway_epoch: uuid::Uuid,
    generation: u64,
    purpose: GatewayClientDispatchFencePurpose,
    initially_protected: HashSet<uuid::Uuid>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    renewer: Option<tokio::task::JoinHandle<()>>,
    ownership_healthy: tokio::sync::watch::Receiver<bool>,
}

async fn retire_unconfirmed_dispatch_fence(
    state: &AppState,
    prepare: &GatewayClientDispatchFencePrepare,
) {
    let request = GatewayClientDispatchFenceClear {
        client_id: prepare.client_id.clone(),
        expected_token: prepare.token,
        gateway_epoch: prepare.gateway_epoch,
        expected_generation: prepare.generation,
        restore_fallback: true,
        reason: "dispatch_fence_prepare_unconfirmed".to_string(),
    };
    let result = tokio::time::timeout(
        Duration::from_secs(CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS),
        state.gateway.clear_client_dispatch_fences(vec![request]),
    )
    .await;
    if !matches!(
        &result,
        Ok(Ok(batch))
            if matches!(batch.results.as_slice(), [result]
                if result.client_id == prepare.client_id
                    && (result.accepted
                        || result.message == "dispatch_fence_gateway_epoch_stale"))
    ) {
        tracing::warn!(
            client_id = %prepare.client_id,
            token = %prepare.token,
            ?result,
            "unconfirmed dispatch-fence owner retirement was not acknowledged"
        );
    }
}

impl ClientDispatchFenceLease {
    fn owner(&self) -> vpsman_common::GatewayClientDispatchFenceOwner {
        vpsman_common::GatewayClientDispatchFenceOwner {
            token: self.token,
            gateway_epoch: self.gateway_epoch,
            generation: self.generation,
        }
    }

    async fn prepare(
        state: &AppState,
        client_id: &str,
        purpose: GatewayClientDispatchFencePurpose,
        supersede_prepared_suspension: bool,
    ) -> anyhow::Result<(Self, Vec<uuid::Uuid>)> {
        let token = uuid::Uuid::new_v4();
        state.refresh_gateway_dispatch_timeouts();
        let acquired = tokio::time::timeout(
            Duration::from_secs(CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS),
            state
                .gateway
                .acquire_client_dispatch_fence(GatewayClientDispatchFenceAcquire {
                    client_id: client_id.to_string(),
                    token,
                    purpose,
                    supersede_prepared_suspension,
                }),
        )
        .await
        .map_err(|error| anyhow::anyhow!("gateway dispatch-fence acquire timed out: {error}"))??;
        anyhow::ensure!(
            acquired.client_id == client_id && acquired.owner.token == token,
            "gateway dispatch-fence acquire result invalid"
        );
        let mut prepare = GatewayClientDispatchFencePrepare {
            client_id: client_id.to_string(),
            token,
            gateway_epoch: acquired.owner.gateway_epoch,
            generation: acquired.owner.generation,
            renewal: false,
            lease_secs: CLIENT_LIFECYCLE_FENCE_LEASE_SECS,
            purpose,
        };
        let prepared = tokio::time::timeout(
            Duration::from_secs(CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS),
            state
                .gateway
                .prepare_client_dispatch_fences(vec![prepare.clone()]),
        )
        .await;
        let result = match &prepared {
            Ok(Ok(batch)) => match batch.results.as_slice() {
                [result] if result.client_id == client_id && result.accepted && result.fenced => {
                    result.clone()
                }
                _ => {
                    retire_unconfirmed_dispatch_fence(state, &prepare).await;
                    anyhow::bail!("gateway dispatch-fence conflict");
                }
            },
            Ok(Err(error)) => {
                let message = error.to_string();
                retire_unconfirmed_dispatch_fence(state, &prepare).await;
                anyhow::bail!("gateway dispatch-fence prepare failed: {message}");
            }
            Err(error) => {
                let message = error.to_string();
                retire_unconfirmed_dispatch_fence(state, &prepare).await;
                anyhow::bail!("gateway dispatch-fence prepare timed out: {message}");
            }
        };

        let protected_job_ids = result.enqueued_job_ids.clone();
        let initially_protected = protected_job_ids.iter().copied().collect::<HashSet<_>>();
        let renewal_protected = initially_protected.clone();
        let renewal_state = state.clone();
        prepare.renewal = true;
        let renewal_prepare = prepare;
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let (ownership_health, ownership_healthy) = tokio::sync::watch::channel(true);
        let renewer = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stopped => return,
                    _ = tokio::time::sleep(Duration::from_secs(
                        CLIENT_LIFECYCLE_FENCE_RENEWAL_SECS,
                    )) => {}
                }
                let renewal = tokio::time::timeout(
                    Duration::from_secs(CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS),
                    renewal_state
                        .gateway
                        .prepare_client_dispatch_fences(vec![renewal_prepare.clone()]),
                )
                .await;
                let renewed = matches!(
                    &renewal,
                    Ok(Ok(batch))
                        if matches!(batch.results.as_slice(), [result]
                            if result.client_id == renewal_prepare.client_id
                                && result.accepted
                                && result.fenced
                                && result.ownership_continuous
                                && result.enqueued_job_ids.iter().all(|job_id|
                                    renewal_protected.contains(job_id)))
                );
                if !renewed {
                    let _ = ownership_health.send(false);
                    tracing::warn!(
                        client_id = %renewal_prepare.client_id,
                        ?purpose,
                        "exact-client dispatch fence renewal was not acknowledged"
                    );
                }
            }
        });
        Ok((
            Self {
                state: state.clone(),
                client_id: client_id.to_string(),
                token,
                gateway_epoch: acquired.owner.gateway_epoch,
                generation: acquired.owner.generation,
                purpose,
                initially_protected,
                stop: Some(stop),
                renewer: Some(renewer),
                ownership_healthy,
            },
            protected_job_ids,
        ))
    }

    fn commit_proof(&self) -> ClientDispatchFenceCommitProof {
        ClientDispatchFenceCommitProof {
            state: self.state.clone(),
            client_id: self.client_id.clone(),
            token: self.token,
            gateway_epoch: self.gateway_epoch,
            generation: self.generation,
            purpose: self.purpose,
            initially_protected: self.initially_protected.clone(),
            ownership_healthy: self.ownership_healthy.clone(),
        }
    }

    async fn promote_once(&self) -> Option<bool> {
        let request = GatewayClientDispatchFencePromote {
            client_id: self.client_id.clone(),
            token: self.token,
            gateway_epoch: self.gateway_epoch,
            generation: self.generation,
            purpose: self.purpose,
        };
        let result = tokio::time::timeout(
            Duration::from_secs(CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS),
            self.state
                .gateway
                .promote_client_dispatch_fences(vec![request]),
        )
        .await;
        match result {
            Ok(Ok(batch)) => match batch.results.as_slice() {
                [result]
                    if result.client_id == self.client_id
                        && result.accepted
                        && result.fenced
                        && result
                            .enqueued_job_ids
                            .iter()
                            .all(|job_id| self.initially_protected.contains(job_id)) =>
                {
                    Some(result.ownership_continuous)
                }
                _ => None,
            },
            _ => None,
        }
    }

    async fn promote_committed(&mut self) -> bool {
        self.stop_renewal().await;
        match self.promote_once().await {
            Some(promotion_continuous) => promotion_continuous && *self.ownership_healthy.borrow(),
            None => {
                tracing::warn!(
                    client_id = %self.client_id,
                    purpose = ?self.purpose,
                    "committed exact-client dispatch fence remains in safe recovery state"
                );
                false
            }
        }
    }

    async fn compensate(&self, reason: &str) {
        let request = GatewayClientDispatchFenceClear {
            client_id: self.client_id.clone(),
            expected_token: self.token,
            gateway_epoch: self.gateway_epoch,
            expected_generation: self.generation,
            restore_fallback: true,
            reason: reason.to_string(),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS),
            self.state
                .gateway
                .clear_client_dispatch_fences(vec![request]),
        )
        .await;
        if !matches!(
            &result,
            Ok(Ok(batch))
                if matches!(batch.results.as_slice(), [result]
                    if result.client_id == self.client_id
                        && result.accepted)
        ) {
            tracing::warn!(
                client_id = %self.client_id,
                ?result,
                "temporary exact-client dispatch fence compensation was not acknowledged"
            );
        }
    }

    async fn clear_committed(&mut self, reason: &str) -> bool {
        self.stop_renewal().await;
        let request = GatewayClientDispatchFenceClear {
            client_id: self.client_id.clone(),
            expected_token: self.token,
            gateway_epoch: self.gateway_epoch,
            expected_generation: self.generation,
            restore_fallback: false,
            reason: reason.to_string(),
        };
        let result = tokio::time::timeout(
            Duration::from_secs(CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS),
            self.state
                .gateway
                .clear_client_dispatch_fences(vec![request]),
        )
        .await;
        let cleared = matches!(
            &result,
            Ok(Ok(batch))
                if matches!(batch.results.as_slice(), [result]
                    if result.client_id == self.client_id
                        && (result.accepted
                            || result.message == "dispatch_fence_generation_retired"
                            || result.message == "dispatch_fence_gateway_epoch_stale"))
        );
        if !cleared {
            tracing::warn!(
                client_id = %self.client_id,
                ?result,
                "committed exact-client dispatch fence clear was not acknowledged"
            );
        }
        cleared
    }

    async fn stop_renewal(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(renewer) = self.renewer.take() {
            let _ = renewer.await;
        }
    }
}

#[derive(Clone)]
struct ClientDispatchFenceCommitProof {
    state: AppState,
    client_id: String,
    token: uuid::Uuid,
    gateway_epoch: uuid::Uuid,
    generation: u64,
    purpose: GatewayClientDispatchFencePurpose,
    initially_protected: HashSet<uuid::Uuid>,
    ownership_healthy: tokio::sync::watch::Receiver<bool>,
}

impl ClientDispatchFenceCommitProof {
    async fn verify(self) -> anyhow::Result<()> {
        anyhow::ensure!(
            *self.ownership_healthy.borrow(),
            "gateway dispatch-fence ownership was previously lost"
        );
        let request = GatewayClientDispatchFencePrepare {
            client_id: self.client_id.clone(),
            token: self.token,
            gateway_epoch: self.gateway_epoch,
            generation: self.generation,
            renewal: true,
            lease_secs: CLIENT_LIFECYCLE_FENCE_LEASE_SECS,
            purpose: self.purpose,
        };
        let batch = tokio::time::timeout(
            Duration::from_secs(CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS),
            self.state
                .gateway
                .prepare_client_dispatch_fences(vec![request]),
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!("gateway dispatch-fence commit proof timed out: {error}")
        })??;
        let [result] = batch.results.as_slice() else {
            anyhow::bail!("gateway dispatch-fence commit proof invalid");
        };
        anyhow::ensure!(
            result.client_id == self.client_id
                && result.accepted
                && result.fenced
                && result.ownership_continuous
                && *self.ownership_healthy.borrow()
                && result
                    .enqueued_job_ids
                    .iter()
                    .all(|job_id| self.initially_protected.contains(job_id)),
            "gateway dispatch-fence ownership changed before commit"
        );
        Ok(())
    }
}

enum OwnedDeleteTargetOutcome {
    Applied {
        result: crate::model::DeleteAgentResult,
        gateway_outcome: crate::model::LifecycleOutcomeView,
    },
    Rejected {
        client_id: String,
        code: &'static str,
    },
}

async fn mutate_delete_agent_target_owned(
    state: AppState,
    operator: crate::model::AuthContext,
    client_id: String,
    reason: Option<String>,
) -> OwnedDeleteTargetOutcome {
    let (mut fence, _) = match ClientDispatchFenceLease::prepare(
        &state,
        &client_id,
        GatewayClientDispatchFencePurpose::Deletion,
        false,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            tracing::warn!(%client_id, %error, "VPS deletion dispatch fence rejected");
            return OwnedDeleteTargetOutcome::Rejected {
                client_id,
                code: "agent_delete_gateway_fence_conflict",
            };
        }
    };

    let commit_proof = fence.commit_proof();
    let outcome = state
        .repo
        .delete_agent_target(
            &client_id,
            reason.as_deref(),
            &operator,
            move || async move { commit_proof.verify().await },
        )
        .await;
    match outcome {
        Ok(crate::repository_inventory::DeleteAgentRepositoryOutcome::Applied(result)) => {
            let fence_owner = fence.owner();
            let continuity_preserved = fence.promote_committed().await;
            if !continuity_preserved {
                tracing::error!(
                    %client_id,
                    "committed VPS deletion fence recovered after lease continuity loss; expired dispatch attempts were returned to durable eligibility checks"
                );
            }

            // Promotion makes deletion authoritative before disconnect. The
            // persistent deletion fence also rejects a hello that the API
            // accepted before the deletion transaction committed.
            state.refresh_gateway_dispatch_timeouts();
            let disconnect = tokio::time::timeout(
                Duration::from_secs(CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS),
                state.gateway.disconnect_session_if_fence_owned(
                    &client_id,
                    "vps_deleted",
                    fence_owner,
                ),
            )
            .await;
            let gateway_outcome = match disconnect {
                Ok(Ok(disconnect)) if disconnect.client_id == client_id && disconnect.accepted => {
                    gateway_disconnect_outcome(Ok(()), &client_id, "VPS deletion")
                }
                Ok(Ok(_)) => gateway_disconnect_outcome(
                    Err(ApiError::conflict(
                        "gateway_session_disconnect_result_invalid",
                    )),
                    &client_id,
                    "VPS deletion",
                ),
                Ok(Err(error)) => {
                    tracing::warn!(%client_id, %error, "VPS deletion gateway disconnect failed");
                    gateway_disconnect_outcome(
                        Err(ApiError::conflict("gateway_session_disconnect_failed")),
                        &client_id,
                        "VPS deletion",
                    )
                }
                Err(error) => gateway_disconnect_outcome(
                    Err(ApiError::conflict("gateway_session_disconnect_timed_out")),
                    &client_id,
                    &format!("VPS deletion ({error})"),
                ),
            };
            OwnedDeleteTargetOutcome::Applied {
                result,
                gateway_outcome,
            }
        }
        Ok(crate::repository_inventory::DeleteAgentRepositoryOutcome::Rejected {
            client_id,
            code,
        }) => {
            fence.stop_renewal().await;
            fence.compensate("deletion_not_committed").await;
            OwnedDeleteTargetOutcome::Rejected { client_id, code }
        }
        Err(error) => {
            fence.stop_renewal().await;
            tracing::error!(%client_id, %error, "exact-client VPS deletion transaction failed");
            OwnedDeleteTargetOutcome::Rejected {
                client_id,
                code: "agent_delete_target_failed",
            }
        }
    }
}

async fn mutate_delete_agents(
    state: &AppState,
    operator: &crate::model::AuthContext,
    request: BulkDeleteAgentsRequest,
) -> Result<BulkDeleteAgentsResponse, ApiError> {
    validate_bulk_delete_agents_request(&request)?;
    let reason = canonical_lifecycle_reason(request.reason.as_deref());
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

    let mut deleted_by_client = HashMap::new();
    let mut affected_client_ids = Vec::new();
    let mut gateway_outcomes = HashMap::new();
    for client_id in approved_client_ids {
        // Each target owns its prepare -> database transaction -> durable
        // promotion -> disconnect sequence. No later target can consume an
        // earlier target's finite prepare lease or postpone its finalization.
        // Dropping the HTTP future detaches only this already-started target;
        // unstarted targets retain the usual request-cancellation semantics.
        let target = tokio::spawn(mutate_delete_agent_target_owned(
            state.clone(),
            operator.clone(),
            client_id.clone(),
            reason.clone(),
        ))
        .await
        .map_err(|error| {
            ApiError::internal(
                "agent_delete_lifecycle_owner_failed",
                "The exact VPS deletion lifecycle owner stopped unexpectedly.",
                error.into(),
            )
        })?;
        match target {
            OwnedDeleteTargetOutcome::Applied {
                result,
                gateway_outcome,
            } => {
                gateway_outcomes.insert(client_id.clone(), gateway_outcome);
                affected_client_ids.push(client_id.clone());
                deleted_by_client.insert(client_id, result);
            }
            OwnedDeleteTargetOutcome::Rejected { client_id, code } => {
                public_by_client
                    .insert(client_id.clone(), rejected_delete_outcome(client_id, code));
            }
        }
    }

    if !affected_client_ids.is_empty() {
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
        "agent_delete_gateway_fence_conflict" => {
            "The gateway could not establish exclusive dispatch ownership for this VPS."
        }
        "agent_delete_target_failed" => {
            "This VPS deletion transaction failed. Review its current state and retry this VPS; other reviewed targets were processed independently."
        }
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
        "agent_delete_gateway_fence_conflict" => {
            ApiError::conflict("agent_delete_gateway_fence_conflict")
        }
        "agent_delete_target_failed" => ApiError::internal(
            "agent_delete_target_failed",
            "The VPS deletion transaction failed. Review its current state and retry this VPS.",
            anyhow::anyhow!("exact-client VPS deletion transaction failed"),
        ),
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

async fn mutate_agent_suspension_target_owned(
    state: AppState,
    operator: crate::model::AuthContext,
    action: AgentSuspensionAction,
    client_id: String,
    reason: Option<String>,
) -> crate::repository_inventory::AgentSuspensionRepositoryOutcome {
    let mut fence = None;
    let mut protected_job_ids = Vec::new();
    if state.gateway.configured() {
        match ClientDispatchFenceLease::prepare(
            &state,
            &client_id,
            GatewayClientDispatchFencePurpose::Suspension,
            action == AgentSuspensionAction::Unsuspend,
        )
        .await
        {
            Ok((prepared, protected)) => {
                fence = Some(prepared);
                protected_job_ids = protected;
            }
            Err(error) => {
                tracing::warn!(%client_id, %error, "VPS suspension transition fence rejected");
                return crate::repository_inventory::AgentSuspensionRepositoryOutcome::Rejected {
                    client_id,
                    code: "agent_suspend_gateway_fence_conflict",
                };
            }
        }
    }

    let commit_proof = fence.as_ref().map(ClientDispatchFenceLease::commit_proof);
    let repository_outcome = state
        .repo
        .mutate_agent_suspension_target(
            action,
            &client_id,
            reason.as_deref(),
            &operator,
            &protected_job_ids,
            move || async move {
                if let Some(commit_proof) = commit_proof {
                    commit_proof.verify().await?;
                }
                Ok(())
            },
        )
        .await;
    let outcome = match repository_outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            if let Some(mut fence) = fence {
                // A repository error may follow either a confirmed rollback
                // or an uncertain commit. Keep the exact owner installed; its
                // finite lease becomes the existing per-request durable DB
                // recheck barrier without guessing either database outcome.
                fence.stop_renewal().await;
            }
            tracing::error!(%client_id, %error, "exact-client VPS suspension transaction failed");
            return crate::repository_inventory::AgentSuspensionRepositoryOutcome::Rejected {
                client_id,
                code: "agent_suspension_target_failed",
            };
        }
    };

    match &outcome {
        crate::repository_inventory::AgentSuspensionRepositoryOutcome::Applied {
            client_id,
            ..
        } => {
            if action == AgentSuspensionAction::Suspend {
                if let Some(mut fence) = fence {
                    let continuity_preserved = fence.promote_committed().await;
                    if !continuity_preserved {
                        tracing::error!(
                            %client_id,
                            "committed VPS suspension fence recovered after lease continuity loss; expired dispatch attempts were returned to durable eligibility checks"
                        );
                    }
                }
            } else if let Some(mut fence) = fence {
                fence.clear_committed("db_authoritative_unsuspend").await;
            }
        }
        crate::repository_inventory::AgentSuspensionRepositoryOutcome::Rejected {
            code, ..
        } if action == AgentSuspensionAction::Unsuspend && *code == "agent_not_suspended" => {
            // The exact transition owner prevented a newer suspension from
            // entering its database transaction. A no-op unsuspend is thus
            // authoritative evidence that its replaced fallback is obsolete.
            if let Some(mut fence) = fence {
                fence
                    .clear_committed("db_authoritative_unsuspend_noop")
                    .await;
            }
        }
        crate::repository_inventory::AgentSuspensionRepositoryOutcome::Rejected { .. } => {
            if let Some(mut fence) = fence {
                fence.stop_renewal().await;
                fence
                    .compensate("suspension_transition_not_committed")
                    .await;
            }
        }
    }
    outcome
}

async fn mutate_agent_suspensions(
    state: &AppState,
    operator: &crate::model::AuthContext,
    request: BulkAgentSuspensionRequest,
) -> Result<BulkAgentSuspensionResponse, ApiError> {
    validate_bulk_agent_suspension_request(&request)?;
    let reason = canonical_lifecycle_reason(request.reason.as_deref());
    let mut public_by_client = HashMap::new();
    let mut affected_client_ids = Vec::new();
    for client_id in &request.client_ids {
        let outcome = tokio::spawn(mutate_agent_suspension_target_owned(
            state.clone(),
            operator.clone(),
            request.action,
            client_id.clone(),
            reason.clone(),
        ))
        .await
        .map_err(|error| {
            ApiError::internal(
                "agent_suspension_lifecycle_owner_failed",
                "The exact VPS suspension lifecycle owner stopped unexpectedly.",
                error.into(),
            )
        })?;
        match outcome {
            crate::repository_inventory::AgentSuspensionRepositoryOutcome::Applied {
                client_id,
                agent,
                mutation,
            } => {
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
                public_by_client.insert(
                    client_id.clone(),
                    rejected_suspension_outcome(client_id, code),
                );
            }
        }
    }

    if !affected_client_ids.is_empty() {
        state.repo.wake_agent_suspension_consumers().await;
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
        "agent_suspension_target_failed" => {
            "This VPS suspension transaction failed. Review its current state and retry this VPS; other reviewed targets were processed independently."
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
        "agent_suspension_target_failed" => ApiError::internal(
            "agent_suspension_target_failed",
            "The VPS suspension transaction failed. Review its current state and retry this VPS.",
            anyhow::anyhow!("exact-client VPS suspension transaction failed"),
        ),
        _ => ApiError::conflict("agent_suspension_rejected"),
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

fn canonical_lifecycle_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(str::to_string)
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
        || request.client_ids.len() > GATEWAY_CLIENT_DISPATCH_FENCE_BATCH_MAX_ITEMS
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
