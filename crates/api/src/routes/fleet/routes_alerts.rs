use std::collections::HashSet;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL, Engine as _};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vpsman_common::{
    is_fleet_alert_notification_delivery_process_status,
    is_fleet_alert_notification_delivery_status,
};

use crate::{
    error::ApiError,
    fleet_alerts::apply_alert_states,
    model::{
        BulkResolveFleetAlertsRequest, BulkResolveFleetAlertsResponse, FleetAlertEventPage,
        FleetAlertEventQuery, FleetAlertEventSyncRequest, FleetAlertEventSyncResponse,
        FleetAlertHistoryQuery, FleetAlertQuery, FleetAlertView, ResolveFleetAlertRequest,
    },
    model_alert_notifications::{
        CreateFleetAlertNotificationChannelRequest, DeleteFleetAlertNotificationChannelRequest,
        FleetAlertNotificationChannelBulkRequest, FleetAlertNotificationChannelBulkResponse,
        FleetAlertNotificationChannelQuery, FleetAlertNotificationChannelView,
        FleetAlertNotificationDeliveryQuery, FleetAlertNotificationDeliveryView,
        FleetAlertNotificationDispatchRequest, FleetAlertNotificationProcessRequest,
    },
    model_alert_policies::{
        CreateFleetAlertPolicyRequest, DeleteFleetAlertPolicyRequest, FleetAlertPolicyBulkRequest,
        FleetAlertPolicyBulkResponse, FleetAlertPolicyQuery, PolicyAlertQuery, PolicyAlertRecord,
        PolicyDryRunRequest, PolicyDryRunResponse, PolicyGroupRecord, TrafficAccountingQuery,
        TrafficAccountingRecord, VpsRuleQuery, VpsRuleValueRecord, VpsRulesBulkUnsetRequest,
        VpsRulesBulkUpsertRequest, VpsRulesDryRunRequest, VpsRulesDryRunResponse,
    },
    model_alert_states::{
        BulkUpdateFleetAlertStatesRequest, BulkUpdateFleetAlertStatesResponse,
        FleetAlertExportView, FleetAlertStateQuery, FleetAlertStateView,
        UpdateFleetAlertStateRequest,
    },
    repository_operational_alerts::operational_episode_to_fleet_alert,
    repository_webhook_rules::validate_webhook_rule_target,
    security::{
        operator_has_scope, require_vps_rule_selector_scope, SCOPE_BACKUPS_READ, SCOPE_CONFIG_READ,
        SCOPE_FLEET_READ, SCOPE_INTEGRATIONS_READ, SCOPE_INTEGRATIONS_WRITE,
    },
    selector_expression::parse_selector_expression,
    state::AppState,
    unix_now,
    util::{limit_or_default, parse_timestamp_utc},
};

const FLEET_ALERT_EVENT_PAGE_LIMIT: usize = 200;
const FLEET_ALERT_EVENT_SYNC_ID_LIMIT: usize = 5_000;
const ALERT_CONFIGURATION_BULK_ITEM_LIMIT: usize = 500;

#[derive(Debug, Deserialize, Serialize)]
struct FleetAlertEventCursor {
    triggered_at: String,
    episode_id: Uuid,
}

pub(crate) async fn list_fleet_alerts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertQuery>,
) -> Result<Json<Vec<FleetAlertView>>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_BACKUPS_READ) {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    validate_alert_query(&query)?;
    Ok(Json(state.list_fleet_alerts(query).await.map_err(
        ApiError::internal_mapper(
            "fleet_alerts_unavailable",
            "Fleet alerts could not be loaded.",
        ),
    )?))
}

pub(crate) async fn list_fleet_alert_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertHistoryQuery>,
) -> Result<Json<Vec<FleetAlertView>>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_BACKUPS_READ) {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    let fleet_query = FleetAlertQuery {
        limit: query.limit,
        client_id: query.client_id,
        severity: query.severity,
        category: query.category,
        operator_state: query.operator_state,
        include_muted: query.include_muted,
    };
    validate_alert_query(&fleet_query)?;
    if query
        .start_unix
        .zip(query.end_unix)
        .is_some_and(|(start, end)| start > end)
    {
        return Err(ApiError::bad_request("fleet_alert_history_window_invalid"));
    }
    Ok(Json(
        state
            .list_fleet_alert_history_bounded(fleet_query, query.start_unix, query.end_unix)
            .await
            .map_err(ApiError::internal_mapper(
                "fleet_alert_history_unavailable",
                "Fleet alert history could not be loaded.",
            ))?
            .alerts,
    ))
}

pub(crate) async fn list_fleet_alert_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertEventQuery>,
) -> Result<Json<FleetAlertEventPage>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_BACKUPS_READ) {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    let fleet_query = FleetAlertQuery {
        limit: query.limit,
        client_id: query.client_id,
        severity: query.severity,
        category: query.category,
        operator_state: query.operator_state,
        include_muted: query.include_muted,
    };
    validate_alert_query(&fleet_query)?;
    let limit = fleet_query.limit.unwrap_or(50).clamp(1, 200) as usize;
    let cursor = decode_fleet_alert_event_cursor(query.cursor.as_deref())?;
    let mut episodes = state
        .repo
        .list_unresolved_operational_alert_events_page(
            &fleet_query,
            cursor,
            limit.saturating_add(1),
        )
        .await
        .map_err(ApiError::internal_mapper(
            "fleet_alert_events_unavailable",
            "Fleet alert events could not be loaded.",
        ))?;
    let has_more = episodes.len() > limit;
    if has_more {
        episodes.truncate(limit);
    }
    let next_cursor = if has_more {
        episodes
            .last()
            .map(encode_fleet_alert_event_cursor)
            .transpose()?
    } else {
        None
    };
    let mut items = episodes
        .iter()
        .map(operational_episode_to_fleet_alert)
        .collect::<Vec<_>>();
    let alert_ids = items
        .iter()
        .map(|alert| alert.id.clone())
        .collect::<Vec<_>>();
    let states = state
        .repo
        .list_fleet_alert_states_for_alert_ids(&alert_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "fleet_alert_state_unavailable",
            "Fleet alert triage state could not be loaded.",
        ))?;
    apply_alert_states(&mut items, &states);
    Ok(Json(FleetAlertEventPage {
        items,
        next_cursor,
        has_more,
    }))
}

