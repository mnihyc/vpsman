use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::{payload_hash, AgentPingProbeKind, AgentRuntimeConfig};

use crate::{
    error::ApiError,
    model::{
        AgentView, BillingPlanView, BulkPingTargetLifecycleRequest,
        BulkPingTargetLifecycleResponse, BulkResolveRequest,
        BulkUpdateMonitoringShareTargetsRequest, BulkUpdateMonitoringShareTargetsResponse,
        BulkUpdatePingTargetsRequest, BulkUpdatePingTargetsResponse, ClientMonitoringView,
        CreateMonitoringShareRequest, CreateMonitoringShareResponse, CurrentPingView,
        DeletePingTargetRequest, DeletePingTargetResponse, ExtendMonitoringSharesRequest,
        MakePrimaryPingTargetRequest, MonitoringCardView, MonitoringCardsPageView,
        MonitoringRangeView, MonitoringResolutionView, MonitoringShareDefinitionChangeView,
        MonitoringShareDefinitionUpdate, MonitoringShareListQuery, MonitoringShareRecord,
        MonitoringShareRevisionView, MonitoringShareTargetChangeView, MonitoringShareTargetRecord,
        MonitoringShareTargetReplacement, MonitoringShareUrlResponse, MonitoringShareView,
        MonitoringShareVisibilityRequest, MonitoringShareVisibilityView,
        MonitoringSharesMutationResponse, PingRollupView, PingTargetAssignmentChangeView,
        PingTargetAssignmentReplacement, PingTargetDetailView, PingTargetMutationRequest,
        PingTargetMutationResponse, PingTargetRecord, PingTargetRuntimeSyncView, PingTargetView,
        PortSpeedView, PublicBillingPlanView, PublicMonitoringCardView, PublicMonitoringDataView,
        PublicMonitoringDetailView, PublicMonitoringRangeView, PublicMonitoringShareBootstrapView,
        PublicMonitoringShareView, PublicNetworkMetricView, PublicNetworkPointView,
        PublicPingMetricView, PublicPingPointView, PublicPortSpeedView, PublicResourceMetricView,
        PublicSystemInformationView, PublicTrafficHistoryPointView, PublicTrafficMetricView,
        RevokeMonitoringSharesRequest, RuntimeConfigApplyStateRecord, SystemInformationView,
        TelemetryNetworkRateView, TelemetryRollupView, UpdateMonitoringShareRequest,
        UpdateMonitoringShareResponse,
    },
    model_alert_policies::TrafficAccountingRecord,
    model_alert_policies::{
        VPS_RULE_KEY_BILLING_CYCLE, VPS_RULE_KEY_BILLING_PRICE, VPS_RULE_KEY_NETWORK_PORT_SPEED,
        VPS_RULE_KEY_PRODUCT_NAME,
    },
    repository::Repository,
    repository_monitoring::monitoring_share_status,
    runtime_config::dispatch_runtime_config_for_clients,
    security::{
        generate_token, operator_has_scope, require_vps_rule_selector_scope, SCOPE_CONFIG_READ,
        SCOPE_FLEET_READ, SCOPE_NETWORK_READ, SCOPE_SHARING_READ, SCOPE_SHARING_WRITE,
    },
    selector_expression::parse_selector_expression,
    state::AppState,
};

const SHARE_TOKEN_HEADER: &str = "x-vpsman-share-token";
const SHARE_VISITOR_HEADER: &str = "x-vpsman-share-visitor";
const SHARE_VISITOR_COOKIE: &str = "vpsman_share_visitor";
const MIN_SHARE_EXPIRY_SECS: u64 = 60;
const MAX_SHARE_EXPIRY_SECS: u64 = 365 * 24 * 60 * 60;
const MAX_MONITORING_SELECTOR_BYTES: usize = 4_096;
const MAX_SHARE_SELECTOR_BYTES: usize = 65_535;
const MAX_SHARE_TARGETS: usize = 1_000;
const MAX_BULK_PING_TARGET_UPDATES: usize = 500;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct MonitoringCardsQuery {
    pub(crate) selector_expression: Option<String>,
    pub(crate) limit: Option<usize>,
    pub(crate) offset: Option<usize>,
    pub(crate) include_history: Option<bool>,
    pub(crate) history_mode: Option<MonitoringCardsHistoryMode>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MonitoringCardsHistoryMode {
    #[default]
    PerInterface,
    SelectedAggregate,
}

impl MonitoringCardsHistoryMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::PerInterface => "per_interface",
            Self::SelectedAggregate => "selected_aggregate",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ClientMonitoringQuery {
    pub(crate) window: Option<String>,
    pub(crate) start_unix: Option<u64>,
    pub(crate) end_unix: Option<u64>,
    pub(crate) points: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PublicMonitoringDataQuery {
    pub(crate) client_key: Option<String>,
    pub(crate) window: Option<String>,
    pub(crate) start_unix: Option<u64>,
    pub(crate) end_unix: Option<u64>,
    pub(crate) points: Option<i64>,
    pub(crate) limit: Option<usize>,
    pub(crate) offset: Option<usize>,
}

pub(crate) async fn list_monitoring_cards(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MonitoringCardsQuery>,
) -> Result<Json<MonitoringCardsPageView>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    let selector_expression = query
        .selector_expression
        .as_deref()
        .map(str::trim)
        .filter(|selector| !selector.is_empty())
        .map(str::to_string);
    if let Some(selector) = selector_expression.as_deref() {
        let expression = parse_selector_expression(selector)
            .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
            .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
        require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    }
    let limit = query.limit.unwrap_or(1_000).clamp(1, 1_000);
    let requested_offset = query.offset.unwrap_or(0);
    let include_history = query.include_history.unwrap_or(true);
    let history_mode = query.history_mode.unwrap_or_default();
    let key = serde_json::json!({
        "endpoint": "monitoring_cards",
        "auth": crate::state::read_singleflight_auth_key(
            operator.operator.id,
            &operator.operator.scopes,
        ),
        "selector_expression": selector_expression,
        "limit": limit,
        "offset": requested_offset,
        "include_history": include_history,
        "history_mode": history_mode.as_str(),
    })
    .to_string();
    let events = state.events.clone();
    let response = events
        .singleflight_monitoring_cards(key, move || async move {
            build_monitoring_cards_page(
                &state,
                selector_expression.as_deref(),
                limit,
                requested_offset,
                include_history,
                history_mode,
            )
            .await
        })
        .await?;
    Ok(Json(response))
}

async fn build_monitoring_cards_page(
    state: &AppState,
    selector_expression: Option<&str>,
    limit: usize,
    requested_offset: usize,
    include_history: bool,
    history_mode: MonitoringCardsHistoryMode,
) -> Result<MonitoringCardsPageView, ApiError> {
    let mut agents = monitoring_agents(state, selector_expression).await?;
    sort_monitoring_agents(&mut agents);
    let total = agents.len();
    let offset = requested_offset.min(total);
    let page = agents
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let items = monitoring_cards_for_agents_projection_with_history_mode(
        state,
        page,
        include_history,
        history_mode,
    )
    .await?;
    let consumed = offset.saturating_add(items.len());
    Ok(MonitoringCardsPageView {
        items,
        offset,
        limit,
        total,
        next_offset: (consumed < total).then_some(consumed),
    })
}

pub(crate) async fn get_client_monitoring(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Query(query): Query<ClientMonitoringQuery>,
) -> Result<Json<ClientMonitoringView>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    let key = serde_json::json!({
        "endpoint": "client_monitoring",
        "auth": crate::state::read_singleflight_auth_key(
            operator.operator.id,
            &operator.operator.scopes,
        ),
        "client_id": &client_id,
        "window": query.window.as_deref(),
        "start_unix": query.start_unix,
        "end_unix": query.end_unix,
        "points": query.points,
    })
    .to_string();
    let events = state.events.clone();
    let response = events
        .singleflight_client_monitoring(key, move || async move {
            client_monitoring_view(
                &state,
                &client_id,
                &query,
                CurrentNetworkDetail::SingleVpsDetail,
            )
            .await
        })
        .await?;
    Ok(Json(response))
}

pub(crate) async fn list_ping_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PingTargetView>>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_NETWORK_READ)
        .await?;
    let mut targets = state
        .repo
        .list_ping_targets()
        .await
        .map_err(ApiError::internal_mapper(
            "ping_targets_unavailable",
            "Ping targets could not be loaded.",
        ))?;
    enrich_ping_target_evidence(
        &state,
        &mut targets,
        operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
    )
    .await?;
    Ok(Json(targets))
}

pub(crate) async fn get_ping_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
) -> Result<Json<PingTargetDetailView>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_NETWORK_READ)
        .await?;
    let mut detail = state
        .repo
        .get_ping_target_detail(target_id)
        .await
        .map_err(ApiError::internal_mapper(
            "ping_target_unavailable",
            "The Ping target could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::not_found("ping_target_not_found"))?;
    enrich_ping_target_evidence(
        &state,
        std::slice::from_mut(&mut detail.target),
        operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
    )
    .await?;
    Ok(Json(detail))
}

pub(crate) async fn create_ping_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PingTargetMutationRequest>,
) -> Result<(StatusCode, Json<PingTargetMutationResponse>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    let expression = parse_selector_expression(&request.selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
    require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    let normalized = validate_ping_target_mutation(&state, &request, None).await?;
    if !request.confirmed {
        return Err(ApiError::bad_request("ping_target_confirmation_required"));
    }
    let now = crate::unix_now().to_string();
    let record = PingTargetRecord {
        id: Uuid::new_v4(),
        name: normalized.name,
        host: normalized.host,
        probe_kind: normalized.probe_kind,
        port: normalized.port,
        enabled: request.enabled,
        selector_expression: normalized.selector_expression,
        generation: 1,
        created_by: Some(operator.operator.id),
        created_at: now.clone(),
        updated_at: now,
    };
    let mut target = state
        .repo
        .upsert_ping_target(
            record,
            &normalized.target_client_ids,
            None,
            &operator,
            "ping_target.created",
        )
        .await
        .map_err(monitoring_repository_error)?;
    target.target.target_update_evidence_available = true;
    target.target.target_update_available = false;
    let runtime_sync = dispatch_runtime_config_for_clients(
        &state,
        &operator,
        normalized.target_client_ids,
        "ping_targets_updated",
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(PingTargetMutationResponse {
            target,
            runtime_sync,
        }),
    ))
}

pub(crate) async fn update_ping_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    Json(request): Json<PingTargetMutationRequest>,
) -> Result<Json<PingTargetMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    let existing = state
        .repo
        .ping_target_record(target_id)
        .await
        .map_err(ApiError::internal_mapper(
            "ping_target_unavailable",
            "The Ping target could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::not_found("ping_target_not_found"))?;
    let mut prior_assignments = state
        .repo
        .list_ping_target_assignment_records(Some(target_id))
        .await
        .map_err(ApiError::internal_mapper(
            "ping_target_assignments_unavailable",
            "Ping-target assignments could not be loaded.",
        ))?
        .into_iter()
        .map(|assignment| assignment.client_id)
        .collect::<Vec<_>>();
    prior_assignments.sort();
    prior_assignments.dedup();
    let selector_unchanged = request.selector_expression.trim() == existing.selector_expression;
    if !selector_unchanged {
        let expression = parse_selector_expression(&request.selector_expression)
            .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
            .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
        require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    }
    let normalized = validate_ping_target_mutation(
        &state,
        &request,
        selector_unchanged.then_some(prior_assignments.as_slice()),
    )
    .await?;
    if !request.confirmed {
        return Err(ApiError::bad_request("ping_target_confirmation_required"));
    }
    let probe_changed = existing.host != normalized.host
        || existing.probe_kind != normalized.probe_kind
        || existing.port != normalized.port
        || existing.enabled != request.enabled;
    let expected_target = existing.clone();
    let expectation = PingTargetAssignmentReplacement {
        expected_target,
        expected_client_ids: prior_assignments.clone(),
        next_client_ids: normalized.target_client_ids.clone(),
    };
    let record = PingTargetRecord {
        id: existing.id,
        name: normalized.name,
        host: normalized.host,
        probe_kind: normalized.probe_kind,
        port: normalized.port,
        enabled: request.enabled,
        selector_expression: normalized.selector_expression,
        generation: if probe_changed {
            existing.generation.saturating_add(1)
        } else {
            existing.generation
        },
        created_by: existing.created_by,
        created_at: existing.created_at,
        updated_at: crate::unix_now().to_string(),
    };
    let mut target = state
        .repo
        .upsert_ping_target(
            record,
            &normalized.target_client_ids,
            Some(&expectation),
            &operator,
            "ping_target.updated",
        )
        .await
        .map_err(monitoring_repository_error)?;
    if selector_unchanged {
        enrich_ping_target_mutation_evidence(
            &state,
            &mut target.target,
            operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
        )
        .await;
    } else {
        target.target.target_update_evidence_available = true;
        target.target.target_update_available = false;
    }
    let affected = prior_assignments
        .into_iter()
        .chain(normalized.target_client_ids)
        .collect::<BTreeSet<_>>();
    let runtime_sync =
        dispatch_runtime_config_for_clients(&state, &operator, affected, "ping_targets_updated")
            .await;
    Ok(Json(PingTargetMutationResponse {
        target,
        runtime_sync,
    }))
}

pub(crate) async fn bulk_update_ping_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkUpdatePingTargetsRequest>,
) -> Result<Json<BulkUpdatePingTargetsResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    validate_bulk_ping_target_selection(&request.target_ids)?;
    let target_ids = request
        .target_ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let assignments = state
        .repo
        .list_ping_target_assignment_records_for_targets(&target_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "ping_target_assignments_unavailable",
            "Ping-target assignments could not be loaded.",
        ))?;
    let targets = state
        .repo
        .ping_target_records_by_ids(&target_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "ping_target_unavailable",
            "Ping targets could not be loaded.",
        ))?
        .into_iter()
        .map(|target| (target.id, target))
        .collect::<HashMap<_, _>>();
    if targets.len() != target_ids.len() {
        return Err(ApiError::not_found("ping_target_not_found"));
    }
    let mut selectors = BTreeSet::new();
    for target_id in &target_ids {
        let target = targets
            .get(target_id)
            .expect("every requested Ping target was loaded");
        validate_monitoring_selector(&target.selector_expression, MAX_MONITORING_SELECTOR_BYTES)?;
        require_monitoring_selector_scope(&operator.operator.scopes, &target.selector_expression)?;
        selectors.insert(target.selector_expression.clone());
    }
    let selector_evidence =
        resolve_saved_selectors_batch(&state, &selectors, MAX_MONITORING_SELECTOR_BYTES, true)
            .await;
    if selectors.iter().any(|selector| {
        !selector_evidence
            .evidence_available
            .get(selector)
            .copied()
            .unwrap_or(false)
    }) {
        return Err(ApiError::internal(
            "selector_targets_unavailable",
            "Selector targets could not be resolved.",
            anyhow::anyhow!("saved Ping-target selector evidence unavailable"),
        ));
    }
    let mut replacements = Vec::new();
    let mut changes = Vec::new();
    for target_id in &target_ids {
        let target = targets
            .get(target_id)
            .cloned()
            .expect("every requested Ping target was loaded");
        let resolved = selector_evidence
            .resolved_client_ids
            .get(&target.selector_expression)
            .cloned()
            .expect("every validated Ping selector was resolved");
        let current = assignments
            .iter()
            .filter(|assignment| assignment.target_id == *target_id)
            .map(|assignment| assignment.client_id.clone())
            .collect::<BTreeSet<_>>();
        let next = resolved.iter().cloned().collect::<BTreeSet<_>>();
        changes.push(PingTargetAssignmentChangeView {
            target_id: *target_id,
            target_name: target.name.clone(),
            selector_expression: target.selector_expression.clone(),
            added_client_ids: next.difference(&current).cloned().collect(),
            removed_client_ids: current.difference(&next).cloned().collect(),
            unchanged_count: current.intersection(&next).count(),
        });
        replacements.push(PingTargetAssignmentReplacement {
            expected_target: target,
            expected_client_ids: current.into_iter().collect(),
            next_client_ids: resolved,
        });
    }
    let preview_hash = ping_assignment_preview_hash(&changes)?;
    if !request.confirmed {
        return Ok(Json(BulkUpdatePingTargetsResponse {
            preview_hash,
            applied: false,
            changes,
            runtime_sync: Vec::new(),
        }));
    }
    if request.preview_hash.as_deref() != Some(preview_hash.as_str()) {
        return Err(ApiError::conflict("ping_target_preview_stale"));
    }
    let affected = state
        .repo
        .replace_ping_target_assignments_bulk(&replacements, &operator)
        .await
        .map_err(monitoring_repository_error)?;
    let runtime_sync =
        dispatch_runtime_config_for_clients(&state, &operator, affected, "ping_targets_updated")
            .await;
    Ok(Json(BulkUpdatePingTargetsResponse {
        preview_hash,
        applied: true,
        changes,
        runtime_sync,
    }))
}

pub(crate) fn validate_bulk_ping_target_selection(target_ids: &[Uuid]) -> Result<(), ApiError> {
    if target_ids.is_empty() {
        return Err(ApiError::bad_request("ping_target_selection_required"));
    }
    if target_ids.len() > MAX_BULK_PING_TARGET_UPDATES {
        return Err(ApiError::bad_request("ping_target_selection_too_large"));
    }
    Ok(())
}

pub(crate) async fn make_primary_ping_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    Json(request): Json<MakePrimaryPingTargetRequest>,
) -> Result<Json<PingTargetMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    let mut target = state
        .repo
        .make_primary_ping_target(target_id, &request.client_ids, &operator)
        .await
        .map_err(monitoring_repository_error)?;
    enrich_ping_target_mutation_evidence(
        &state,
        &mut target.target,
        operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
    )
    .await;
    Ok(Json(PingTargetMutationResponse {
        target,
        runtime_sync: Vec::new(),
    }))
}

pub(crate) async fn delete_ping_target(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(target_id): Path<Uuid>,
    Json(request): Json<DeletePingTargetRequest>,
) -> Result<Json<DeletePingTargetResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "ping_target_delete_confirmation_required",
        ));
    }
    let affected = state
        .repo
        .delete_ping_target(target_id, &operator)
        .await
        .map_err(monitoring_repository_error)?;
    let runtime_sync =
        dispatch_runtime_config_for_clients(&state, &operator, affected, "ping_targets_updated")
            .await;
    Ok(Json(DeletePingTargetResponse { runtime_sync }))
}

pub(crate) async fn bulk_ping_target_lifecycle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkPingTargetLifecycleRequest>,
) -> Result<Json<BulkPingTargetLifecycleResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "network:write")
        .await?;
    let target_ids = request
        .target_ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if target_ids.is_empty() {
        return Err(ApiError::bad_request("ping_target_selection_required"));
    }
    let action = request.action.trim().to_lowercase();
    if !matches!(action.as_str(), "enable" | "disable" | "delete") {
        return Err(ApiError::bad_request(
            "ping_target_lifecycle_action_invalid",
        ));
    }
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "ping_target_lifecycle_confirmation_required",
        ));
    }
    let affected_clients = state
        .repo
        .mutate_ping_targets_bulk(&target_ids, &action, &operator)
        .await
        .map_err(monitoring_repository_error)?;
    let runtime_sync = dispatch_runtime_config_for_clients(
        &state,
        &operator,
        affected_clients,
        "ping_targets_updated",
    )
    .await;
    Ok(Json(BulkPingTargetLifecycleResponse {
        action,
        affected_target_ids: target_ids,
        runtime_sync,
    }))
}

pub(crate) async fn list_monitoring_shares(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MonitoringShareListQuery>,
) -> Result<Json<Vec<MonitoringShareView>>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_SHARING_READ)
        .await?;
    if query
        .status
        .as_deref()
        .is_some_and(|status| !matches!(status, "active" | "expired" | "revoked"))
    {
        return Err(ApiError::bad_request("monitoring_share_status_invalid"));
    }
    let mut shares = state
        .repo
        .list_monitoring_shares(
            query.status.as_deref(),
            query.limit.unwrap_or(100),
            query.offset.unwrap_or(0),
        )
        .await
        .map_err(ApiError::internal_mapper(
            "monitoring_shares_unavailable",
            "Shared monitoring views could not be loaded.",
        ))?;
    enrich_monitoring_share_target_evidence(
        &state,
        &mut shares,
        operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
    )
    .await?;
    Ok(Json(shares))
}

pub(crate) async fn create_monitoring_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateMonitoringShareRequest>,
) -> Result<Response, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_SHARING_WRITE)
        .await?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "monitoring_share_confirmation_required",
        ));
    }
    let name = request.name.trim().to_string();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("monitoring_share_name_invalid"));
    }
    if !(MIN_SHARE_EXPIRY_SECS..=MAX_SHARE_EXPIRY_SECS).contains(&request.expires_in_secs) {
        return Err(ApiError::bad_request("monitoring_share_expiry_invalid"));
    }
    let selector_expression = request.selector_expression.trim().to_string();
    validate_monitoring_selector(&selector_expression, MAX_SHARE_SELECTOR_BYTES)?;
    parse_selector_expression(&selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
    let expression = parse_selector_expression(&selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
    require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    let resolved = resolve_selector(&state, &selector_expression, MAX_SHARE_SELECTOR_BYTES).await?;
    validate_monitoring_share_targets(&resolved)?;
    let submitted = request
        .target_client_ids
        .iter()
        .map(|client_id| client_id.trim())
        .filter(|client_id| !client_id.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if resolved != submitted {
        return Err(ApiError::conflict("monitoring_share_resolution_stale"));
    }
    let visibility = normalized_monitoring_share_visibility(&request.visibility)?;
    let secret = generate_token();
    let now = crate::unix_now();
    let id = Uuid::new_v4();
    let record = MonitoringShareRecord {
        id,
        name,
        token_secret: secret.clone(),
        selector_expression,
        targets: resolved
            .into_iter()
            .map(|client_id| MonitoringShareTargetRecord {
                client_id,
                public_client_key: generate_token(),
            })
            .collect(),
        visibility,
        expires_at: now.saturating_add(request.expires_in_secs).to_string(),
        revoked_at: None,
        created_at: now.to_string(),
        updated_at: now.to_string(),
    };
    let mut share = state
        .repo
        .create_monitoring_share(record, &operator)
        .await
        .map_err(share_repository_error)?;
    share.target_update_evidence_available = true;
    share.target_update_available = false;
    let mut response = (
        StatusCode::CREATED,
        Json(CreateMonitoringShareResponse {
            share,
            fragment_path: format!("#/share/{id}/{secret}"),
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub(crate) async fn update_monitoring_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(share_id): Path<Uuid>,
    Json(request): Json<UpdateMonitoringShareRequest>,
) -> Result<Json<UpdateMonitoringShareResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_SHARING_WRITE)
        .await?;
    let expected_updated_at = request.expected_updated_at.trim();
    if expected_updated_at.is_empty() {
        return Err(ApiError::bad_request(
            "monitoring_share_expected_revision_required",
        ));
    }
    let current = state
        .repo
        .monitoring_share_record(share_id)
        .await
        .map_err(ApiError::internal_mapper(
            "monitoring_share_unavailable",
            "The shared monitoring view could not be loaded.",
        ))?
        .ok_or_else(|| ApiError::not_found("monitoring_share_not_found"))?;
    if monitoring_share_status(&current, crate::unix_now()) != "active" {
        return Err(ApiError::conflict("monitoring_share_not_active"));
    }
    if current.updated_at != expected_updated_at {
        return Err(ApiError::conflict("monitoring_share_preview_stale"));
    }

    let name = request.name.trim().to_string();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("monitoring_share_name_invalid"));
    }
    let selector_expression = request.selector_expression.trim().to_string();
    validate_monitoring_selector(&selector_expression, MAX_SHARE_SELECTOR_BYTES)?;
    require_monitoring_selector_scope(&operator.operator.scopes, &selector_expression)?;
    let resolved = resolve_selector(&state, &selector_expression, MAX_SHARE_SELECTOR_BYTES).await?;
    validate_monitoring_share_targets(&resolved)?;
    let submitted = request
        .target_client_ids
        .iter()
        .map(|client_id| client_id.trim())
        .filter(|client_id| !client_id.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if resolved != submitted {
        return Err(ApiError::conflict("monitoring_share_resolution_stale"));
    }
    let visibility = normalized_monitoring_share_visibility(&request.visibility)?;
    let current_targets = current
        .target_client_ids()
        .into_iter()
        .collect::<BTreeSet<_>>();
    let next_targets = resolved.iter().cloned().collect::<BTreeSet<_>>();
    let change = MonitoringShareDefinitionChangeView {
        share_id,
        previous_name: current.name.clone(),
        name: name.clone(),
        previous_selector_expression: current.selector_expression.clone(),
        selector_expression: selector_expression.clone(),
        added_client_ids: next_targets.difference(&current_targets).cloned().collect(),
        removed_client_ids: current_targets.difference(&next_targets).cloned().collect(),
        unchanged_count: current_targets.intersection(&next_targets).count(),
        previous_visibility: current.visibility.clone(),
        visibility: visibility.clone(),
    };
    let preview_hash = monitoring_share_definition_preview_hash(&change, &current.updated_at)?;
    if !request.confirmed {
        return Ok(Json(UpdateMonitoringShareResponse {
            preview_hash,
            applied: false,
            change,
            share: None,
        }));
    }
    if request.preview_hash.as_deref() != Some(preview_hash.as_str()) {
        return Err(ApiError::conflict("monitoring_share_preview_stale"));
    }

    let mut share = state
        .repo
        .update_monitoring_share_definition(
            MonitoringShareDefinitionUpdate {
                expected_share: current,
                next_name: name,
                next_selector_expression: selector_expression,
                next_client_ids: resolved,
                next_visibility: visibility,
            },
            &operator,
        )
        .await
        .map_err(share_repository_error)?;
    enrich_monitoring_share_target_evidence(
        &state,
        std::slice::from_mut(&mut share),
        operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
    )
    .await?;
    Ok(Json(UpdateMonitoringShareResponse {
        preview_hash,
        applied: true,
        change,
        share: Some(share),
    }))
}

pub(crate) async fn extend_monitoring_shares(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ExtendMonitoringSharesRequest>,
) -> Result<Json<MonitoringSharesMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_SHARING_WRITE)
        .await?;
    if !(MIN_SHARE_EXPIRY_SECS..=MAX_SHARE_EXPIRY_SECS).contains(&request.extend_by_secs) {
        return Err(ApiError::bad_request("monitoring_share_extension_invalid"));
    }
    let mut shares = state
        .repo
        .extend_monitoring_shares(&request.share_ids, request.extend_by_secs, &operator)
        .await
        .map_err(share_repository_error)?;
    enrich_monitoring_share_target_evidence(
        &state,
        &mut shares,
        operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
    )
    .await?;
    Ok(Json(MonitoringSharesMutationResponse { shares }))
}