pub(crate) async fn sync_fleet_alert_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<FleetAlertEventSyncRequest>,
) -> Result<Json<FleetAlertEventSyncResponse>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_BACKUPS_READ) {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    let known_alert_ids = normalize_fleet_alert_event_sync_ids(request.known_alert_ids)?;
    let mut sync = state
        .repo
        .sync_unresolved_operational_alert_events(
            &known_alert_ids,
            FLEET_ALERT_EVENT_PAGE_LIMIT.saturating_add(1),
        )
        .await
        .map_err(ApiError::internal_mapper(
            "fleet_alert_event_sync_unavailable",
            "Current Fleet alert occurrences could not be synchronized.",
        ))?;
    let has_more = sync.head.len() > FLEET_ALERT_EVENT_PAGE_LIMIT;
    if has_more {
        sync.head.truncate(FLEET_ALERT_EVENT_PAGE_LIMIT);
    }
    let next_cursor = if has_more {
        sync.head
            .last()
            .map(encode_fleet_alert_event_cursor)
            .transpose()?
    } else {
        None
    };
    let mut head = sync
        .head
        .iter()
        .map(operational_episode_to_fleet_alert)
        .collect::<Vec<_>>();
    let mut current_items = sync
        .current
        .iter()
        .map(operational_episode_to_fleet_alert)
        .collect::<Vec<_>>();
    apply_alert_states(&mut head, &sync.states);
    apply_alert_states(&mut current_items, &sync.states);
    Ok(Json(FleetAlertEventSyncResponse {
        head: FleetAlertEventPage {
            items: head,
            next_cursor,
            has_more,
        },
        current_items,
    }))
}

fn normalize_fleet_alert_event_sync_ids(
    requested_ids: Vec<String>,
) -> Result<Vec<String>, ApiError> {
    if requested_ids.len() > FLEET_ALERT_EVENT_SYNC_ID_LIMIT {
        return Err(ApiError::bad_request(
            "fleet_alert_event_sync_items_invalid",
        ));
    }
    let mut unique = HashSet::with_capacity(requested_ids.len());
    let mut known_alert_ids = Vec::with_capacity(requested_ids.len());
    for alert_id in requested_ids {
        validate_alert_id(&alert_id)?;
        let alert_id = alert_id.trim().to_string();
        if !unique.insert(alert_id.clone()) {
            return Err(ApiError::bad_request(
                "fleet_alert_event_sync_duplicate_item",
            ));
        }
        known_alert_ids.push(alert_id);
    }
    Ok(known_alert_ids)
}

fn decode_fleet_alert_event_cursor(
    cursor: Option<&str>,
) -> Result<Option<(DateTime<Utc>, Uuid)>, ApiError> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let bytes = BASE64_URL
        .decode(cursor)
        .map_err(|_| ApiError::bad_request("fleet_alert_event_cursor_invalid"))?;
    let payload = serde_json::from_slice::<FleetAlertEventCursor>(&bytes)
        .map_err(|_| ApiError::bad_request("fleet_alert_event_cursor_invalid"))?;
    let triggered_at = parse_timestamp_utc(&payload.triggered_at)
        .ok_or_else(|| ApiError::bad_request("fleet_alert_event_cursor_invalid"))?;
    Ok(Some((triggered_at, payload.episode_id)))
}

fn encode_fleet_alert_event_cursor(
    episode: &crate::model::OperationalAlertEpisodeRecord,
) -> Result<String, ApiError> {
    let triggered_at = parse_timestamp_utc(&episode.triggered_at)
        .ok_or_else(|| {
            ApiError::internal(
                "fleet_alert_event_cursor_failed",
                "The Fleet alert cursor could not be generated.",
                anyhow::anyhow!("operational alert has an invalid triggered_at timestamp"),
            )
        })?
        .to_rfc3339();
    let payload = serde_json::to_vec(&FleetAlertEventCursor {
        triggered_at,
        episode_id: episode.id,
    })
    .map_err(|error| {
        ApiError::internal(
            "fleet_alert_event_cursor_failed",
            "The Fleet alert cursor could not be generated.",
            error.into(),
        )
    })?;
    Ok(BASE64_URL.encode(payload))
}

pub(crate) async fn resolve_fleet_alert(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(alert_id): Path<String>,
    Json(request): Json<ResolveFleetAlertRequest>,
) -> Result<Json<FleetAlertView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_FLEET_READ)
        || !operator_has_scope(&operator.operator.scopes, SCOPE_BACKUPS_READ)
    {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_resolution_confirmation_required",
        ));
    }
    let reason = request.reason.trim();
    if reason.is_empty() || reason.len() > 1024 {
        return Err(ApiError::bad_request(
            "fleet_alert_resolution_reason_invalid",
        ));
    }
    let episode = state
        .repo
        .resolve_operational_alert_event(&alert_id, reason, &operator)
        .await
        .map_err(fleet_alert_resolution_error)?;
    let mut alert = operational_episode_to_fleet_alert(&episode);
    let states = state
        .repo
        .list_fleet_alert_states_for_alert_ids(&[alert.id.clone()])
        .await
        .map_err(ApiError::internal_mapper(
            "fleet_alert_state_unavailable",
            "Fleet alert triage state could not be loaded.",
        ))?;
    apply_alert_states(std::slice::from_mut(&mut alert), &states);
    Ok(Json(alert))
}

pub(crate) async fn bulk_resolve_fleet_alerts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkResolveFleetAlertsRequest>,
) -> Result<Json<BulkResolveFleetAlertsResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_FLEET_READ)
        || !operator_has_scope(&operator.operator.scopes, SCOPE_BACKUPS_READ)
    {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    validate_bulk_fleet_alert_resolution(&request)?;
    let items = request
        .items
        .iter()
        .map(|item| {
            (
                item.alert_id.clone(),
                Some(item.expected_trigger_generation),
            )
        })
        .collect::<Vec<_>>();
    let (_, episodes) = state
        .repo
        .resolve_operational_alert_events(&items, request.reason.trim(), &operator)
        .await
        .map_err(fleet_alert_resolution_error)?;
    let mut alerts = episodes
        .iter()
        .map(operational_episode_to_fleet_alert)
        .collect::<Vec<_>>();
    let alert_ids = alerts
        .iter()
        .map(|alert| alert.id.clone())
        .collect::<Vec<_>>();
    let states = state
        .repo
        .list_fleet_alert_states_for_alert_ids(&alert_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "fleet_alert_state_unavailable",
            "Fleet alert triage state could not be loaded.",
        ))?;
    apply_alert_states(&mut alerts, &states);
    Ok(Json(BulkResolveFleetAlertsResponse { alerts }))
}

fn fleet_alert_resolution_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("fleet_alert_not_found") {
        return ApiError::not_found("fleet_alert_not_found");
    }
    if message.contains("fleet_alert_condition_not_operator_resolvable") {
        return ApiError::conflict("fleet_alert_condition_not_operator_resolvable");
    }
    if message.contains("fleet_alert_already_resolved") {
        return ApiError::conflict("fleet_alert_already_resolved");
    }
    if message.contains("fleet_alert_resolution_snapshot_stale") {
        return ApiError::conflict("fleet_alert_resolution_snapshot_stale");
    }
    for code in [
        "fleet_alert_resolution_items_invalid",
        "fleet_alert_resolution_duplicate_item",
        "fleet_alert_resolution_generation_invalid",
        "fleet_alert_id_required",
    ] {
        if message.contains(code) {
            return ApiError::bad_request(code);
        }
    }
    if message.contains("fleet_alert_resolution_reason_invalid") {
        return ApiError::bad_request("fleet_alert_resolution_reason_invalid");
    }
    ApiError::internal(
        "fleet_alert_resolution_failed",
        "The Fleet alert could not be resolved.",
        error,
    )
}

fn validate_bulk_fleet_alert_resolution(
    request: &BulkResolveFleetAlertsRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_resolution_confirmation_required",
        ));
    }
    let reason = request.reason.trim();
    if reason.is_empty() || reason.len() > 1024 {
        return Err(ApiError::bad_request(
            "fleet_alert_resolution_reason_invalid",
        ));
    }
    if request.items.is_empty() || request.items.len() > 1_000 {
        return Err(ApiError::bad_request(
            "fleet_alert_resolution_items_invalid",
        ));
    }
    let mut alert_ids = HashSet::with_capacity(request.items.len());
    for item in &request.items {
        validate_alert_id(&item.alert_id)?;
        if item.expected_trigger_generation < 1 {
            return Err(ApiError::bad_request(
                "fleet_alert_resolution_generation_invalid",
            ));
        }
        if !alert_ids.insert(item.alert_id.trim()) {
            return Err(ApiError::bad_request(
                "fleet_alert_resolution_duplicate_item",
            ));
        }
    }
    Ok(())
}

pub(crate) async fn export_fleet_alerts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertQuery>,
) -> Result<Json<FleetAlertExportView>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_BACKUPS_READ) {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    validate_alert_query(&query)?;
    let query_summary = serde_json::json!({
        "limit": query.limit,
        "client_id": &query.client_id,
        "severity": &query.severity,
        "category": &query.category,
        "operator_state": &query.operator_state,
        "include_muted": query.include_muted,
    });
    let alerts = state
        .list_fleet_alerts(query)
        .await
        .map_err(ApiError::internal_mapper(
            "fleet_alerts_unavailable",
            "Fleet alerts could not be loaded.",
        ))?;
    Ok(Json(FleetAlertExportView {
        generated_at: unix_now().to_string(),
        query: query_summary,
        alerts,
    }))
}

pub(crate) async fn list_fleet_alert_states(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertStateQuery>,
) -> Result<Json<Vec<FleetAlertStateView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_alert_state_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_fleet_alert_states(
                query.limit.unwrap_or(50).clamp(1, 1000),
                query.state.as_deref(),
            )
            .await
            .map_err(ApiError::internal_mapper(
                "fleet_alert_states_unavailable",
                "Fleet alert states could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn update_fleet_alert_state(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateFleetAlertStateRequest>,
) -> Result<Json<FleetAlertStateView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_alert_state_request(&request)?;
    Ok(Json(
        state
            .repo
            .update_fleet_alert_state(&request, &operator)
            .await
            .map_err(fleet_alert_state_mutation_error)?,
    ))
}

pub(crate) async fn bulk_update_fleet_alert_states(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkUpdateFleetAlertStatesRequest>,
) -> Result<Json<BulkUpdateFleetAlertStatesResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_bulk_alert_state_request(&request)?;
    Ok(Json(
        state
            .repo
            .bulk_update_fleet_alert_states(&request, &operator)
            .await
            .map_err(fleet_alert_state_mutation_error)?,
    ))
}

fn fleet_alert_state_mutation_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("fleet_alert_state_snapshot_stale") {
        return ApiError::conflict("fleet_alert_state_snapshot_stale");
    }
    for code in [
        "fleet_alert_state_confirmation_required",
        "fleet_alert_state_items_invalid",
        "fleet_alert_state_duplicate_item",
        "fleet_alert_state_expected_revision_invalid",
        "fleet_alert_state_action_invalid",
        "fleet_alert_mute_duration_invalid",
        "fleet_alert_mute_duration_unexpected",
    ] {
        if message.contains(code) {
            return ApiError::bad_request(code);
        }
    }
    if message.contains("fleet_alert_escalation_level_overflow")
        || message.contains("fleet_alert_state_revision_overflow")
    {
        return ApiError::conflict("fleet_alert_state_snapshot_stale");
    }
    ApiError::internal(
        "fleet_alert_state_update_failed",
        "The fleet alert state could not be updated.",
        error,
    )
}

pub(crate) async fn list_fleet_alert_policies(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertPolicyQuery>,
) -> Result<Json<Vec<PolicyGroupRecord>>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    validate_alert_policy_query(&query)?;
    if let Some(selector) = query.selector_expression.as_deref() {
        if let Some(expression) = parse_selector_expression(selector)
            .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        {
            require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
        }
    }
    let groups = state
        .repo
        .list_fleet_alert_policies(
            limit_or_default(query.limit),
            query.enabled,
            query.selector_expression.as_deref(),
            query.client_id.as_deref(),
            operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
        )
        .await
        .map_err(fleet_alert_policy_error)?;
    Ok(Json(groups))
}