pub(crate) async fn get_monitoring_share_url(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(share_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_SHARING_WRITE)
        .await?;
    let token_secret = state
        .repo
        .recover_monitoring_share_url(share_id, &operator)
        .await
        .map_err(share_repository_error)?;
    let mut response = Json(MonitoringShareUrlResponse {
        fragment_path: format!("#/share/{share_id}/{token_secret}"),
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub(crate) async fn bulk_update_monitoring_share_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkUpdateMonitoringShareTargetsRequest>,
) -> Result<Json<BulkUpdateMonitoringShareTargetsResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_SHARING_WRITE)
        .await?;
    let share_ids = request
        .share_ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if share_ids.is_empty() {
        return Err(ApiError::bad_request("monitoring_share_selection_required"));
    }
    if share_ids.len() > 1_000 {
        return Err(ApiError::bad_request(
            "monitoring_share_selection_too_large",
        ));
    }
    let shares = state
        .repo
        .monitoring_share_records_by_ids(&share_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "monitoring_share_unavailable",
            "The shared monitoring views could not be loaded.",
        ))?;
    if shares.len() != share_ids.len() {
        return Err(ApiError::not_found("monitoring_share_not_found"));
    }
    let shares = shares
        .into_iter()
        .map(|share| (share.id, share))
        .collect::<HashMap<_, _>>();
    let mut selectors = BTreeSet::new();
    for share_id in &share_ids {
        let share = shares
            .get(share_id)
            .expect("every requested monitoring share was loaded");
        if monitoring_share_status(share, crate::unix_now()) != "active" {
            return Err(ApiError::conflict("monitoring_share_not_active"));
        }
        validate_monitoring_selector(&share.selector_expression, MAX_SHARE_SELECTOR_BYTES)?;
        require_monitoring_selector_scope(&operator.operator.scopes, &share.selector_expression)?;
        selectors.insert(share.selector_expression.clone());
    }
    let selector_evidence =
        resolve_saved_selectors_batch(&state, &selectors, MAX_SHARE_SELECTOR_BYTES, true).await;
    if selectors.iter().any(|selector| {
        !selector_evidence
            .evidence_available
            .get(selector)
            .copied()
            .unwrap_or(false)
    }) {
        return Err(ApiError::internal(
            "selector_targets_unavailable",
            "Selector targets could not be resolved.",
            anyhow::anyhow!("saved monitoring-share selector evidence unavailable"),
        ));
    }
    let mut changes = Vec::with_capacity(share_ids.len());
    let mut replacements = Vec::with_capacity(share_ids.len());
    for share_id in &share_ids {
        let share = shares
            .get(share_id)
            .cloned()
            .expect("every requested monitoring share was loaded");
        let resolved = selector_evidence
            .resolved_client_ids
            .get(&share.selector_expression)
            .cloned()
            .expect("every validated monitoring-share selector was resolved");
        validate_monitoring_share_targets(&resolved)?;
        let current = share
            .target_client_ids()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let next = resolved.iter().cloned().collect::<BTreeSet<_>>();
        changes.push(MonitoringShareTargetChangeView {
            share_id: *share_id,
            share_name: share.name.clone(),
            selector_expression: share.selector_expression.clone(),
            added_client_ids: next.difference(&current).cloned().collect(),
            removed_client_ids: current.difference(&next).cloned().collect(),
            unchanged_count: current.intersection(&next).count(),
        });
        replacements.push(MonitoringShareTargetReplacement {
            expected_share: share,
            next_client_ids: resolved,
        });
    }
    let preview_hash = monitoring_share_target_preview_hash(&changes)?;
    if !request.confirmed {
        return Ok(Json(BulkUpdateMonitoringShareTargetsResponse {
            preview_hash,
            applied: false,
            changes,
            revisions: Vec::new(),
        }));
    }
    if request.preview_hash.as_deref() != Some(preview_hash.as_str()) {
        return Err(ApiError::conflict("monitoring_share_preview_stale"));
    }
    let committed = state
        .repo
        .replace_monitoring_share_targets_bulk(&replacements, &operator)
        .await
        .map_err(share_repository_error)?;
    let revisions = committed
        .into_iter()
        .map(|revision| {
            let mut target_client_ids = revision.target_client_ids;
            target_client_ids.sort();
            target_client_ids.dedup();
            MonitoringShareRevisionView {
                share_id: revision.share_id,
                updated_at: revision.updated_at,
                target_count: target_client_ids.len(),
                target_client_ids,
                // The selector was resolved successfully as part of this request,
                // so the committed frozen target set is current and has usable
                // refresh evidence. Inactive status is still safe to report as
                // evidence-available because its retained frozen targets remain
                // immutable evidence.
                target_update_available: false,
                target_update_evidence_available: true,
            }
        })
        .collect();
    Ok(Json(BulkUpdateMonitoringShareTargetsResponse {
        preview_hash,
        applied: true,
        changes,
        revisions,
    }))
}

pub(crate) async fn revoke_monitoring_shares(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RevokeMonitoringSharesRequest>,
) -> Result<Json<MonitoringSharesMutationResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_SHARING_WRITE)
        .await?;
    let mut shares = state
        .repo
        .revoke_monitoring_shares(&request.share_ids, &operator)
        .await
        .map_err(share_repository_error)?;
    enrich_monitoring_share_target_evidence(
        &state,
        &mut shares,
        operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
    )
    .await?;
    Ok(Json(MonitoringSharesMutationResponse { shares }))
}