pub(crate) async fn get_fleet_alert_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(policy_id): Path<uuid::Uuid>,
) -> Result<Json<PolicyGroupRecord>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    let group = state
        .repo
        .get_fleet_alert_policy(
            policy_id,
            operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
        )
        .await
        .map_err(fleet_alert_policy_error)?;
    Ok(Json(group))
}

pub(crate) async fn dry_run_fleet_alert_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PolicyDryRunRequest>,
) -> Result<Json<PolicyDryRunResponse>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    require_alert_policy_source_scopes(&operator.operator.scopes)?;
    let expression = parse_selector_expression(&request.selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
    require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    Ok(Json(
        state
            .repo
            .dry_run_fleet_alert_policy(&request)
            .await
            .map_err(fleet_alert_policy_error)?,
    ))
}

pub(crate) async fn upsert_fleet_alert_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateFleetAlertPolicyRequest>,
) -> Result<Json<PolicyGroupRecord>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    require_alert_policy_source_scopes(&operator.operator.scopes)?;
    let expression = parse_selector_expression(&request.selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
    require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    Ok(Json(
        state
            .repo
            .upsert_fleet_alert_policy(&request, &operator)
            .await
            .map_err(fleet_alert_policy_error)?,
    ))
}

pub(crate) async fn bulk_mutate_fleet_alert_policies(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<FleetAlertPolicyBulkRequest>,
) -> Result<Json<FleetAlertPolicyBulkResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    require_alert_policy_source_scopes(&operator.operator.scopes)?;
    validate_fleet_alert_policy_bulk_request(&request)?;
    Ok(Json(
        state
            .repo
            .bulk_mutate_fleet_alert_policies(
                &request,
                &operator,
                operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
            )
            .await
            .map_err(fleet_alert_policy_error)?,
    ))
}

pub(crate) async fn delete_fleet_alert_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(policy_id): Path<uuid::Uuid>,
    Json(request): Json<DeleteFleetAlertPolicyRequest>,
) -> Result<StatusCode, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    require_alert_policy_source_scopes(&operator.operator.scopes)?;
    let policy = state
        .repo
        .get_fleet_alert_policy(
            policy_id,
            operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ),
        )
        .await
        .map_err(fleet_alert_policy_error)?;
    let expression = parse_selector_expression(&policy.selector_expression)
        .map_err(|_| ApiError::conflict("invalid_selector_expression"))?
        .ok_or_else(|| ApiError::conflict("invalid_selector_expression"))?;
    require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    validate_delete_confirmation(
        request.confirmed,
        &request.reviewed_name,
        "fleet_alert_policy_delete_confirmation_required",
        "fleet_alert_policy_delete_review_invalid",
    )?;
    state
        .repo
        .delete_fleet_alert_policy(policy_id, &request.reviewed_name, &operator)
        .await
        .map_err(fleet_alert_policy_error)?;
    Ok(StatusCode::NO_CONTENT)
}

fn require_alert_policy_source_scopes(scopes: &[String]) -> Result<(), ApiError> {
    if operator_has_scope(scopes, SCOPE_FLEET_READ)
        && operator_has_scope(scopes, SCOPE_BACKUPS_READ)
    {
        Ok(())
    } else {
        Err(ApiError::forbidden("operator_scope_insufficient"))
    }
}

fn validate_fleet_alert_policy_bulk_request(
    request: &FleetAlertPolicyBulkRequest,
) -> Result<(), ApiError> {
    validate_alert_configuration_bulk_items(
        request.confirmed,
        request.items.len(),
        request.items.iter().map(|item| {
            (
                item.id,
                item.reviewed_name.as_str(),
                item.expected_updated_at.as_str(),
            )
        }),
        "fleet_alert_policy_bulk_confirmation_required",
        "fleet_alert_policy_bulk_items_invalid",
        "fleet_alert_policy_bulk_duplicate_item",
        "fleet_alert_policy_bulk_review_invalid",
        "fleet_alert_policy_bulk_expected_updated_at_invalid",
    )
}

pub(crate) async fn list_vps_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<VpsRuleQuery>,
) -> Result<Json<Vec<VpsRuleValueRecord>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .list_vps_rules(&query)
            .await
            .map_err(vps_rules_error)?,
    ))
}

pub(crate) async fn get_effective_vps_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
) -> Result<Json<Vec<VpsRuleValueRecord>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .effective_vps_rules(&client_id)
            .await
            .map_err(vps_rules_error)?,
    ))
}

pub(crate) async fn dry_run_vps_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<VpsRulesDryRunRequest>,
) -> Result<Json<VpsRulesDryRunResponse>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .dry_run_vps_rules(&request)
            .await
            .map_err(vps_rules_error)?,
    ))
}

pub(crate) async fn bulk_upsert_vps_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<VpsRulesBulkUpsertRequest>,
) -> Result<Json<VpsRulesDryRunResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    let expression = parse_selector_expression(&request.selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
    require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    Ok(Json(
        state
            .repo
            .bulk_upsert_vps_rules(&request, &operator)
            .await
            .map_err(vps_rules_error)?,
    ))
}

pub(crate) async fn bulk_unset_vps_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<VpsRulesBulkUnsetRequest>,
) -> Result<Json<VpsRulesDryRunResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "config:write")
        .await?;
    let expression = parse_selector_expression(&request.selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
    require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    Ok(Json(
        state
            .repo
            .bulk_unset_vps_rules(&request, &operator)
            .await
            .map_err(vps_rules_error)?,
    ))
}

pub(crate) async fn list_traffic_accounting(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TrafficAccountingQuery>,
) -> Result<Json<Vec<TrafficAccountingRecord>>, ApiError> {
    let operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    if let Some(selector) = query.selector_expression.as_deref() {
        if let Some(expression) = parse_selector_expression(selector)
            .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        {
            require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
        }
    }
    Ok(Json(
        state
            .repo
            .list_traffic_accounting(&query)
            .await
            .map_err(vps_rules_error)?,
    ))
}

pub(crate) async fn get_traffic_accounting(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
) -> Result<Json<TrafficAccountingRecord>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .get_traffic_accounting(&client_id)
            .await
            .map_err(|error| {
                if error.to_string().contains("traffic_accounting_not_found") {
                    ApiError::not_found("traffic_accounting_not_found")
                } else {
                    ApiError::internal(
                        "traffic_accounting_unavailable",
                        "Traffic accounting could not be loaded.",
                        error,
                    )
                }
            })?,
    ))
}

pub(crate) async fn list_policy_alerts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PolicyAlertQuery>,
) -> Result<Json<Vec<PolicyAlertRecord>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(state.repo.list_policy_alerts(&query).await.map_err(
        ApiError::internal_mapper(
            "policy_alerts_unavailable",
            "Policy alerts could not be loaded.",
        ),
    )?))
}

pub(crate) async fn list_fleet_alert_notification_channels(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertNotificationChannelQuery>,
) -> Result<Json<Vec<FleetAlertNotificationChannelView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_INTEGRATIONS_READ)
        .await?;
    validate_alert_notification_channel_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_fleet_alert_notification_channels(
                limit_or_default(query.limit),
                query.enabled,
                query.scope_kind.as_deref(),
                query.scope_value.as_deref(),
                query.delivery_kind.as_deref(),
            )
            .await
            .map_err(ApiError::internal_mapper(
                "alert_notification_channels_unavailable",
                "Alert notification channels could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn upsert_fleet_alert_notification_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateFleetAlertNotificationChannelRequest>,
) -> Result<Json<FleetAlertNotificationChannelView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_alert_notification_channel_request(&request)?;
    Ok(Json(
        state
            .repo
            .upsert_fleet_alert_notification_channel(&request, &operator)
            .await
            .map_err(fleet_alert_notification_channel_error)?,
    ))
}

pub(crate) async fn bulk_mutate_fleet_alert_notification_channels(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<FleetAlertNotificationChannelBulkRequest>,
) -> Result<Json<FleetAlertNotificationChannelBulkResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_fleet_alert_notification_channel_bulk_request(&request)?;
    Ok(Json(
        state
            .repo
            .bulk_mutate_fleet_alert_notification_channels(&request, &operator)
            .await
            .map_err(fleet_alert_notification_channel_error)?,
    ))
}

pub(crate) async fn delete_fleet_alert_notification_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(channel_id): Path<uuid::Uuid>,
    Json(request): Json<DeleteFleetAlertNotificationChannelRequest>,
) -> Result<StatusCode, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_delete_confirmation(
        request.confirmed,
        &request.reviewed_name,
        "fleet_alert_notification_channel_delete_confirmation_required",
        "fleet_alert_notification_channel_delete_review_invalid",
    )?;
    state
        .repo
        .delete_fleet_alert_notification_channel(channel_id, &request.reviewed_name, &operator)
        .await
        .map_err(fleet_alert_notification_channel_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn list_fleet_alert_notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<FleetAlertNotificationDeliveryQuery>,
) -> Result<Json<Vec<FleetAlertNotificationDeliveryView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_INTEGRATIONS_READ)
        .await?;
    validate_alert_notification_delivery_query(&query)?;
    Ok(Json(
        state
            .repo
            .list_fleet_alert_notification_deliveries(
                limit_or_default(query.limit),
                query.channel_id,
                query.alert_id.as_deref(),
                query.status.as_deref(),
            )
            .await
            .map_err(ApiError::internal_mapper(
                "alert_notification_deliveries_unavailable",
                "Alert notification deliveries could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn dispatch_fleet_alert_notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<FleetAlertNotificationDispatchRequest>,
) -> Result<Json<Vec<FleetAlertNotificationDeliveryView>>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_alert_notification_dispatch_request(&request)?;
    Ok(Json(
        state
            .dispatch_fleet_alert_notifications(&request, &operator)
            .await
            .map_err(alert_notification_delivery_error)?,
    ))
}

pub(crate) async fn process_fleet_alert_notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<FleetAlertNotificationProcessRequest>,
) -> Result<Json<Vec<FleetAlertNotificationDeliveryView>>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_INTEGRATIONS_WRITE)
        .await?;
    validate_alert_notification_process_request(&request)?;
    Ok(Json(
        state
            .process_fleet_alert_notifications(&request, &operator)
            .await
            .map_err(alert_notification_delivery_error)?,
    ))
}

fn alert_notification_delivery_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("preview_hash_mismatch") {
        return ApiError::conflict("fleet_alert_notification_preview_hash_mismatch");
    }
    if message.contains("fleet_alert_notification_dispatch_channel_limit_exceeded") {
        return ApiError::conflict("fleet_alert_notification_dispatch_channel_limit_exceeded");
    }
    ApiError::internal(
        "fleet_alert_notification_dispatch_failed",
        "The alert notification dispatch could not be completed.",
        error,
    )
}

fn validate_alert_query(query: &FleetAlertQuery) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=200).contains(&limit) {
            return Err(ApiError::bad_request("fleet_alert_limit_invalid"));
        }
    }
    if let Some(client_id) = query.client_id.as_deref() {
        if client_id.is_empty() || client_id.len() > 128 {
            return Err(ApiError::bad_request("fleet_alert_client_id_invalid"));
        }
    }
    if let Some(severity) = query.severity.as_deref() {
        if !matches!(severity, "critical" | "warning" | "info") {
            return Err(ApiError::bad_request("fleet_alert_severity_invalid"));
        }
    }
    if let Some(category) = query.category.as_deref() {
        validate_alert_token(category, "fleet_alert_category_invalid")?;
    }
    if let Some(operator_state) = query.operator_state.as_deref() {
        validate_alert_state_value(operator_state, "fleet_alert_operator_state_invalid")?;
    }
    Ok(())
}

fn validate_delete_confirmation(
    confirmed: bool,
    reviewed_name: &str,
    confirmation_error: &'static str,
    reviewed_name_error: &'static str,
) -> Result<(), ApiError> {
    if !confirmed {
        return Err(ApiError::bad_request(confirmation_error));
    }
    validate_short_required_value(reviewed_name, reviewed_name_error)?;
    Ok(())
}

fn validate_alert_state_query(query: &FleetAlertStateQuery) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=1000).contains(&limit) {
            return Err(ApiError::bad_request("fleet_alert_state_limit_invalid"));
        }
    }
    if let Some(state) = query.state.as_deref() {
        validate_alert_state_value(state, "fleet_alert_state_invalid")?;
    }
    Ok(())
}