pub(crate) async fn public_monitoring_share_bootstrap(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(share_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let secret = share_secret(&headers)?;
    let share = state
        .repo
        .authenticate_monitoring_share(share_id, secret)
        .await
        .map_err(ApiError::internal_mapper(
            "monitoring_share_authentication_unavailable",
            "The shared monitoring view could not be authenticated.",
        ))?
        .ok_or_else(|| ApiError::not_found("monitoring_share_not_found"))?;
    if monitoring_share_status(&share, crate::unix_now()) != "active" {
        return Err(ApiError::gone("monitoring_share_unavailable"));
    }
    let proposed_visitor_id =
        cookie_value(&headers, SHARE_VISITOR_COOKIE).and_then(|value| Uuid::parse_str(value).ok());
    let source_ip = state.operator_client_ip(peer, &headers);
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok());
    let target_client_ids = share.target_client_ids();
    let visible_target_count = state
        .repo
        .list_agents_for_client_ids(&target_client_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "monitoring_share_targets_unavailable",
            "Shared-view VPS targets could not be loaded.",
        ))?
        .into_iter()
        .filter(AgentView::is_monitoring_visible)
        .count();
    let (visitor_id, _) = state
        .repo
        .record_monitoring_share_visitor(&share, proposed_visitor_id, &source_ip, user_agent)
        .await
        .map_err(ApiError::internal_mapper(
            "monitoring_share_visitor_record_failed",
            "Shared-view access could not be recorded.",
        ))?;
    let mut response = Json(PublicMonitoringShareBootstrapView {
        share: public_monitoring_share(&share, visible_target_count),
        visitor_id,
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    let cookie = format!(
        "{SHARE_VISITOR_COOKIE}={visitor_id}; Max-Age=31536000; Path=/api/v1/public/monitoring-shares; HttpOnly; Secure; SameSite=Lax"
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    Ok(response)
}

pub(crate) async fn public_monitoring_share_data(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(share_id): Path<Uuid>,
    Query(query): Query<PublicMonitoringDataQuery>,
) -> Result<Response, ApiError> {
    let secret = share_secret(&headers)?;
    let share = state
        .repo
        .authenticate_monitoring_share(share_id, secret)
        .await
        .map_err(ApiError::internal_mapper(
            "monitoring_share_authentication_unavailable",
            "The shared monitoring view could not be authenticated.",
        ))?
        .ok_or_else(|| ApiError::not_found("monitoring_share_not_found"))?;
    if monitoring_share_status(&share, crate::unix_now()) != "active" {
        return Err(ApiError::gone("monitoring_share_unavailable"));
    }
    let visitor_id = share_visitor_id(&headers)?;
    if !state
        .repo
        .touch_monitoring_share_visitor(share.id, visitor_id)
        .await
        .map_err(ApiError::internal_mapper(
            "monitoring_share_visitor_update_failed",
            "Shared-view access evidence could not be updated.",
        ))?
    {
        return Err(ApiError::unauthorized(
            "monitoring_share_visitor_bootstrap_required",
        ));
    }
    let target_client_ids = share.target_client_ids();
    let mut agents = state
        .repo
        .list_agents_for_client_ids(&target_client_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "monitoring_share_targets_unavailable",
            "Shared-view VPS targets could not be loaded.",
        ))?;
    agents.retain(AgentView::is_monitoring_visible);
    sort_monitoring_agents(&mut agents);
    let total = agents.len();
    let offset = query.offset.unwrap_or(0).min(total);
    let limit = query.limit.unwrap_or(1_000).clamp(1, 1_000);
    let page_agents = agents
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let card_key =
        public_monitoring_cards_singleflight_key(&share, &page_agents, offset, limit, total);
    let events = state.events.clone();
    let card_state = state.clone();
    let card_page = events
        .singleflight_monitoring_cards(card_key, move || async move {
            let items = monitoring_cards_for_agents(&card_state, page_agents).await?;
            let consumed = offset.saturating_add(items.len());
            Ok(MonitoringCardsPageView {
                items,
                offset,
                limit,
                total,
                next_offset: (consumed < total).then_some(consumed),
            })
        })
        .await?;
    let cards = card_page
        .items
        .into_iter()
        .map(|card| public_monitoring_card(card, &share))
        .collect::<Result<Vec<_>, _>>()?;
    let consumed = offset.saturating_add(cards.len());
    let detail = match query.client_key.as_deref() {
        None => None,
        Some(client_key) => {
            if !share.visibility.detail_history {
                return Err(ApiError::forbidden(
                    "monitoring_share_detail_history_not_allowed",
                ));
            }
            let client_id = share
                .client_id_for_public_key(client_key)
                .ok_or_else(|| ApiError::not_found("monitoring_share_client_not_found"))?
                .to_string();
            let range_query = ClientMonitoringQuery {
                window: query.window.clone(),
                start_unix: query.start_unix,
                end_unix: query.end_unix,
                points: query.points,
            };
            let key = public_monitoring_detail_singleflight_key(
                &share,
                &client_id,
                client_key,
                &range_query,
            );
            let events = state.events.clone();
            let detail_view = events
                .singleflight_client_monitoring(key, move || async move {
                    client_monitoring_view(
                        &state,
                        &client_id,
                        &range_query,
                        CurrentNetworkDetail::Hidden,
                    )
                    .await
                })
                .await?;
            Some(public_monitoring_detail(detail_view, &share)?)
        }
    };
    let mut response = Json(PublicMonitoringDataView {
        share: public_monitoring_share(&share, total),
        cards,
        offset,
        total,
        next_offset: (consumed < total).then_some(consumed),
        detail,
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn public_monitoring_cards_singleflight_key(
    share: &MonitoringShareRecord,
    page_agents: &[AgentView],
    offset: usize,
    limit: usize,
    total: usize,
) -> String {
    serde_json::json!({
        "endpoint": "public_monitoring_cards",
        "share_id": share.id,
        // Definition, target, visibility, extension and revocation mutations
        // all advance this revision monotonically in both repositories.
        "share_revision": &share.updated_at,
        "client_ids": page_agents
            .iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>(),
        "offset": offset,
        "limit": limit,
        // A visibility change outside this page can leave its client IDs
        // unchanged while changing pagination metadata returned with it.
        "total": total,
    })
    .to_string()
}

fn public_monitoring_detail_singleflight_key(
    share: &MonitoringShareRecord,
    client_id: &str,
    client_key: &str,
    range_query: &ClientMonitoringQuery,
) -> String {
    serde_json::json!({
        "endpoint": "public_monitoring_detail",
        "share_id": share.id,
        "share_revision": &share.updated_at,
        "client_id": client_id,
        "client_key": client_key,
        "window": range_query.window.as_deref(),
        "start_unix": range_query.start_unix,
        "end_unix": range_query.end_unix,
        "points": range_query.points,
    })
    .to_string()
}

async fn monitoring_agents(
    state: &AppState,
    selector_expression: Option<&str>,
) -> Result<Vec<AgentView>, ApiError> {
    let selector_expression = selector_expression
        .map(str::trim)
        .filter(|selector| !selector.is_empty());
    let mut agents = if let Some(selector_expression) = selector_expression {
        let client_ids =
            resolve_selector(state, selector_expression, MAX_MONITORING_SELECTOR_BYTES).await?;
        state
            .repo
            .list_agents_for_client_ids(&client_ids)
            .await
            .map_err(ApiError::internal_mapper(
                "vps_inventory_unavailable",
                "The VPS inventory could not be loaded.",
            ))?
    } else {
        state
            .repo
            .list_agents()
            .await
            .map_err(ApiError::internal_mapper(
                "vps_inventory_unavailable",
                "The VPS inventory could not be loaded.",
            ))?
    };
    agents.retain(AgentView::is_monitoring_visible);
    Ok(agents)
}

fn sort_monitoring_agents(agents: &mut [AgentView]) {
    agents.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
}

pub(crate) async fn monitoring_cards_for_agents(
    state: &AppState,
    agents: Vec<AgentView>,
) -> Result<Vec<MonitoringCardView>, ApiError> {
    // Public cards expose one client-aggregate network series, so aggregate in
    // PostgreSQL instead of transferring per-interface rows only to fold them
    // immediately in `public_network_points`.
    monitoring_cards_for_agents_projection_with_history_mode(
        state,
        agents,
        true,
        MonitoringCardsHistoryMode::SelectedAggregate,
    )
    .await
}

pub(crate) async fn monitoring_cards_for_agents_projection(
    state: &AppState,
    agents: Vec<AgentView>,
    include_history: bool,
) -> Result<Vec<MonitoringCardView>, ApiError> {
    monitoring_cards_for_agents_projection_with_history_mode(
        state,
        agents,
        include_history,
        MonitoringCardsHistoryMode::PerInterface,
    )
    .await
}

async fn monitoring_cards_for_agents_projection_with_history_mode(
    state: &AppState,
    mut agents: Vec<AgentView>,
    include_history: bool,
    history_mode: MonitoringCardsHistoryMode,
) -> Result<Vec<MonitoringCardView>, ApiError> {
    agents.retain(AgentView::is_monitoring_visible);
    if agents.is_empty() {
        return Ok(Vec::new());
    }
    let client_ids = agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect::<Vec<_>>();
    let rules = state
        .repo
        .list_all_vps_rules_for_clients(&client_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "vps_rules_unavailable",
            "VPS display rules could not be loaded.",
        ))?;
    let network_rate_selection = state
        .repo
        .network_rate_interface_selection_from_rules(&client_ids, &rules)
        .await
        .map_err(ApiError::internal_mapper(
            "network_interface_selection_unavailable",
            "Network-interface selection could not be loaded.",
        ))?;
    let history_end = crate::unix_now();
    let history_start = history_end.saturating_sub(15 * 60);
    let (
        system_information,
        resources,
        projection_pending,
        resource_history_rows,
        network_rows,
        network_history_rows,
        traffic,
        primary_ping,
        primary_ping_history_rows,
    ) = tokio::join!(
        state
            .repo
            .monitoring_system_information_for_clients(&client_ids),
        state
            .repo
            .list_latest_telemetry_rollups_for_clients(&client_ids, None),
        state
            .repo
            .telemetry_projection_pending_for_clients(&client_ids),
        async {
            if include_history {
                state
                    .repo
                    .list_dashboard_raw_telemetry_rollups(
                        16,
                        history_start,
                        history_end,
                        60,
                        &client_ids,
                    )
                    .await
            } else {
                Ok(Vec::new())
            }
        },
        state
            .repo
            .list_latest_telemetry_network_rates_for_selection(&network_rate_selection),
        async {
            if include_history {
                match history_mode {
                    MonitoringCardsHistoryMode::PerInterface => {
                        state
                            .repo
                            .list_dashboard_raw_telemetry_network_rates_selected(
                                16,
                                history_start,
                                history_end,
                                60,
                                &network_rate_selection,
                            )
                            .await
                    }
                    MonitoringCardsHistoryMode::SelectedAggregate => {
                        state
                            .repo
                            .list_monitoring_card_raw_network_history_selected(
                                16,
                                history_start,
                                history_end,
                                60,
                                &network_rate_selection,
                            )
                            .await
                    }
                }
            } else {
                Ok(Vec::new())
            }
        },
        state
            .repo
            .list_traffic_accounting_for_agents_with_rules(&agents, &rules),
        state.repo.current_primary_ping_for_clients(&client_ids),
        async {
            if include_history {
                state
                    .repo
                    .list_raw_primary_ping_results_for_clients(
                        &client_ids,
                        history_start,
                        history_end,
                        16,
                        60,
                    )
                    .await
            } else {
                Ok(Vec::new())
            }
        },
    );
    let mut system_information = system_information.map_err(ApiError::internal_mapper(
        "monitoring_system_information_unavailable",
        "VPS system information could not be loaded.",
    ))?;
    let resources = resources
        .map_err(ApiError::internal_mapper(
            "monitoring_resources_unavailable",
            "VPS resource metrics could not be loaded.",
        ))?
        .into_iter()
        .map(|row| (row.client_id.clone(), row))
        .collect::<HashMap<_, _>>();
    let projection_pending = projection_pending.map_err(ApiError::internal_mapper(
        "monitoring_projection_pending_unavailable",
        "VPS telemetry projection status could not be loaded.",
    ))?;
    let mut resource_history = HashMap::<String, Vec<TelemetryRollupView>>::new();
    for row in resource_history_rows.map_err(ApiError::internal_mapper(
        "monitoring_resource_history_unavailable",
        "VPS resource history could not be loaded.",
    ))? {
        resource_history
            .entry(row.client_id.clone())
            .or_default()
            .push(row);
    }
    let mut network = HashMap::<String, Vec<TelemetryNetworkRateView>>::new();
    for row in network_rows.map_err(ApiError::internal_mapper(
        "monitoring_network_rates_unavailable",
        "VPS network rates could not be loaded.",
    ))? {
        network.entry(row.client_id.clone()).or_default().push(row);
    }
    let mut network_history = HashMap::<String, Vec<TelemetryNetworkRateView>>::new();
    for row in network_history_rows.map_err(ApiError::internal_mapper(
        "monitoring_network_history_unavailable",
        "VPS network history could not be loaded.",
    ))? {
        network_history
            .entry(row.client_id.clone())
            .or_default()
            .push(row);
    }
    let traffic = traffic
        .map_err(ApiError::internal_mapper(
            "traffic_accounting_unavailable",
            "Traffic accounting could not be loaded.",
        ))?
        .into_iter()
        .map(|row| (row.client_id.clone(), row))
        .collect::<HashMap<_, _>>();
    let mut billing_rules = HashMap::<String, HashMap<String, serde_json::Value>>::new();
    let mut port_speeds = HashMap::<String, PortSpeedView>::new();
    let mut product_names = HashMap::<String, String>::new();
    for row in rules.into_iter().filter(|row| {
        matches!(
            row.key.as_str(),
            VPS_RULE_KEY_BILLING_PRICE
                | VPS_RULE_KEY_BILLING_CYCLE
                | VPS_RULE_KEY_NETWORK_PORT_SPEED
                | VPS_RULE_KEY_PRODUCT_NAME
        )
    }) {
        if row.key == VPS_RULE_KEY_PRODUCT_NAME {
            product_names.insert(row.client_id, row.value_raw);
        } else if row.key == VPS_RULE_KEY_NETWORK_PORT_SPEED {
            port_speeds.insert(
                row.client_id,
                monitoring_port_speed(&row.value_json).map_err(|error| {
                    ApiError::internal(
                        "monitoring_card_projection_failed",
                        "The VPS monitoring cards could not be prepared.",
                        error,
                    )
                })?,
            );
        } else {
            billing_rules
                .entry(row.client_id)
                .or_default()
                .insert(row.key, row.value_json);
        }
    }
    let billing = billing_rules
        .into_iter()
        .map(|(client_id, rules)| {
            let plan = monitoring_billing_plan(&rules).map_err(|error| {
                ApiError::internal(
                    "monitoring_card_projection_failed",
                    "The VPS monitoring cards could not be prepared.",
                    error,
                )
            })?;
            Ok((client_id, plan))
        })
        .collect::<Result<HashMap<_, _>, ApiError>>()?;
    let primary_ping = primary_ping
        .map_err(ApiError::internal_mapper(
            "monitoring_ping_status_unavailable",
            "Primary Ping status could not be loaded.",
        ))?
        .into_iter()
        .collect::<HashMap<_, _>>();
    let mut primary_ping_history = HashMap::<String, Vec<PingRollupView>>::new();
    for row in primary_ping_history_rows.map_err(ApiError::internal_mapper(
        "monitoring_ping_history_unavailable",
        "Primary Ping history could not be loaded.",
    ))? {
        primary_ping_history
            .entry(row.client_id.clone())
            .or_default()
            .push(row);
    }
    agents
        .into_iter()
        .map(|client| {
            let client_id = client.id.clone();
            let traffic = traffic.get(&client_id).cloned().ok_or_else(|| {
                ApiError::internal(
                    "monitoring_card_projection_failed",
                    "The VPS monitoring cards could not be prepared.",
                    anyhow::anyhow!("traffic projection missing for {client_id}"),
                )
            })?;
            let resources = resources.get(&client_id).cloned();
            let (projection_pending_since, projection_checked_at) = projection_pending
                .get(&client_id)
                .cloned()
                .unwrap_or((None, None));
            Ok(MonitoringCardView {
                client,
                projection_pending_since,
                projection_checked_at,
                product_name: product_names.remove(&client_id),
                billing: billing.get(&client_id).cloned(),
                system_information: system_information.remove(&client_id),
                port_speed: port_speeds.get(&client_id).cloned(),
                resources,
                resource_history: resource_history.remove(&client_id).unwrap_or_default(),
                network: network.remove(&client_id).unwrap_or_default(),
                network_history: network_history.remove(&client_id).unwrap_or_default(),
                network_rate_expected: network_rate_selection.expects_rates(&client_id),
                traffic,
                primary_ping: primary_ping.get(&client_id).cloned(),
                primary_ping_history: primary_ping_history.remove(&client_id).unwrap_or_default(),
            })
        })
        .collect()
}

fn monitoring_port_speed(value: &serde_json::Value) -> anyhow::Result<PortSpeedView> {
    Ok(PortSpeedView {
        bps: value
            .get("bps")
            .and_then(serde_json::Value::as_i64)
            .filter(|value| *value > 0)
            .ok_or_else(|| anyhow::anyhow!("port_speed_bps_invalid"))?,
        display: value
            .get("display")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("port_speed_display_missing"))?,
    })
}

fn monitoring_billing_plan(
    rules: &HashMap<String, serde_json::Value>,
) -> anyhow::Result<BillingPlanView> {
    let price = rules
        .get(VPS_RULE_KEY_BILLING_PRICE)
        .ok_or_else(|| anyhow::anyhow!("billing_cycle_without_price"))?;
    let optional_field = |name: &str| {
        price
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let disabled = price
        .get("disabled")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| anyhow::anyhow!("billing_plan_disabled_missing"))?;
    let display = price
        .get("display")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("billing_plan_display_missing"))?;
    let cycle = rules
        .get(VPS_RULE_KEY_BILLING_CYCLE)
        .map(|value| {
            value
                .get("display")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("billing_cycle_display_missing"))
        })
        .transpose()?;
    let plan = BillingPlanView {
        disabled,
        price: optional_field("price"),
        currency: optional_field("currency"),
        currency_display: optional_field("currency_display"),
        period: optional_field("period"),
        period_code: optional_field("period_code"),
        cycle,
        display,
    };
    if plan.disabled {
        anyhow::ensure!(plan.cycle.is_none(), "billing_disabled_cycle_invalid");
    } else {
        anyhow::ensure!(
            plan.price.is_some()
                && plan.currency.is_some()
                && plan.currency_display.is_some()
                && plan.period.is_some()
                && plan.period_code.is_some(),
            "billing_plan_fields_incomplete"
        );
    }
    Ok(plan)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CurrentNetworkDetail {
    Hidden,
    SingleVpsDetail,
}

async fn client_monitoring_view(
    state: &AppState,
    client_id: &str,
    query: &ClientMonitoringQuery,
    current_network_detail: CurrentNetworkDetail,
) -> Result<ClientMonitoringView, ApiError> {
    let client_ids = vec![client_id.to_string()];
    let client = state
        .repo
        .list_agents_for_client_ids(&client_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "vps_inventory_unavailable",
            "The VPS inventory could not be loaded.",
        ))?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found("monitoring_client_not_found"))?;
    if !client.is_monitoring_visible() {
        return Err(ApiError::not_found("monitoring_client_not_found"));
    }
    // These four endpoint inputs are independent after visibility has been
    // established. Evaluate their errors in the historical order below so
    // public error codes remain stable if more than one source is unavailable.
    let (network_rate_selection, product_name, system_information, range) = tokio::join!(
        state
            .repo
            .network_rate_interface_selection_for_clients(&client_ids),
        state
            .repo
            .list_vps_rules_for_clients(&client_ids, &[VPS_RULE_KEY_PRODUCT_NAME]),
        state
            .repo
            .monitoring_system_information_for_clients(&client_ids),
        monitoring_range(state, &client_ids, query),
    );
    let network_rate_selection = network_rate_selection.map_err(ApiError::internal_mapper(
        "network_interface_selection_unavailable",
        "Network-interface selection could not be loaded.",
    ))?;
    let product_name = product_name
        .map_err(ApiError::internal_mapper(
            "vps_rules_unavailable",
            "VPS display rules could not be loaded.",
        ))?
        .into_iter()
        .next()
        .map(|rule| rule.value_raw);
    let system_information = system_information
        .map_err(ApiError::internal_mapper(
            "monitoring_system_information_unavailable",
            "VPS system information could not be loaded.",
        ))?
        .remove(client_id);
    let range = range?;

    // Every history reader receives the validated, output-bounded point count
    // and time range. The retained readers decode the same unified range
    // projection; running independent telemetry domains together removes the
    // serial round-trip floor without widening any returned result.
    let (
        resources,
        network,
        network_current_detail,
        tunnel_current_detail,
        ping,
        traffic,
        traffic_history,
        ping_targets,
        primary_ping,
    ) = tokio::join!(
        async {
            if range.source == "raw" {
                state
                    .repo
                    .list_dashboard_raw_telemetry_rollups(
                        range.effective_points,
                        range.start_unix,
                        range.end_unix,
                        range.step_secs,
                        &client_ids,
                    )
                    .await
                    .map_err(ApiError::internal_mapper(
                        "monitoring_resource_history_unavailable",
                        "VPS resource history could not be loaded.",
                    ))
            } else {
                let projection = state
                    .repo
                    .list_projected_telemetry_resource_history(
                        range.effective_points,
                        range.start_unix,
                        range.end_unix,
                        range.step_secs,
                        &client_ids,
                    )
                    .await
                    .map_err(ApiError::internal_mapper(
                        "monitoring_resource_history_unavailable",
                        "VPS resource history could not be loaded.",
                    ))?;
                if !projection.complete {
                    return Err(crate::routes_dashboard::dashboard_projection_initializing());
                }
                Ok(projection.rows)
            }
        },
        async {
            if range.source == "raw" {
                state
                    .repo
                    .list_dashboard_raw_telemetry_network_rates_selected(
                        range.effective_points,
                        range.start_unix,
                        range.end_unix,
                        range.step_secs,
                        &network_rate_selection,
                    )
                    .await
                    .map_err(ApiError::internal_mapper(
                        "monitoring_network_history_unavailable",
                        "VPS network history could not be loaded.",
                    ))
            } else {
                let projection = state
                    .repo
                    .list_projected_telemetry_network_history(
                        range.effective_points,
                        range.start_unix,
                        range.end_unix,
                        range.step_secs,
                        &network_rate_selection,
                    )
                    .await
                    .map_err(ApiError::internal_mapper(
                        "monitoring_network_history_unavailable",
                        "VPS network history could not be loaded.",
                    ))?;
                if !projection.complete {
                    return Err(crate::routes_dashboard::dashboard_projection_initializing());
                }
                Ok(projection.rows)
            }
        },
        async {
            match current_network_detail {
                CurrentNetworkDetail::Hidden => Ok(Vec::new()),
                CurrentNetworkDetail::SingleVpsDetail => {
                    state
                        .repo
                        .list_latest_telemetry_network_rates_for_vps_detail(client_id)
                        .await
                }
            }
        },
        async {
            match current_network_detail {
                CurrentNetworkDetail::Hidden => Ok(Vec::new()),
                CurrentNetworkDetail::SingleVpsDetail => {
                    state
                        .repo
                        .list_telemetry_tunnels_for_vps_detail(client_id)
                        .await
                }
            }
        },
        async {
            if range.source == "raw" {
                state
                    .repo
                    .list_raw_ping_results(
                        client_id,
                        Some(range.start_unix),
                        Some(range.end_unix),
                        range.effective_points,
                        range.step_secs,
                    )
                    .await
            } else {
                state
                    .repo
                    .list_ping_rollups(
                        client_id,
                        Some(range.start_unix),
                        Some(range.end_unix),
                        range.effective_points,
                        range.step_secs,
                    )
                    .await
            }
        },
        state.repo.get_traffic_accounting(client_id),
        state.repo.list_traffic_history(
            client_id,
            range.start_unix,
            range.end_unix,
            range.step_secs,
        ),
        state.repo.current_ping_targets_for_client(client_id),
        state.repo.current_primary_ping_for_clients(&client_ids),
    );
    let resources = resources?;
    let network = network?;
    let network_current_detail = network_current_detail.map_err(ApiError::internal_mapper(
        "monitoring_network_current_detail_unavailable",
        "Current VPS network detail could not be loaded.",
    ))?;
    let tunnel_current_detail = tunnel_current_detail.map_err(ApiError::internal_mapper(
        "monitoring_tunnel_current_detail_unavailable",
        "Current VPS tunnel detail could not be loaded.",
    ))?;
    let ping = ping.map_err(ApiError::internal_mapper(
        "monitoring_ping_history_unavailable",
        "Ping history could not be loaded.",
    ))?;
    let traffic = traffic.map_err(ApiError::internal_mapper(
        "traffic_accounting_unavailable",
        "Traffic accounting could not be loaded.",
    ))?;
    let traffic_history = traffic_history.map_err(ApiError::internal_mapper(
        "traffic_history_unavailable",
        "Traffic history could not be loaded.",
    ))?;
    let ping_targets = ping_targets.map_err(ApiError::internal_mapper(
        "monitoring_ping_targets_unavailable",
        "Assigned Ping targets could not be loaded.",
    ))?;
    let primary_ping = primary_ping
        .map_err(ApiError::internal_mapper(
            "monitoring_ping_status_unavailable",
            "Primary Ping status could not be loaded.",
        ))?
        .into_iter()
        .next()
        .map(|(_, ping)| ping);
    Ok(ClientMonitoringView {
        client,
        product_name,
        system_information,
        range,
        resources,
        network,
        network_current_detail,
        tunnel_current_detail,
        traffic,
        traffic_history,
        ping_targets,
        ping,
        primary_ping,
    })
}