fn validate_alert_state_request(request: &UpdateFleetAlertStateRequest) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_state_confirmation_required",
        ));
    }
    validate_alert_id(&request.alert_id)?;
    if request
        .expected_revision
        .is_some_and(|revision| revision < 0)
    {
        return Err(ApiError::bad_request(
            "fleet_alert_state_expected_revision_invalid",
        ));
    }
    validate_alert_state_action_fields(
        &request.action,
        request.muted_for_secs,
        request.reason.as_deref(),
    )
}

fn validate_bulk_alert_state_request(
    request: &BulkUpdateFleetAlertStatesRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_state_confirmation_required",
        ));
    }
    if request.items.is_empty() || request.items.len() > 1_000 {
        return Err(ApiError::bad_request("fleet_alert_state_items_invalid"));
    }
    let mut alert_ids = HashSet::with_capacity(request.items.len());
    for item in &request.items {
        validate_alert_id(&item.alert_id)?;
        if item.expected_revision < 0 {
            return Err(ApiError::bad_request(
                "fleet_alert_state_expected_revision_invalid",
            ));
        }
        if !alert_ids.insert(item.alert_id.trim()) {
            return Err(ApiError::bad_request("fleet_alert_state_duplicate_item"));
        }
    }
    validate_alert_state_action_fields(
        &request.action,
        request.muted_for_secs,
        request.reason.as_deref(),
    )
}

fn validate_alert_state_action_fields(
    action: &str,
    muted_for_secs: Option<i64>,
    reason: Option<&str>,
) -> Result<(), ApiError> {
    match action.trim() {
        "acknowledge" | "escalate" | "clear" => {
            if muted_for_secs.is_some() {
                return Err(ApiError::bad_request(
                    "fleet_alert_mute_duration_unexpected",
                ));
            }
        }
        "mute" => {
            if let Some(seconds) = muted_for_secs {
                if !(60..=90 * 24 * 60 * 60).contains(&seconds) {
                    return Err(ApiError::bad_request("fleet_alert_mute_duration_invalid"));
                }
            }
        }
        _ => return Err(ApiError::bad_request("fleet_alert_state_action_invalid")),
    }
    if reason.is_some_and(|reason| reason.len() > 1024) {
        return Err(ApiError::bad_request("fleet_alert_state_reason_too_long"));
    }
    Ok(())
}

fn validate_alert_notification_channel_query(
    query: &FleetAlertNotificationChannelQuery,
) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=1000).contains(&limit) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_channel_limit_invalid",
            ));
        }
    }
    if let Some(scope_kind) = query.scope_kind.as_deref() {
        validate_alert_notification_scope_kind(scope_kind)?;
    }
    if let Some(scope_value) = query.scope_value.as_deref() {
        validate_short_required_value(scope_value, "fleet_alert_notification_scope_value_invalid")?;
    }
    if let Some(delivery_kind) = query.delivery_kind.as_deref() {
        validate_alert_notification_delivery_kind(delivery_kind)?;
    }
    Ok(())
}

fn validate_fleet_alert_notification_channel_bulk_request(
    request: &FleetAlertNotificationChannelBulkRequest,
) -> Result<(), ApiError> {
    validate_alert_configuration_bulk_items(
        request.confirmed,
        request.items.len(),
        request.items.iter().map(|item| {
            (
                item.id,
                item.reviewed_name.as_str(),
                item.expected_updated_at.as_str(),
            )
        }),
        "fleet_alert_notification_channel_bulk_confirmation_required",
        "fleet_alert_notification_channel_bulk_items_invalid",
        "fleet_alert_notification_channel_bulk_duplicate_item",
        "fleet_alert_notification_channel_bulk_review_invalid",
        "fleet_alert_notification_channel_bulk_expected_updated_at_invalid",
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_alert_configuration_bulk_items<'a, I>(
    confirmed: bool,
    item_count: usize,
    items: I,
    confirmation_error: &'static str,
    items_error: &'static str,
    duplicate_error: &'static str,
    reviewed_name_error: &'static str,
    timestamp_error: &'static str,
) -> Result<(), ApiError>
where
    I: IntoIterator<Item = (Uuid, &'a str, &'a str)>,
{
    if !confirmed {
        return Err(ApiError::bad_request(confirmation_error));
    }
    if !(1..=ALERT_CONFIGURATION_BULK_ITEM_LIMIT).contains(&item_count) {
        return Err(ApiError::bad_request(items_error));
    }
    let mut ids = HashSet::with_capacity(item_count);
    for (id, reviewed_name, expected_updated_at) in items {
        if !ids.insert(id) {
            return Err(ApiError::bad_request(duplicate_error));
        }
        validate_short_required_value(reviewed_name, reviewed_name_error)?;
        if parse_timestamp_utc(expected_updated_at).is_none() {
            return Err(ApiError::bad_request(timestamp_error));
        }
    }
    Ok(())
}

fn validate_alert_notification_channel_request(
    request: &CreateFleetAlertNotificationChannelRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_channel_confirmation_required",
        ));
    }
    validate_short_required_value(
        &request.name,
        "fleet_alert_notification_channel_name_invalid",
    )?;
    validate_alert_notification_scope_kind(&request.scope_kind)?;
    if request.scope_kind.trim() == "global" {
        if request
            .scope_value
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_global_scope_value_invalid",
            ));
        }
    } else if request
        .scope_value
        .as_deref()
        .is_none_or(|value| value.trim().is_empty() || value.len() > 128)
    {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_scope_value_required",
        ));
    }
    if let Some(min_severity) = request.min_severity.as_deref() {
        validate_alert_severity(
            min_severity,
            "fleet_alert_notification_min_severity_invalid",
        )?;
    }
    validate_alert_token_list(
        request.categories.as_deref().unwrap_or(&[]),
        "fleet_alert_notification_category_invalid",
    )?;
    for state in request.operator_states.as_deref().unwrap_or(&[]) {
        validate_alert_state_value(state, "fleet_alert_notification_operator_state_invalid")?;
    }
    validate_alert_notification_delivery_kind(&request.delivery_kind)?;
    let target = request.target.trim();
    if target.is_empty() || target.len() > 512 || target.as_bytes().contains(&0) {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_target_invalid",
        ));
    }
    validate_webhook_rule_target(target).map_err(|error| {
        ApiError::bad_request_with_message(
            "fleet_alert_notification_target_invalid",
            error.to_string(),
        )
    })?;
    if let Some(cooldown_secs) = request.cooldown_secs {
        if !(0..=30 * 24 * 60 * 60).contains(&cooldown_secs) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_cooldown_invalid",
            ));
        }
    }
    if let Some(notes) = request.notes.as_deref() {
        if notes.len() > 1024 {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_notes_too_long",
            ));
        }
    }
    Ok(())
}