async fn monitoring_range(
    state: &AppState,
    client_ids: &[String],
    query: &ClientMonitoringQuery,
) -> Result<MonitoringRangeView, ApiError> {
    let window = query.window.as_deref().unwrap_or("1d");
    let points = query.points.unwrap_or(360);
    if !(2..=1_440).contains(&points) {
        return Err(ApiError::bad_request("monitoring_points_invalid"));
    }
    let now = crate::unix_now();
    let end_unix = query.end_unix.unwrap_or(now);
    if end_unix > now.saturating_add(300) {
        return Err(ApiError::bad_request("monitoring_end_in_future"));
    }
    let fixed_seconds = match window {
        "15m" => Some(15 * 60),
        "1h" => Some(60 * 60),
        "8h" => Some(8 * 60 * 60),
        "1d" => Some(24 * 60 * 60),
        "7d" => Some(7 * 24 * 60 * 60),
        "30d" => Some(30 * 24 * 60 * 60),
        "90d" => Some(90 * 24 * 60 * 60),
        "180d" => Some(180 * 24 * 60 * 60),
        "1y" => Some(365 * 24 * 60 * 60),
        "all" | "custom" => None,
        _ => return Err(ApiError::bad_request("monitoring_window_invalid")),
    };
    let start_unix = match (window, fixed_seconds) {
        (_, Some(seconds)) => end_unix.saturating_sub(seconds),
        ("all", None) => {
            let telemetry_start = state
                .repo
                .dashboard_telemetry_start_unix(client_ids)
                .await
                .map_err(ApiError::internal_mapper(
                    "monitoring_history_range_unavailable",
                    "The available monitoring-history range could not be loaded.",
                ))?;
            let mut first = crate::routes_dashboard::dashboard_telemetry_start_or_initializing(
                telemetry_start,
            )?;
            for client_id in client_ids {
                if let Some(traffic_start) = state
                    .repo
                    .traffic_history_start_unix(client_id)
                    .await
                    .map_err(ApiError::internal_mapper(
                        "traffic_history_unavailable",
                        "Traffic history could not be loaded.",
                    ))?
                {
                    first = Some(first.map_or(traffic_start, |current| current.min(traffic_start)));
                }
            }
            first.unwrap_or(end_unix)
        }
        ("custom", None) => query
            .start_unix
            .ok_or_else(|| ApiError::bad_request("monitoring_custom_start_required"))?,
        _ => unreachable!(),
    };
    if start_unix > end_unix
        || (window == "custom" && end_unix.saturating_sub(start_unix) > 3_650_u64 * 24 * 60 * 60)
    {
        return Err(ApiError::bad_request("monitoring_range_invalid"));
    }
    let span = end_unix.saturating_sub(start_unix);
    let intervals = (points - 1) as u64;
    let requested_step_secs = span
        .saturating_add(intervals.saturating_sub(1))
        .checked_div(intervals.max(1))
        .unwrap_or(60)
        .max(60)
        .saturating_add(59)
        / 60
        * 60;
    let raw_covers_start = if matches!(window, "180d" | "1y" | "all") {
        false
    } else {
        let raw_retention_secs =
            vpsman_common::DEFAULT_TELEMETRY_SAMPLE_RETENTION_DAYS as u64 * 24 * 60 * 60;
        let short_window = matches!(window, "15m" | "1h" | "8h" | "1d" | "7d" | "30d" | "90d")
            || (window == "custom" && span <= raw_retention_secs);
        short_window
            && start_unix >= now.saturating_sub(raw_retention_secs)
            && state
                .repo
                .raw_telemetry_covers_range_start(client_ids, start_unix)
                .await
                .map_err(ApiError::internal_mapper(
                    "monitoring_history_coverage_unavailable",
                    "Monitoring-history coverage could not be determined.",
                ))?
    };
    let source = if raw_covers_start { "raw" } else { "retained" };
    let effective_resolution_secs = if raw_covers_start {
        60
    } else {
        retained_resolution_for_age(now.saturating_sub(start_unix))
    };
    let step_secs = tier_aligned_step_secs(
        span,
        requested_step_secs,
        effective_resolution_secs,
        points as u64,
    );
    let effective_points = aligned_timeline_point_count(start_unix, end_unix, step_secs);
    // Traffic has its own canonical minute ledger and retention frontier. Do
    // not downgrade it because an unrelated resource, network, or Ping raw
    // family starts later.
    let traffic_uses_exact_source = traffic_uses_exact_source(now.saturating_sub(start_unix));
    let traffic_resolution = if traffic_uses_exact_source {
        60
    } else {
        retained_traffic_resolution_for_age(now.saturating_sub(start_unix))
    };
    Ok(MonitoringRangeView {
        window: window.to_string(),
        source: source.to_string(),
        start_unix,
        end_unix,
        requested_step_secs: requested_step_secs.min(i32::MAX as u64) as i32,
        effective_resolution_secs: effective_resolution_secs.min(i32::MAX as u64) as i32,
        step_secs: step_secs.min(i32::MAX as u64) as i32,
        points,
        effective_points,
        resolutions: MonitoringResolutionView {
            resources: effective_resolution_secs.min(i32::MAX as u64) as i32,
            network: effective_resolution_secs.min(i32::MAX as u64) as i32,
            ping: effective_resolution_secs.min(i32::MAX as u64) as i32,
            traffic: traffic_resolution.min(i32::MAX as u64) as i32,
        },
    })
}