fn validate_alert_notification_delivery_query(
    query: &FleetAlertNotificationDeliveryQuery,
) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=1000).contains(&limit) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_delivery_limit_invalid",
            ));
        }
    }
    if let Some(alert_id) = query.alert_id.as_deref() {
        validate_alert_id(alert_id)?;
    }
    if let Some(status) = query.status.as_deref() {
        if !is_fleet_alert_notification_delivery_status(status) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_status_invalid",
            ));
        }
    }
    Ok(())
}

fn validate_alert_notification_dispatch_request(
    request: &FleetAlertNotificationDispatchRequest,
) -> Result<(), ApiError> {
    if !request.dry_run.unwrap_or(false) && !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_dispatch_confirmation_required",
        ));
    }
    if !request.dry_run.unwrap_or(false)
        && request
            .preview_hash
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_dispatch_preview_hash_required",
        ));
    }
    validate_alert_query(&FleetAlertQuery {
        limit: request.limit,
        client_id: request.client_id.clone(),
        severity: request.severity.clone(),
        category: request.category.clone(),
        operator_state: request.operator_state.clone(),
        include_muted: request.include_muted,
    })?;
    Ok(())
}

fn validate_alert_notification_process_request(
    request: &FleetAlertNotificationProcessRequest,
) -> Result<(), ApiError> {
    if !request.dry_run.unwrap_or(false) && !request.confirmed {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_process_confirmation_required",
        ));
    }
    if !request.dry_run.unwrap_or(false)
        && request
            .preview_hash
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_process_preview_hash_required",
        ));
    }
    if let Some(limit) = request.limit {
        if !(1..=200).contains(&limit) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_process_limit_invalid",
            ));
        }
    }
    if let Some(status) = request.status.as_deref() {
        if !is_fleet_alert_notification_delivery_process_status(status) {
            return Err(ApiError::bad_request(
                "fleet_alert_notification_process_status_invalid",
            ));
        }
    }
    if let Some(delivery_kind) = request.delivery_kind.as_deref() {
        validate_alert_notification_delivery_kind(delivery_kind)?;
    }
    Ok(())
}

fn validate_alert_notification_delivery_kind(delivery_kind: &str) -> Result<(), ApiError> {
    if delivery_kind.trim() != "webhook" {
        return Err(ApiError::bad_request(
            "fleet_alert_notification_delivery_kind_invalid",
        ));
    }
    Ok(())
}

fn validate_alert_policy_query(query: &FleetAlertPolicyQuery) -> Result<(), ApiError> {
    if let Some(limit) = query.limit {
        if !(1..=1000).contains(&limit) {
            return Err(ApiError::bad_request("fleet_alert_policy_limit_invalid"));
        }
    }
    if let Some(selector) = query.selector_expression.as_deref() {
        if selector.trim().is_empty() || selector.len() > 4096 {
            return Err(ApiError::bad_request("fleet_alert_policy_selector_invalid"));
        }
    }
    if let Some(client_id) = query.client_id.as_deref() {
        if client_id.trim().is_empty() || client_id.len() > 128 {
            return Err(ApiError::bad_request("fleet_alert_policy_client_invalid"));
        }
    }
    Ok(())
}

fn vps_rules_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("vps_rules_target_not_found") {
        return ApiError::not_found("vps_rules_target_not_found");
    }
    if message.contains("vps_rules_preview_hash_mismatch") {
        return ApiError::conflict("vps_rules_preview_hash_mismatch");
    }
    if message.contains("vps_rules_target_no_longer_available") {
        return ApiError::conflict("vps_rules_target_no_longer_available");
    }
    for code in [
        "vps_rules_confirmation_required",
        "vps_rules_operation_invalid",
        "vps_rules_values_required",
        "vps_rules_keys_required",
        "vps_rules_duplicate_key",
        "vps_rules_key_unsupported",
        "vps_rules_empty_value_invalid",
        "vps_rules_value_too_long",
        "vps_rules_preview_contains_invalid_rows",
        "traffic_reset_day_invalid",
        "traffic_selector_empty",
        "traffic_selector_empty_item",
        "traffic_selector_source_invalid",
        "traffic_selector_interface_required",
        "traffic_selector_interface_invalid",
        "traffic_selector_direction_invalid",
        "traffic_selector_duplicate",
        "traffic_selector_direction_overlap",
        "traffic_selector_too_many_items",
        "network_rate_selector_source_invalid",
        "byte_size_empty",
        "byte_size_number_invalid",
        "byte_size_unit_invalid",
        "byte_size_too_large",
        "billing_plan_price_required",
        "billing_plan_price_invalid",
        "billing_plan_currency_required",
        "billing_plan_currency_invalid",
        "billing_plan_period_required",
        "billing_plan_period_invalid",
        "billing_cycle_day_invalid",
        "billing_cycle_month_invalid",
        "billing_cycle_requires_price",
        "billing_cycle_disabled_price_invalid",
        "billing_month_cycle_requires_day",
        "billing_long_cycle_requires_month_day",
        "port_speed_unit_required",
        "port_speed_unit_invalid",
        "port_speed_value_invalid",
        "port_speed_value_too_large",
    ] {
        if message.contains(code) {
            return ApiError::bad_request(code);
        }
    }
    if message.contains("invalid selector expression") || message.contains("selector expression") {
        return ApiError::bad_request("vps_rules_selector_invalid");
    }
    ApiError::internal(
        "vps_rules_mutation_failed",
        "The VPS rule change could not be completed.",
        error,
    )
}