fn traffic_uses_exact_source(age_secs: u64) -> bool {
    const DAY: u64 = 24 * 60 * 60;
    age_secs <= vpsman_common::TRAFFIC_COUNTER_RAW_RETENTION_DAYS as u64 * DAY
}

fn retained_resolution_for_age(age_secs: u64) -> u64 {
    const DAY: u64 = 24 * 60 * 60;
    match age_secs {
        value if value <= 2 * DAY => 60,
        value if value <= 8 * DAY => 5 * 60,
        value if value <= 31 * DAY => 30 * 60,
        value if value <= 91 * DAY => 60 * 60,
        value if value <= 181 * DAY => 3 * 60 * 60,
        value if value <= 366 * DAY => 6 * 60 * 60,
        _ => DAY,
    }
}

fn retained_traffic_resolution_for_age(age_secs: u64) -> u64 {
    const DAY: u64 = 24 * 60 * 60;
    vpsman_common::TRAFFIC_COUNTER_HISTORY_TIERS
        .iter()
        .find(|tier| age_secs <= tier.retain_days as u64 * DAY)
        .map(|tier| tier.bucket_secs as u64)
        .unwrap_or(DAY)
}

fn aligned_timeline_point_count(start_unix: u64, end_unix: u64, step_secs: u64) -> i64 {
    let step_secs = step_secs.max(60);
    // Both monitoring UIs floor each bound to an epoch-aligned chart slot.
    // Counting elapsed intervals alone can omit the first honest slot when an
    // unaligned narrow range crosses a coarse retained boundary.
    let first_slot = start_unix / step_secs;
    let last_slot = end_unix / step_secs;
    last_slot
        .saturating_sub(first_slot)
        .saturating_add(1)
        .min(1_440) as i64
}

fn tier_aligned_step_secs(
    span: u64,
    requested: u64,
    resolution: u64,
    requested_points: u64,
) -> u64 {
    let resolution = resolution.max(60);
    let requested = requested.max(resolution);
    let lower = requested / resolution * resolution;
    let lower_points = span
        .checked_div(lower.max(1))
        .unwrap_or(0)
        .saturating_add(1);
    if lower >= resolution && lower_points <= requested_points.saturating_add(12) {
        return lower;
    }
    requested
        .saturating_add(resolution.saturating_sub(1))
        .checked_div(resolution)
        .unwrap_or(1)
        .saturating_mul(resolution)
}

pub(crate) fn public_monitoring_share(
    share: &MonitoringShareRecord,
    visible_target_count: usize,
) -> PublicMonitoringShareView {
    PublicMonitoringShareView {
        id: share.id,
        name: share.name.clone(),
        target_count: visible_target_count,
        visibility: share.visibility.clone(),
        expires_at: share.expires_at.clone(),
    }
}

fn public_monitoring_card(
    card: MonitoringCardView,
    share: &MonitoringShareRecord,
) -> Result<PublicMonitoringCardView, ApiError> {
    let visibility = &share.visibility;
    let projected_telemetry_visible = public_visibility_uses_projected_telemetry(visibility);
    let client_key = share
        .public_client_key(&card.client.id)
        .ok_or_else(|| {
            ApiError::internal(
                "monitoring_share_projection_failed",
                "The shared monitoring view could not be prepared.",
                anyhow::anyhow!("monitoring share target key missing"),
            )
        })?
        .to_string();
    Ok(PublicMonitoringCardView {
        client_key,
        display_name: card.client.display_name,
        status: card.client.status,
        projection_pending_since: projected_telemetry_visible
            .then_some(card.projection_pending_since)
            .flatten(),
        projection_checked_at: projected_telemetry_visible
            .then_some(card.projection_checked_at)
            .flatten(),
        tags: visibility
            .identity_context
            .then(|| public_identity_tags(card.client.tags)),
        product_name: visibility
            .identity_context
            .then_some(card.product_name)
            .flatten(),
        billing: visibility
            .billing
            .then(|| card.billing.map(public_billing_plan))
            .flatten(),
        system_information: visibility
            .system_information
            .then(|| card.system_information.map(public_system_information))
            .flatten(),
        resources: visibility
            .resources
            .then(|| card.resources.map(public_resource_metric))
            .flatten(),
        resource_history: visibility.resources.then(|| {
            card.resource_history
                .into_iter()
                .map(public_resource_metric)
                .collect()
        }),
        network: visibility
            .network
            .then(|| public_network_metric(&card.network, card.network_rate_expected)),
        network_history: visibility
            .network
            .then(|| public_network_points(card.network_history)),
        traffic: visibility
            .traffic
            .then(|| public_traffic_metric(card.traffic, card.port_speed)),
        primary_ping: visibility
            .ping
            .then(|| card.primary_ping.map(public_ping_metric))
            .flatten(),
        primary_ping_history: visibility.ping.then(|| {
            card.primary_ping_history
                .into_iter()
                .map(|row| PublicPingPointView {
                    target_name: row.target_name,
                    bucket_start: row.bucket_start,
                    bucket_secs: row.bucket_secs,
                    sample_count: row.sample_count,
                    latency_avg_ms: row.latency_avg_ms,
                    loss_ratio: row.loss_ratio_avg,
                    status: row.latest_status,
                    checked_at: row.latest_checked_at,
                })
                .collect()
        }),
    })
}

fn public_visibility_uses_projected_telemetry(visibility: &MonitoringShareVisibilityView) -> bool {
    visibility.system_information
        || visibility.resources
        || visibility.network
        || visibility.traffic
        || visibility.ping
}

fn public_billing_plan(plan: BillingPlanView) -> PublicBillingPlanView {
    PublicBillingPlanView {
        disabled: plan.disabled,
        display: plan.display,
        period_code: plan.period_code,
        cycle: plan.cycle,
    }
}

fn public_system_information(information: SystemInformationView) -> PublicSystemInformationView {
    PublicSystemInformationView {
        os_name: information.os_name,
        architecture: information.architecture,
        cpu_model: information.cpu_model,
        kernel_release: information.kernel_release,
        virtualization: information.virtualization,
        reported_at: information.reported_at,
        uptime_secs: information.uptime_secs,
        uptime_observed_at: information.uptime_observed_at,
    }
}

fn public_monitoring_detail(
    detail: ClientMonitoringView,
    share: &MonitoringShareRecord,
) -> Result<PublicMonitoringDetailView, ApiError> {
    let visibility = &share.visibility;
    let client_key = share
        .public_client_key(&detail.client.id)
        .ok_or_else(|| {
            ApiError::internal(
                "monitoring_share_projection_failed",
                "The shared monitoring view could not be prepared.",
                anyhow::anyhow!("monitoring share target key missing"),
            )
        })?
        .to_string();
    Ok(PublicMonitoringDetailView {
        client_key,
        range: PublicMonitoringRangeView {
            window: detail.range.window,
            source: detail.range.source,
            start_unix: detail.range.start_unix,
            end_unix: detail.range.end_unix,
            requested_step_secs: detail.range.requested_step_secs,
            effective_resolution_secs: detail.range.effective_resolution_secs,
            step_secs: detail.range.step_secs,
            points: detail.range.points,
            effective_points: detail.range.effective_points,
            resolutions: detail.range.resolutions,
        },
        resources: visibility.resources.then(|| {
            detail
                .resources
                .into_iter()
                .map(public_resource_metric)
                .collect()
        }),
        network: visibility
            .network
            .then(|| public_network_points(detail.network)),
        traffic: visibility.traffic.then(|| {
            detail
                .traffic_history
                .into_iter()
                .map(|row| PublicTrafficHistoryPointView {
                    bucket_start: row.bucket_start,
                    bucket_secs: row.bucket_secs,
                    sample_count: row.sample_count,
                    reset_count: row.reset_count,
                    rx_bytes: row.rx_bytes,
                    tx_bytes: row.tx_bytes,
                    total_bytes: row.total_bytes,
                })
                .collect()
        }),
        ping_targets: visibility.ping.then(|| {
            detail
                .ping_targets
                .into_iter()
                .map(public_ping_metric)
                .collect()
        }),
        ping: visibility.ping.then(|| {
            detail
                .ping
                .into_iter()
                .map(|row| PublicPingPointView {
                    target_name: row.target_name,
                    bucket_start: row.bucket_start,
                    bucket_secs: row.bucket_secs,
                    sample_count: row.sample_count,
                    latency_avg_ms: row.latency_avg_ms,
                    loss_ratio: row.loss_ratio_avg,
                    status: row.latest_status,
                    checked_at: row.latest_checked_at,
                })
                .collect()
        }),
    })
}