fn fleet_alert_policy_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("vps_rule_selector_scope_required") {
        return ApiError::forbidden("operator_scope_insufficient");
    }
    if message.contains("fleet_alert_policy_not_found") {
        return ApiError::not_found("fleet_alert_policy_not_found");
    }
    if message.contains("fleet_alert_policy_preview_hash_mismatch") {
        return ApiError::conflict("fleet_alert_policy_preview_hash_mismatch");
    }
    if message.contains("fleet_alert_policy_name_conflict") {
        return ApiError::conflict("fleet_alert_policy_name_conflict");
    }
    if message.contains("fleet_alert_policy_delete_review_stale") {
        return ApiError::conflict("fleet_alert_policy_delete_review_stale");
    }
    for code in [
        "fleet_alert_policy_bulk_review_stale",
        "fleet_alert_policy_bulk_state_stale",
        "fleet_alert_policy_bulk_snapshot_stale",
        "fleet_alert_policy_rule_version_overflow",
    ] {
        if message.contains(code) {
            return ApiError::conflict(code);
        }
    }
    if message.contains("fleet_alert_policy_rule_id_conflict") {
        return ApiError::conflict("fleet_alert_policy_rule_id_conflict");
    }
    if message.contains("fleet_alert_policy_rule_identity_conflict") {
        return ApiError::conflict("fleet_alert_policy_rule_identity_conflict");
    }
    if message.contains("fleet_alert_policy_duplicate_rule_id") {
        return ApiError::bad_request("fleet_alert_policy_duplicate_rule_id");
    }
    if message.contains("confirmation_required") {
        return ApiError::bad_request("fleet_alert_policy_confirmation_required");
    }
    if message.contains("rule name") {
        return ApiError::bad_request("fleet_alert_policy_rule_name_invalid");
    }
    if message.contains("policy name") {
        return ApiError::bad_request("fleet_alert_policy_name_invalid");
    }
    if message.contains("selector expression") {
        return ApiError::bad_request("fleet_alert_policy_selector_invalid");
    }
    if message.contains("requires at least one rule") {
        return ApiError::bad_request("fleet_alert_policy_rules_required");
    }
    if message.contains("fleet_alert_policy_condition_invalid")
        || message.contains("condition expression")
    {
        let reason = message
            .split_once("fleet_alert_policy_condition_invalid:")
            .map(|(_, reason)| reason.trim())
            .filter(|reason| !reason.is_empty())
            .unwrap_or("Enter a supported condition expression of 4096 bytes or fewer");
        return ApiError::bad_request_with_message(
            "fleet_alert_policy_condition_invalid",
            format!(
                "{reason}. Supported metrics include cpu.utilization_ratio (busy-time ratio), cpu.load_saturation (load per core), cpu.load_1 (raw load), memory.available_ratio, disk.available_ratio, and traffic quota/cycle values"
            ),
        );
    }
    if message.contains("fleet_alert_policy_severity_invalid") {
        return ApiError::bad_request("fleet_alert_policy_severity_invalid");
    }
    if message.contains("fleet_alert_policy_window_invalid") {
        return ApiError::bad_request("fleet_alert_policy_window_invalid");
    }
    if message.contains("fleet_alert_policy_traffic_selector_requires_traffic_metric") {
        return ApiError::bad_request(
            "fleet_alert_policy_traffic_selector_requires_traffic_metric",
        );
    }
    if message.contains("traffic selector") || message.contains("traffic_selector_") {
        return ApiError::bad_request("fleet_alert_policy_traffic_selector_invalid");
    }
    if message.contains("notes are too long") {
        return ApiError::bad_request("fleet_alert_policy_notes_too_long");
    }
    ApiError::internal(
        "fleet_alert_policy_mutation_failed",
        "The fleet alert policy change could not be completed.",
        error,
    )
}

fn fleet_alert_notification_channel_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("fleet_alert_notification_channel_name_conflict") {
        return ApiError::conflict("fleet_alert_notification_channel_name_conflict");
    }
    if message.contains("fleet_alert_notification_channel_not_found") {
        return ApiError::not_found("fleet_alert_notification_channel_not_found");
    }
    if message.contains("fleet_alert_notification_channel_delete_review_stale") {
        return ApiError::conflict("fleet_alert_notification_channel_delete_review_stale");
    }
    for code in [
        "fleet_alert_notification_channel_bulk_review_stale",
        "fleet_alert_notification_channel_bulk_state_stale",
        "fleet_alert_notification_channel_bulk_snapshot_stale",
    ] {
        if message.contains(code) {
            return ApiError::conflict(code);
        }
    }
    ApiError::internal(
        "fleet_alert_notification_channel_mutation_failed",
        "The notification channel change could not be completed.",
        error,
    )
}

fn validate_alert_severity(severity: &str, error: &'static str) -> Result<(), ApiError> {
    if !matches!(severity, "critical" | "warning" | "info") {
        return Err(ApiError::bad_request(error));
    }
    Ok(())
}

fn validate_alert_token_list(values: &[String], error: &'static str) -> Result<(), ApiError> {
    if values.len() > 64 {
        return Err(ApiError::bad_request(error));
    }
    for value in values {
        validate_alert_token(value, error)?;
    }
    Ok(())
}

fn validate_short_required_value(value: &str, error: &'static str) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 {
        return Err(ApiError::bad_request(error));
    }
    Ok(())
}

fn validate_alert_id(alert_id: &str) -> Result<(), ApiError> {
    let alert_id = alert_id.trim();
    if alert_id.is_empty() || alert_id.len() > 192 {
        return Err(ApiError::bad_request("fleet_alert_id_invalid"));
    }
    if !alert_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.'))
    {
        return Err(ApiError::bad_request("fleet_alert_id_invalid"));
    }
    Ok(())
}

fn validate_alert_token(value: &str, code: &'static str) -> Result<(), ApiError> {
    if value.trim().is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.'))
    {
        return Err(ApiError::bad_request(code));
    }
    Ok(())
}

fn validate_alert_state_value(state: &str, code: &'static str) -> Result<(), ApiError> {
    if matches!(
        state.trim(),
        "open" | "acknowledged" | "muted" | "escalated"
    ) {
        Ok(())
    } else {
        Err(ApiError::bad_request(code))
    }
}

fn validate_alert_notification_scope_kind(scope_kind: &str) -> Result<(), ApiError> {
    if matches!(scope_kind.trim(), "global" | "provider" | "tag" | "client") {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "fleet_alert_notification_scope_kind_invalid",
        ))
    }
}

#[cfg(test)]
#[path = "tests_routes_alerts.rs"]
mod tests;