fn public_identity_tags(tags: Vec<String>) -> Vec<String> {
    tags.into_iter()
        .filter(|tag| {
            let Some((key, value)) = tag.split_once(':') else {
                return false;
            };
            matches!(
                key.trim().to_lowercase().as_str(),
                "provider" | "region" | "country"
            ) && !value.trim().is_empty()
                && value.trim().parse::<IpAddr>().is_err()
        })
        .collect()
}

fn public_resource_metric(row: TelemetryRollupView) -> PublicResourceMetricView {
    PublicResourceMetricView {
        bucket_start: row.bucket_start,
        bucket_secs: row.bucket_secs,
        sample_count: row.sample_count,
        cpu_usage_avg: row.cpu_usage_avg,
        cpu_cores: row.cpu_cores_max,
        load_1: row.cpu_load_1_avg,
        load_5: row.cpu_load_5_avg,
        load_15: row.cpu_load_15_avg,
        memory_total_bytes: row.memory_total_bytes_max,
        memory_available_bytes: row.memory_available_bytes_avg,
        memory_used_ratio_avg: row.memory_used_ratio_avg,
        swap_sample_count: row.swap_sample_count,
        swap_total_bytes: row.swap_total_bytes_max,
        swap_available_bytes: row.swap_available_bytes_avg,
        swap_used_ratio_avg: row.swap_used_ratio_avg,
        disk_sample_count: row.disk_sample_count,
        disk_total_bytes: row.disk_total_bytes_max,
        disk_available_bytes: row.disk_available_bytes_avg,
        disk_used_ratio_avg: row.disk_used_ratio_avg,
        tcp_sockets: row.tcp_sockets_latest,
        udp_sockets: row.udp_sockets_latest,
        connections_observed_at: row.connections_observed_at,
        observed_at: row.latest_observed_at,
    }
}

fn public_network_metric(
    rows: &[TelemetryNetworkRateView],
    rate_expected: bool,
) -> PublicNetworkMetricView {
    // Every interface collected in one agent telemetry envelope has the same
    // rebased latest_observed_at. Older current rows belong to an interface
    // absent from the newest sample and must not inflate the current sum.
    let observed_at = rows.iter().map(|row| row.latest_observed_at.as_str()).max();
    PublicNetworkMetricView {
        rate_expected,
        rx_bps: observed_at.map(|latest| {
            rows.iter()
                .filter(|row| row.latest_observed_at == latest)
                .map(|row| row.rx_bps_avg)
                .sum()
        }),
        tx_bps: observed_at.map(|latest| {
            rows.iter()
                .filter(|row| row.latest_observed_at == latest)
                .map(|row| row.tx_bps_avg)
                .sum()
        }),
        observed_at: observed_at.map(str::to_string),
    }
}

fn public_network_points(rows: Vec<TelemetryNetworkRateView>) -> Vec<PublicNetworkPointView> {
    let mut points = std::collections::BTreeMap::<(String, i32), (f64, f64)>::new();
    for row in rows {
        let point = points
            .entry((row.bucket_start, row.bucket_secs))
            .or_default();
        point.0 += row.rx_bps_avg;
        point.1 += row.tx_bps_avg;
    }
    points
        .into_iter()
        .map(
            |((bucket_start, bucket_secs), (rx_bps, tx_bps))| PublicNetworkPointView {
                bucket_start,
                bucket_secs,
                rx_bps,
                tx_bps,
            },
        )
        .collect()
}

pub(super) fn public_traffic_metric(
    row: TrafficAccountingRecord,
    port_speed: Option<PortSpeedView>,
) -> PublicTrafficMetricView {
    let configured = !row.selectors.is_empty() && row.reset_day.is_some();
    PublicTrafficMetricView {
        configured,
        reset_day: configured.then_some(row.reset_day).flatten(),
        reset_hour: configured.then_some(row.reset_hour).flatten(),
        cycle_start: configured.then_some(row.cycle_start).flatten(),
        cycle_end: configured.then_some(row.cycle_end).flatten(),
        rx_bytes: configured.then_some(row.rx_bytes),
        tx_bytes: configured.then_some(row.tx_bytes),
        total_bytes: configured.then_some(row.total_bytes),
        diagnostic_rx_bytes: configured.then_some(row.diagnostic_rx_bytes),
        diagnostic_tx_bytes: configured.then_some(row.diagnostic_tx_bytes),
        diagnostic_total_bytes: configured.then_some(row.diagnostic_total_bytes),
        quota_rx_bytes: configured.then_some(row.quota_rx_bytes).flatten(),
        quota_tx_bytes: configured.then_some(row.quota_tx_bytes).flatten(),
        quota_total_bytes: configured.then_some(row.quota_total_bytes).flatten(),
        cycle_percent: configured.then_some(row.cycle_percent).flatten(),
        state: if configured {
            row.state
        } else {
            "unconfigured".to_string()
        },
        observed_at: configured.then_some(row.last_sample_at).flatten(),
        port_speed: port_speed.map(|speed| PublicPortSpeedView {
            bps: speed.bps,
            display: speed.display,
        }),
    }
}

fn public_ping_metric(row: CurrentPingView) -> PublicPingMetricView {
    PublicPingMetricView {
        target_name: row.target_name,
        state: row.state,
        status: row.status,
        latency_avg_ms: row.latency_avg_ms,
        loss_ratio: row.loss_ratio,
        checked_at: row.checked_at,
    }
}

struct NormalizedPingMutation {
    name: String,
    host: String,
    probe_kind: String,
    port: Option<i32>,
    selector_expression: String,
    target_client_ids: Vec<String>,
}

async fn validate_ping_target_mutation(
    state: &AppState,
    request: &PingTargetMutationRequest,
    frozen_client_ids: Option<&[String]>,
) -> Result<NormalizedPingMutation, ApiError> {
    let name = request.name.trim().to_string();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("ping_target_name_invalid"));
    }
    let host = request.host.trim().trim_matches(['[', ']']).to_string();
    if !valid_ping_host(&host) {
        return Err(ApiError::bad_request("ping_target_host_invalid"));
    }
    let probe_kind = request.probe_kind.trim().to_lowercase();
    let port = match (probe_kind.as_str(), request.port) {
        ("icmp", None) => None,
        ("tcp", Some(port)) if (1..=65_535).contains(&port) => Some(port),
        ("icmp", Some(_)) => return Err(ApiError::bad_request("ping_target_icmp_port_forbidden")),
        ("tcp", None) => return Err(ApiError::bad_request("ping_target_tcp_port_required")),
        _ => return Err(ApiError::bad_request("ping_target_probe_kind_invalid")),
    };
    let selector_expression = request.selector_expression.trim().to_string();
    validate_monitoring_selector(&selector_expression, MAX_MONITORING_SELECTOR_BYTES)?;
    parse_selector_expression(&selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
    let resolved = match frozen_client_ids {
        Some(client_ids) => client_ids.to_vec(),
        None => {
            resolve_selector(state, &selector_expression, MAX_MONITORING_SELECTOR_BYTES).await?
        }
    };
    let submitted = request
        .target_client_ids
        .iter()
        .map(|client_id| client_id.trim())
        .filter(|client_id| !client_id.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if submitted != resolved {
        return Err(ApiError::conflict("ping_target_resolution_stale"));
    }
    Ok(NormalizedPingMutation {
        name,
        host,
        probe_kind,
        port,
        selector_expression,
        target_client_ids: resolved,
    })
}

async fn resolve_selector(
    state: &AppState,
    selector_expression: &str,
    max_bytes: usize,
) -> Result<Vec<String>, ApiError> {
    validate_monitoring_selector(selector_expression, max_bytes)?;
    let mut client_ids = state
        .repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: selector_expression.to_string(),
        })
        .await
        .map_err(ApiError::internal_mapper(
            "selector_targets_unavailable",
            "Selector targets could not be resolved.",
        ))?
        .targets
        .into_iter()
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    client_ids.sort();
    client_ids.dedup();
    Ok(client_ids)
}

fn require_monitoring_selector_scope(
    scopes: &[String],
    selector_expression: &str,
) -> Result<(), ApiError> {
    let expression = parse_selector_expression(selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
    require_vps_rule_selector_scope(scopes, &expression)
}

async fn enrich_ping_target_evidence(
    state: &AppState,
    targets: &mut [PingTargetView],
    allow_vps_rule_selectors: bool,
) -> Result<(), ApiError> {
    if targets.is_empty() {
        return Ok(());
    }
    let target_ids = targets
        .iter()
        .map(|target| target.id)
        .collect::<BTreeSet<_>>();
    let mut assignments = HashMap::<Uuid, Vec<String>>::new();
    for assignment in state
        .repo
        .list_ping_target_assignment_records(None)
        .await
        .map_err(ApiError::internal_mapper(
            "ping_target_assignments_unavailable",
            "Ping-target assignments could not be loaded.",
        ))?
    {
        if target_ids.contains(&assignment.target_id) {
            assignments
                .entry(assignment.target_id)
                .or_default()
                .push(assignment.client_id);
        }
    }
    let applies = state
        .repo
        .list_runtime_config_apply_records(None)
        .await
        .map_err(ApiError::internal_mapper(
            "runtime_config_apply_records_unavailable",
            "Runtime-config application evidence could not be loaded.",
        ))?
        .into_iter()
        .map(|apply| (apply.client_id.clone(), apply))
        .collect::<HashMap<_, _>>();
    let selectors = targets
        .iter()
        .map(|target| target.selector_expression.clone())
        .collect::<BTreeSet<_>>();
    let selector_evidence = resolve_saved_selectors_batch(
        state,
        &selectors,
        MAX_MONITORING_SELECTOR_BYTES,
        allow_vps_rule_selectors,
    )
    .await;
    for target in targets {
        let mut client_ids = assignments.remove(&target.id).unwrap_or_default();
        client_ids.sort();
        client_ids.dedup();
        target.target_client_ids = client_ids.clone();
        target.target_update_evidence_available = selector_evidence
            .evidence_available
            .get(&target.selector_expression)
            .copied()
            .unwrap_or(false);
        target.target_update_available = target.target_update_evidence_available
            && selector_evidence
                .resolved_client_ids
                .get(&target.selector_expression)
                .is_some_and(|resolved| resolved != &client_ids);
        target.runtime_sync = ping_target_runtime_sync(target, &client_ids, &applies);
    }
    Ok(())
}

/// Mutation responses retain the exact committed assignment snapshot returned
/// by the repository. Runtime/config evidence is supplemental: a transient
/// evidence read must not turn a durable mutation into an ambiguous failure.
async fn enrich_ping_target_mutation_evidence(
    state: &AppState,
    target: &mut PingTargetView,
    allow_vps_rule_selectors: bool,
) {
    let client_ids = target.target_client_ids.clone();
    let applies = match state.repo.list_runtime_config_apply_records(None).await {
        Ok(applies) => Some(
            applies
                .into_iter()
                .map(|apply| (apply.client_id.clone(), apply))
                .collect::<HashMap<_, _>>(),
        ),
        Err(error) => {
            tracing::warn!(
                error = %error,
                target_id = %target.id,
                "Ping-target mutation runtime evidence unavailable"
            );
            None
        }
    };
    let selectors = [target.selector_expression.clone()]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let selector_evidence = resolve_saved_selectors_batch(
        state,
        &selectors,
        MAX_MONITORING_SELECTOR_BYTES,
        allow_vps_rule_selectors,
    )
    .await;
    target.target_update_evidence_available = selector_evidence
        .evidence_available
        .get(&target.selector_expression)
        .copied()
        .unwrap_or(false);
    target.target_update_available = target.target_update_evidence_available
        && selector_evidence
            .resolved_client_ids
            .get(&target.selector_expression)
            .is_some_and(|resolved| resolved != &client_ids);
    if let Some(applies) = applies.as_ref() {
        target.runtime_sync = ping_target_runtime_sync(target, &client_ids, applies);
    }
}

async fn enrich_monitoring_share_target_evidence(
    state: &AppState,
    shares: &mut [MonitoringShareView],
    allow_vps_rule_selectors: bool,
) -> Result<(), ApiError> {
    let selectors = shares
        .iter()
        .filter(|share| share.status == "active")
        .map(|share| share.selector_expression.clone())
        .collect::<BTreeSet<_>>();
    let selector_evidence = resolve_saved_selectors_batch(
        state,
        &selectors,
        MAX_SHARE_SELECTOR_BYTES,
        allow_vps_rule_selectors,
    )
    .await;
    for share in shares {
        share.target_client_ids.sort();
        share.target_client_ids.dedup();
        share.target_update_evidence_available = share.status != "active"
            || selector_evidence
                .evidence_available
                .get(&share.selector_expression)
                .copied()
                .unwrap_or(false);
        share.target_update_available = share.status == "active"
            && share.target_update_evidence_available
            && selector_evidence
                .resolved_client_ids
                .get(&share.selector_expression)
                .is_some_and(|resolved| resolved != &share.target_client_ids);
    }
    Ok(())
}

#[derive(Default)]
struct SavedSelectorEvidenceBatch {
    evidence_available: HashMap<String, bool>,
    resolved_client_ids: HashMap<String, Vec<String>>,
}

/// Resolves all saved selectors from one coherent inventory snapshot. Rule-aware
/// selectors share one complete rule load; malformed or temporarily
/// unresolvable selectors remain visible with their live evidence unavailable.
async fn resolve_saved_selectors_batch(
    state: &AppState,
    selectors: &BTreeSet<String>,
    max_bytes: usize,
    allow_vps_rule_selectors: bool,
) -> SavedSelectorEvidenceBatch {
    let mut batch = SavedSelectorEvidenceBatch::default();
    let mut parsed = Vec::new();
    for selector_expression in selectors {
        batch
            .evidence_available
            .insert(selector_expression.clone(), false);
        if validate_monitoring_selector(selector_expression, max_bytes).is_err() {
            continue;
        }
        let Ok(Some(expression)) = parse_selector_expression(selector_expression) else {
            continue;
        };
        let references_rules = vpsman_common::expression_references_vps_rules(&expression);
        if references_rules && !allow_vps_rule_selectors {
            continue;
        }
        parsed.push((selector_expression.clone(), expression, references_rules));
    }
    if parsed.is_empty() {
        return batch;
    }

    let agents = match state.repo.list_agents().await {
        Ok(agents) => agents,
        Err(error) => {
            tracing::warn!(
                error = %error,
                selector_count = parsed.len(),
                "Saved-selector refresh evidence unavailable"
            );
            return batch;
        }
    };
    let needs_rules = parsed
        .iter()
        .any(|(_, _, references_rules)| *references_rules);
    let rules_by_client = if needs_rules {
        match state.repo.vps_rule_contexts_for_agents(&agents).await {
            Ok(contexts) => Some(contexts),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "VPS-rule selector refresh evidence unavailable"
                );
                None
            }
        }
    } else {
        None
    };

    for (selector_expression, expression, references_rules) in parsed {
        if references_rules && rules_by_client.is_none() {
            continue;
        }
        let empty_rules = HashMap::new();
        let rules = rules_by_client.as_ref().unwrap_or(&empty_rules);
        let mut client_ids = Repository::resolve_agents_for_expression_with_rule_contexts(
            &agents,
            &expression,
            rules,
        )
        .into_iter()
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
        client_ids.sort();
        client_ids.dedup();
        batch
            .evidence_available
            .insert(selector_expression.clone(), true);
        batch
            .resolved_client_ids
            .insert(selector_expression, client_ids);
    }
    batch
}

fn ping_target_runtime_sync(
    target: &PingTargetView,
    client_ids: &[String],
    applies: &HashMap<String, RuntimeConfigApplyStateRecord>,
) -> PingTargetRuntimeSyncView {
    if client_ids.is_empty() {
        return PingTargetRuntimeSyncView {
            state: "not_applicable".to_string(),
            reason: "No frozen VPS assignments require runtime sync".to_string(),
        };
    }
    let mut counts = HashMap::<&'static str, usize>::new();
    let mut first_failure = None;
    for client_id in client_ids {
        let Some(apply) = applies.get(client_id) else {
            *counts.entry("unknown").or_default() += 1;
            continue;
        };
        let pending_is_current =
            runtime_config_matches_ping_target(apply.pending_config.as_ref(), target);
        let applied_is_current =
            runtime_config_matches_ping_target(apply.applied_config.as_ref(), target);
        let state = if pending_is_current && apply.pending_status.as_deref() == Some("failed") {
            if first_failure.is_none() {
                first_failure = Some(format!(
                    "{client_id}: {}",
                    apply
                        .pending_error
                        .as_deref()
                        .unwrap_or("The last runtime apply failed")
                ));
            }
            "failed"
        } else if pending_is_current && apply.pending_status.as_deref() == Some("queued") {
            "queued"
        } else if applied_is_current {
            "applied"
        } else {
            "stale"
        };
        *counts.entry(state).or_default() += 1;
    }
    let applied = counts.get("applied").copied().unwrap_or(0);
    let queued = counts.get("queued").copied().unwrap_or(0);
    let failed = counts.get("failed").copied().unwrap_or(0);
    let stale = counts.get("stale").copied().unwrap_or(0);
    let unknown = counts.get("unknown").copied().unwrap_or(0);
    let state = if failed > 0 {
        "failed"
    } else if stale > 0 {
        "stale"
    } else if queued > 0 {
        "queued"
    } else if unknown > 0 {
        "unknown"
    } else {
        "applied"
    };
    let mut reason = format!(
        "{} assigned VPSs: {applied} applied, {queued} queued, {failed} failed, {stale} stale, {unknown} unknown",
        client_ids.len(),
    );
    if let Some(failure) = first_failure {
        reason.push_str(&format!("; {failure}"));
    }
    PingTargetRuntimeSyncView {
        state: state.to_string(),
        reason,
    }
}

fn runtime_config_matches_ping_target(
    config: Option<&AgentRuntimeConfig>,
    target: &PingTargetView,
) -> bool {
    let Some(config) = config else {
        return false;
    };
    let configured = config
        .network
        .ping_targets
        .iter()
        .find(|configured| configured.id == target.id.to_string());
    if !target.enabled {
        return configured.is_none();
    }
    configured.is_some_and(|configured| {
        configured.generation == target.generation.max(1) as u64
            && configured.name == target.name
            && configured.host == target.host
            && configured.port.map(i32::from) == target.port
            && matches!(
                (&configured.kind, target.probe_kind.as_str()),
                (AgentPingProbeKind::Icmp, "icmp") | (AgentPingProbeKind::Tcp, "tcp")
            )
    })
}

fn validate_monitoring_selector(
    selector_expression: &str,
    max_bytes: usize,
) -> Result<(), ApiError> {
    if selector_expression.is_empty()
        || selector_expression.len() > max_bytes
        || selector_expression.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request("invalid_selector_expression"));
    }
    Ok(())
}

fn validate_monitoring_share_targets(targets: &[String]) -> Result<(), ApiError> {
    if targets.is_empty() {
        return Err(ApiError::bad_request(
            "monitoring_share_target_selection_required",
        ));
    }
    if targets.len() > MAX_SHARE_TARGETS {
        return Err(ApiError::bad_request(
            "monitoring_share_target_count_too_large",
        ));
    }
    Ok(())
}

fn valid_ping_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || host.chars().any(char::is_control) {
        return false;
    }
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    !host.starts_with('.')
        && !host.ends_with('.')
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn ping_assignment_preview_hash(
    changes: &[PingTargetAssignmentChangeView],
) -> Result<String, ApiError> {
    serde_json::to_vec(changes)
        .map(|payload| payload_hash(&payload))
        .map_err(|error| {
            ApiError::internal(
                "ping_target_preview_failed",
                "The Ping target update preview could not be prepared.",
                anyhow::anyhow!(error),
            )
        })
}

fn monitoring_share_target_preview_hash(
    changes: &[MonitoringShareTargetChangeView],
) -> Result<String, ApiError> {
    serde_json::to_vec(changes)
        .map(|payload| payload_hash(&payload))
        .map_err(|error| {
            ApiError::internal(
                "monitoring_share_preview_failed",
                "The shared-view target preview could not be prepared.",
                anyhow::anyhow!(error),
            )
        })
}

fn monitoring_share_definition_preview_hash(
    change: &MonitoringShareDefinitionChangeView,
    expected_updated_at: &str,
) -> Result<String, ApiError> {
    serde_json::to_vec(&(expected_updated_at, change))
        .map(|payload| payload_hash(&payload))
        .map_err(|error| {
            ApiError::internal(
                "monitoring_share_preview_failed",
                "The shared-view edit preview could not be prepared.",
                anyhow::anyhow!(error),
            )
        })
}

fn normalized_monitoring_share_visibility(
    visibility: &MonitoringShareVisibilityRequest,
) -> Result<MonitoringShareVisibilityView, ApiError> {
    let visibility = MonitoringShareVisibilityView {
        identity_context: visibility.identity_context,
        billing: visibility.billing,
        system_information: visibility.system_information,
        resources: visibility.resources,
        network: visibility.network,
        traffic: visibility.traffic,
        ping: visibility.ping,
        detail_history: visibility.detail_history,
    };
    if visibility.detail_history
        && !(visibility.system_information
            || visibility.resources
            || visibility.network
            || visibility.traffic
            || visibility.ping)
    {
        return Err(ApiError::bad_request(
            "monitoring_share_detail_requires_visible_metrics",
        ));
    }
    Ok(visibility)
}

fn monitoring_repository_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("ping_target_update_stale") {
        ApiError::conflict("ping_target_update_stale")
    } else if message.contains("not_found") {
        ApiError::not_found("ping_target_not_found")
    } else if message.contains("preview_stale") {
        ApiError::conflict("ping_target_preview_stale")
    } else if message.contains("resolution_stale") {
        ApiError::conflict("ping_target_resolution_stale")
    } else if message.contains("conflict") || message.contains("too_many") {
        ApiError::conflict_with_message("ping_target_conflict", message)
    } else if message.contains("invalid") || message.contains("required") {
        ApiError::bad_request_with_message("ping_target_invalid", message)
    } else {
        ApiError::internal(
            "ping_target_mutation_failed",
            "The Ping target change could not be completed.",
            error,
        )
    }
}

fn share_repository_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("preview_stale") {
        ApiError::conflict("monitoring_share_preview_stale")
    } else if message.contains("resolution_stale") {
        ApiError::conflict("monitoring_share_resolution_stale")
    } else if message.contains("not_found") {
        ApiError::not_found("monitoring_share_not_found")
    } else if message.contains("not_active") {
        ApiError::conflict("monitoring_share_not_active")
    } else if message.contains("monitoring_share_selection_required")
        || message.contains("monitoring_share_selection_invalid")
        || message.contains("monitoring_share_selection_too_large")
        || message.contains("monitoring_share_target_count_too_large")
        || message.contains("monitoring_share_target_client_id_invalid")
        || message.contains("monitoring_share_public_client_key_invalid")
    {
        ApiError::bad_request_with_message("monitoring_share_invalid", message)
    } else {
        ApiError::internal(
            "monitoring_share_mutation_failed",
            "The shared monitoring view change could not be completed.",
            error,
        )
    }
}

pub(crate) fn share_secret(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(SHARE_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or_else(|| ApiError::unauthorized("monitoring_share_token_required"))
}

fn share_visitor_id(headers: &HeaderMap) -> Result<Uuid, ApiError> {
    headers
        .get(SHARE_VISITOR_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value.trim()).ok())
        .ok_or_else(|| ApiError::unauthorized("monitoring_share_visitor_bootstrap_required"))
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|pair| {
            pair.split_once('=')
                .filter(|(key, _)| *key == name)
                .map(|(_, value)| value)
        })
}

#[cfg(test)]
#[path = "tests_routes_monitoring.rs"]
mod tests;
