use std::collections::{BTreeSet, HashMap};

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
    error::ApiError,
    gateway_client::GatewayControlResponseError,
    job_request::{
        fixed_target_selection, job_command_type_label, normalized_target_client_ids,
        normalized_target_client_ids_allow_empty, validate_job_command,
    },
    model::{
        BulkResolveRequest, BulkUpdateScheduleTargetsOutcome, BulkUpdateScheduleTargetsRequest,
        BulkUpdateScheduleTargetsResponse, CreateJobRequest, CreateScheduleRequest,
        DeferScheduleRequest, EventScheduleTemplateEdgePreview,
        EventScheduleTemplateElementPreview, EventScheduleTemplatePreviewContext,
        EventScheduleTemplatePreviewResponse, ListQuery, PreviewEventScheduleTemplateRequest,
        SchedulePrivilegeMutationRequest, ScheduleTriggerKind, ScheduleView, UpdateScheduleRequest,
        UpdateScheduleTargetsRequest,
    },
    privilege::{verify_privilege_intent, SchedulePrivilegeIntent, SchedulePrivilegeIntentInput},
    repository::Repository,
    repository_schedules::{
        next_cron_runs, ScheduleSnapshotExpectation, ScheduleTargetBatchUpdate,
        ScheduleTargetBatchUpdateResult,
    },
    routes_jobs::create_job_from_saved_schedule,
    security::{operator_has_scope, require_vps_rule_selector_scope, SCOPE_SCHEDULES_READ},
    selector_expression::parse_selector_expression,
    state::AppState,
    util::limit_or_default,
};
use vpsman_common::{
    alert_event_argv_template_hash, alert_event_argv_template_uses_path,
    alert_event_expression_anchor_kinds, encode_json, parse_and_validate_alert_event_expression,
    payload_hash, render_alert_event_argv_template, render_alert_event_job_command,
    validate_alert_event_argv_template, Expression, GatewayPrivilegeVerification,
    GatewayPrivilegeVerificationBatchItem, JobCommand, PrivilegeAssertion, ALERT_EVENT_NOOP_ARGV,
};

pub(crate) const MAX_BULK_SCHEDULE_TARGET_UPDATES: usize = 1_000;

#[derive(Clone, Copy)]
enum ScheduleTargetResolutionMode {
    PreserveFrozenTargets,
    RequireLiveTargets,
}

pub(crate) async fn list_schedules(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<ScheduleView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_SCHEDULES_READ)
        .await?;
    let query = ListQuery {
        limit: Some(limit_or_default(query.limit)),
        ..query
    };
    let schedules = state
        .repo
        .query_schedules(&query)
        .await
        .map_err(ApiError::internal_mapper(
            "schedules_unavailable",
            "Schedules could not be loaded.",
        ))?;
    Ok(Json(schedules))
}

pub(crate) async fn get_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<Uuid>,
) -> Result<Json<ScheduleView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_SCHEDULES_READ)
        .await?;
    let schedule = state
        .repo
        .schedule_by_id(schedule_id)
        .await
        .map_err(map_schedule_lookup_error)?;
    Ok(Json(schedule))
}

pub(crate) async fn create_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<CreateScheduleRequest>,
) -> Result<(StatusCode, Json<ScheduleView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "schedules:write")
        .await?;
    require_event_schedule_read_scopes(&operator.operator.scopes, request.trigger_kind)?;
    validate_schedule_request(&request)?;
    if let Some(expression) = parse_selector_expression(&request.selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
    {
        require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    }
    require_schedule_confirmed(request.confirmed)?;
    request.target_client_ids = normalized_target_client_ids(&request.target_client_ids)?;
    require_selector_target_snapshot(
        &state,
        &request.selector_expression,
        &request.target_client_ids,
        "schedule_target_snapshot_stale",
    )
    .await?;
    verify_schedule_privilege_for_definition(
        &state,
        "schedule.create",
        None,
        ScheduleDefinitionRef::from_create(&request),
        None,
        false,
        request.privilege_assertion.clone(),
        ScheduleTargetResolutionMode::RequireLiveTargets,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .repo
                .create_schedule(request, &operator)
                .await
                .map_err(ApiError::internal_mapper(
                    "schedule_create_failed",
                    "The schedule could not be created.",
                ))?,
        ),
    ))
}

pub(crate) async fn preview_event_schedule_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PreviewEventScheduleTemplateRequest>,
) -> Result<Json<EventScheduleTemplatePreviewResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "schedules:write")
        .await?;
    require_event_schedule_read_scopes(&operator.operator.scopes, ScheduleTriggerKind::Event)?;
    let expression = parse_alert_event_schedule_expression(&request.event_expression)?;

    let template_argv = request.event_argv_template.clone().unwrap_or_else(|| {
        ALERT_EVENT_NOOP_ARGV
            .iter()
            .map(|value| (*value).to_string())
            .collect()
    });
    let validation = validate_alert_event_argv_template(request.event_argv_template.as_deref());
    let (triggered, resolved) = alert_event_expression_anchor_kinds(&expression);
    let previews = [(triggered, "alert.triggered"), (resolved, "alert.resolved")]
        .into_iter()
        .filter(|(eligible, _)| *eligible)
        .map(|(_, event_kind)| {
            event_schedule_edge_preview(
                event_kind,
                &template_argv,
                request.event_argv_template.as_deref(),
                validation.as_ref().err(),
            )
        })
        .collect();
    Ok(Json(EventScheduleTemplatePreviewResponse {
        uses_default_noop: request.event_argv_template.is_none(),
        template_argv: template_argv.clone(),
        previews,
        template_hash: alert_event_argv_template_hash(request.event_argv_template.as_deref())
            .unwrap_or_else(|_| {
                encode_json(&template_argv)
                    .map(|bytes| payload_hash(&bytes))
                    .unwrap_or_else(|_| payload_hash(b"invalid-event-template"))
            }),
    }))
}

fn event_schedule_edge_preview(
    event_kind: &str,
    template_argv: &[String],
    template: Option<&[String]>,
    validation_error: Option<&String>,
) -> EventScheduleTemplateEdgePreview {
    let fixture = canonical_event_schedule_preview_context(event_kind);
    let rendered = validation_error
        .is_none()
        .then(|| render_alert_event_argv_template(template, &fixture))
        .transpose()
        .ok()
        .flatten();
    let rendered_hash = validation_error.is_none().then(|| {
        render_alert_event_job_command(template, &fixture)
            .ok()
            .map(|(_, hash)| hash)
    });
    let rendered_hash = rendered_hash.flatten();
    let elements = template_argv
        .iter()
        .enumerate()
        .map(|(index, template_element)| {
            if let Some(value) = rendered.as_ref().and_then(|argv| argv.get(index)) {
                return EventScheduleTemplateElementPreview {
                    index,
                    template: template_element.clone(),
                    rendered: Some(value.clone()),
                    error_code: None,
                    error_message: None,
                };
            }
            let message = validation_error
                .cloned()
                .or_else(|| render_alert_event_argv_template(template, &fixture).err());
            EventScheduleTemplateElementPreview {
                index,
                template: template_element.clone(),
                rendered: None,
                error_code: Some("event_template_render_failed".to_string()),
                error_message: Some(
                    message.unwrap_or_else(|| "event argv could not be rendered".to_string()),
                ),
            }
        })
        .collect();
    EventScheduleTemplateEdgePreview {
        rendered_argv: rendered,
        context: EventScheduleTemplatePreviewContext {
            event_kind: event_kind.to_string(),
            alert_title: "Canonical lifecycle preview".to_string(),
            alert_category: "job".to_string(),
            alert_severity: "critical".to_string(),
            policy_name: "Preview policy".to_string(),
            policy_rule_name: "Preview rule".to_string(),
        },
        elements,
        rendered_hash,
    }
}

fn canonical_event_schedule_preview_context(event_kind: &str) -> serde_json::Value {
    let resolved = event_kind == "alert.resolved";
    serde_json::json!({
        "event": {
            "id": "fleet-alert:00000000-0000-4000-8000-000000000001:preview",
            "kind": event_kind,
            "occurred_at": "2026-08-18T00:00:00Z",
            "recorded_at": "2026-08-18T00:00:01Z"
        },
        "alert": {
            "id": "fleet-alert:preview",
            "public_id": "fleet-alert:preview",
            "episode_id": "00000000-0000-4000-8000-000000000001",
            "title": "Canonical lifecycle preview",
            "detail": "Preview rule matched canonical evidence.",
            "category": "job",
            "severity": "critical",
            "record_kind": "event",
            "lifecycle_state": if resolved { "resolved" } else { "triggered" },
            "trigger_generation": 1,
            "source_status": "failed",
            "resolution_reason": if resolved { Some("policy_time_elapsed") } else { None },
            "client_id": "preview-client",
            "target_kind": "job",
            "target_id": "00000000-0000-4000-8000-000000000002"
        },
        "policy": {
            "id": "00000000-0000-4000-8000-000000000003",
            "name": "Preview policy"
        },
        "policy_rule": {
            "id": "00000000-0000-4000-8000-000000000004",
            "name": "Preview rule",
            "rule_version": 1,
            "rule_kind": "occurrence",
            "evidence_source": "job.terminal",
            "system_seed_key": "job.general_hard_failure",
            "trigger_meta_condition": {
                "kind": "immediate",
                "window_seconds": 0
            },
            "resolve_meta_condition": {
                "kind": "elapsed_since_trigger",
                "window_seconds": 604800
            }
        },
        "schedule": {
            "id": "00000000-0000-4000-8000-000000000005",
            "name": "Preview schedule",
            "definition_revision": 1,
            "fixed_target_count": 1,
            "matched_subject_count": 1
        }
    })
}

pub(crate) async fn update_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<Uuid>,
    Json(mut request): Json<UpdateScheduleRequest>,
) -> Result<Json<ScheduleView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "schedules:write")
        .await?;
    require_event_schedule_read_scopes(&operator.operator.scopes, request.trigger_kind)?;
    validate_update_schedule_request(&request)?;
    require_schedule_confirmed(request.confirmed)?;
    request.target_client_ids =
        normalized_target_client_ids_allow_empty(&request.target_client_ids)?;
    request.expected_target_client_ids =
        normalized_target_client_ids_allow_empty(&request.expected_target_client_ids)?;
    let expectation = ScheduleSnapshotExpectation {
        selector_expression: request.expected_selector_expression.clone(),
        target_client_ids: request.expected_target_client_ids.clone(),
        definition_revision: request.expected_definition_revision,
    };
    let current = state
        .repo
        .schedule_by_id(schedule_id)
        .await
        .map_err(map_schedule_lookup_error)?;
    require_schedule_snapshot(&current, &expectation)?;
    let selector_unchanged =
        request.selector_expression.trim() == expectation.selector_expression.trim();
    if selector_unchanged {
        if !same_target_client_ids(&request.target_client_ids, &expectation.target_client_ids) {
            return Err(ApiError::conflict("schedule_target_snapshot_stale"));
        }
        request.target_client_ids = current.target_client_ids.clone();
    } else {
        if let Some(expression) = parse_selector_expression(&request.selector_expression)
            .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        {
            require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
        }
        require_selector_target_snapshot(
            &state,
            &request.selector_expression,
            &request.target_client_ids,
            "schedule_target_snapshot_stale",
        )
        .await?;
    }
    verify_schedule_privilege_for_definition(
        &state,
        "schedule.update",
        Some(schedule_id),
        ScheduleDefinitionRef::from_update(&request),
        None,
        false,
        request.privilege_assertion.clone(),
        if selector_unchanged {
            ScheduleTargetResolutionMode::PreserveFrozenTargets
        } else {
            ScheduleTargetResolutionMode::RequireLiveTargets
        },
    )
    .await?;
    Ok(Json(
        state
            .repo
            .update_schedule_record(schedule_id, request.into(), Some(&expectation), &operator)
            .await
            .map_err(map_schedule_snapshot_error)?,
    ))
}

pub(crate) async fn update_schedule_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<Uuid>,
    Json(request): Json<UpdateScheduleTargetsRequest>,
) -> Result<Json<ScheduleView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "schedules:write")
        .await?;
    require_schedule_confirmed(request.confirmed)?;
    let schedule = state
        .repo
        .schedule_by_id(schedule_id)
        .await
        .map_err(map_schedule_lookup_error)?;
    require_event_schedule_read_scopes(&operator.operator.scopes, schedule.trigger_kind)?;
    require_schedule_revision(&schedule, request.expected_definition_revision)?;
    require_valid_schedule_definition(&schedule)?;
    let selector_expression = schedule.selector_expression.trim().to_string();
    if selector_expression.is_empty() {
        return Err(ApiError::conflict("schedule_selector_missing"));
    }
    let expression = parse_selector_expression(&selector_expression)
        .map_err(|_| ApiError::conflict("schedule_selector_invalid"))?
        .ok_or_else(|| ApiError::conflict("schedule_selector_invalid"))?;
    require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    let mut target_client_ids = state
        .repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: selector_expression.clone(),
        })
        .await
        .map_err(ApiError::internal_mapper(
            "schedule_targets_resolution_failed",
            "Schedule targets could not be resolved.",
        ))?
        .targets
        .into_iter()
        .map(|target| target.id)
        .collect::<Vec<_>>();
    target_client_ids.sort();
    target_client_ids.dedup();
    let expectation = ScheduleSnapshotExpectation {
        selector_expression: schedule.selector_expression.clone(),
        target_client_ids: schedule.target_client_ids.clone(),
        definition_revision: request.expected_definition_revision,
    };
    let mut current_target_client_ids = schedule.target_client_ids.clone();
    current_target_client_ids.sort();
    current_target_client_ids.dedup();
    if current_target_client_ids == target_client_ids {
        return Err(ApiError::conflict("schedule_targets_already_current"));
    }
    verify_schedule_privilege_for_stored_view(
        &state,
        &schedule,
        StoredSchedulePrivilegeRequest {
            action: "schedule.targets.update",
            selector_expression: &selector_expression,
            target_client_ids: &target_client_ids,
            enabled: schedule.enabled,
            deferred_until: schedule.deferred_until.as_deref(),
            deleted: false,
            assertion: request.privilege_assertion.clone(),
        },
    )
    .await?;
    Ok(Json(
        state
            .repo
            .update_schedule_targets(
                schedule_id,
                target_client_ids,
                Some(&expectation),
                &operator,
            )
            .await
            .map_err(map_schedule_snapshot_error)?,
    ))
}

struct PendingScheduleTargetUpdate {
    index: usize,
    request: crate::model::BulkUpdateScheduleTargetsItemRequest,
    schedule: ScheduleView,
    selector_expression: String,
    expression: Expression,
}

pub(crate) fn validate_bulk_schedule_target_selection(
    request: &BulkUpdateScheduleTargetsRequest,
) -> Result<(), ApiError> {
    require_schedule_confirmed(request.confirmed)?;
    if request.items.is_empty() {
        return Err(ApiError::bad_request("schedule_target_selection_required"));
    }
    if request.items.len() > MAX_BULK_SCHEDULE_TARGET_UPDATES {
        return Err(ApiError::bad_request("schedule_target_selection_too_large"));
    }
    let unique = request
        .items
        .iter()
        .map(|item| item.schedule_id)
        .collect::<BTreeSet<_>>();
    if unique.len() != request.items.len() {
        return Err(ApiError::bad_request("schedule_target_selection_duplicate"));
    }
    Ok(())
}

fn rejected_schedule_target_outcome(
    schedule_id: Uuid,
    error: ApiError,
) -> BulkUpdateScheduleTargetsOutcome {
    BulkUpdateScheduleTargetsOutcome {
        schedule_id,
        status: "rejected",
        schedule: None,
        error_code: Some(error.code.to_string()),
    }
}

fn map_schedule_privilege_batch_error(error: anyhow::Error) -> ApiError {
    if error.to_string().contains("ReplayProtectionSaturated") {
        ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "privilege_replay_protection_saturated",
            error,
            public_message: Some(
                "Privilege verification is temporarily saturated; no schedule target was changed."
                    .to_string(),
            ),
        }
    } else if error
        .downcast_ref::<GatewayControlResponseError>()
        .is_some_and(|response| matches!(response.status_code, 403 | 409))
    {
        ApiError::forbidden("privilege_verification_failed")
    } else {
        ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "privilege_verification_unavailable",
            error,
            public_message: Some(
                "The gateway could not verify schedule-target privileges; no schedule target was changed."
                    .to_string(),
            ),
        }
    }
}

pub(crate) fn apply_schedule_privilege_batch_results(
    results: Vec<vpsman_common::GatewayPrivilegeVerificationBatchItemResult>,
    candidates: Vec<(usize, ScheduleTargetBatchUpdate)>,
    outcomes: &mut [Option<BulkUpdateScheduleTargetsOutcome>],
) -> Result<Vec<(usize, ScheduleTargetBatchUpdate)>, ApiError> {
    if results.len() != candidates.len()
        || results
            .iter()
            .zip(&candidates)
            .any(|(result, (_, update))| result.request_id != update.schedule_id.to_string())
    {
        return Err(ApiError::internal(
            "privilege_verification_result_invalid",
            "The gateway returned an invalid schedule-target privilege result set.",
            anyhow::anyhow!("bulk privilege results did not preserve request order"),
        ));
    }
    let mut accepted = Vec::new();
    for (result, (index, update)) in results.into_iter().zip(candidates) {
        if result.approved {
            accepted.push((index, update));
        } else {
            outcomes[index] = Some(rejected_schedule_target_outcome(
                update.schedule_id,
                ApiError::forbidden("privilege_verification_failed"),
            ));
        }
    }
    Ok(accepted)
}

pub(crate) async fn bulk_update_schedule_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkUpdateScheduleTargetsRequest>,
) -> Result<Json<BulkUpdateScheduleTargetsResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "schedules:write")
        .await?;
    validate_bulk_schedule_target_selection(&request)?;

    let schedule_ids = request
        .items
        .iter()
        .map(|item| item.schedule_id)
        .collect::<Vec<_>>();
    let schedules = state
        .repo
        .schedules_by_ids(&schedule_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "schedules_unavailable",
            "Schedules could not be loaded.",
        ))?
        .into_iter()
        .map(|schedule| (schedule.id, schedule))
        .collect::<HashMap<_, _>>();
    let mut outcomes = vec![None; request.items.len()];
    let mut pending = Vec::new();
    for (index, item) in request.items.into_iter().enumerate() {
        let Some(schedule) = schedules.get(&item.schedule_id).cloned() else {
            outcomes[index] = Some(rejected_schedule_target_outcome(
                item.schedule_id,
                ApiError::not_found("schedule_not_found"),
            ));
            continue;
        };
        let validation = (|| -> Result<(String, Expression), ApiError> {
            require_event_schedule_read_scopes(&operator.operator.scopes, schedule.trigger_kind)?;
            require_schedule_revision(&schedule, item.expected_definition_revision)?;
            require_valid_schedule_definition(&schedule)?;
            let selector_expression = schedule.selector_expression.trim().to_string();
            if selector_expression.is_empty() {
                return Err(ApiError::conflict("schedule_selector_missing"));
            }
            let expression = parse_selector_expression(&selector_expression)
                .map_err(|_| ApiError::conflict("schedule_selector_invalid"))?
                .ok_or_else(|| ApiError::conflict("schedule_selector_invalid"))?;
            require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
            Ok((selector_expression, expression))
        })();
        match validation {
            Ok((selector_expression, expression)) => {
                pending.push(PendingScheduleTargetUpdate {
                    index,
                    request: item,
                    schedule,
                    selector_expression,
                    expression,
                });
            }
            Err(error) => {
                outcomes[index] = Some(rejected_schedule_target_outcome(item.schedule_id, error));
            }
        }
    }

    let agents = if pending.is_empty() {
        Vec::new()
    } else {
        state
            .repo
            .list_agents()
            .await
            .map_err(ApiError::internal_mapper(
                "schedule_targets_resolution_failed",
                "Schedule targets could not be resolved.",
            ))?
    };
    let needs_rules = pending
        .iter()
        .any(|update| vpsman_common::expression_references_vps_rules(&update.expression));
    let rules_by_client = if needs_rules {
        state
            .repo
            .vps_rule_contexts_for_agents(&agents)
            .await
            .map_err(ApiError::internal_mapper(
                "schedule_targets_resolution_failed",
                "Schedule targets could not be resolved.",
            ))?
    } else {
        HashMap::new()
    };

    #[cfg(test)]
    let test_auto_approve = state.gateway.test_privilege_auto_approves();
    #[cfg(not(test))]
    let test_auto_approve = false;
    let mut accepted = Vec::new();
    let mut verification_candidates = Vec::new();
    let mut verification_items = Vec::new();
    for update in pending {
        let mut target_client_ids = Repository::resolve_agents_for_expression_with_rule_contexts(
            &agents,
            &update.expression,
            &rules_by_client,
        )
        .into_iter()
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
        target_client_ids.sort();
        target_client_ids.dedup();
        if same_target_client_ids(&update.schedule.target_client_ids, &target_client_ids) {
            outcomes[update.index] = Some(rejected_schedule_target_outcome(
                update.schedule.id,
                ApiError::conflict("schedule_targets_already_current"),
            ));
            continue;
        }
        let repository_update = ScheduleTargetBatchUpdate {
            schedule_id: update.schedule.id,
            target_client_ids: target_client_ids.clone(),
            expectation: ScheduleSnapshotExpectation {
                selector_expression: update.schedule.selector_expression.clone(),
                target_client_ids: update.schedule.target_client_ids.clone(),
                definition_revision: update.request.expected_definition_revision,
            },
        };
        if test_auto_approve {
            accepted.push((update.index, repository_update));
            continue;
        }
        if !state.gateway.privilege_configured() {
            return Err(ApiError::conflict("gateway_control_url_missing"));
        }
        let Some(assertion) = update.request.privilege_assertion.clone() else {
            outcomes[update.index] = Some(rejected_schedule_target_outcome(
                update.schedule.id,
                ApiError::forbidden("privilege_assertion_required"),
            ));
            continue;
        };
        let resolved_targets = if target_client_ids.is_empty() {
            Vec::new()
        } else {
            normalized_target_client_ids(&target_client_ids)?
        };
        let request_id = update.schedule.id.to_string();
        let intent = serde_json::to_string(&stored_schedule_privilege_intent(
            &update.schedule,
            &request_id,
            &resolved_targets,
            "schedule.targets.update",
            &update.selector_expression,
            update.schedule.enabled,
            update.schedule.deferred_until.as_deref(),
            false,
        ))
        .map_err(|error| {
            ApiError::internal(
                "privilege_intent_invalid",
                "The schedule-target privilege intent could not be prepared.",
                error.into(),
            )
        })?;
        verification_items.push(GatewayPrivilegeVerificationBatchItem {
            request_id,
            verification: GatewayPrivilegeVerification { intent, assertion },
        });
        verification_candidates.push((update.index, repository_update));
    }

    if !verification_items.is_empty() {
        state.refresh_gateway_dispatch_timeouts();
        let verification = state
            .gateway
            .verify_privileges(verification_items)
            .await
            .map_err(map_schedule_privilege_batch_error)?;
        accepted.extend(apply_schedule_privilege_batch_results(
            verification.results,
            verification_candidates,
            &mut outcomes,
        )?);
    }

    if !accepted.is_empty() {
        let repository_updates = accepted
            .iter()
            .map(|(_, update)| update.clone())
            .collect::<Vec<_>>();
        let repository_outcomes = state
            .repo
            .update_schedule_targets_bulk(&repository_updates, &operator)
            .await
            .map_err(ApiError::internal_mapper(
                "schedule_target_batch_update_failed",
                "Schedule targets could not be updated.",
            ))?;
        for ((index, expected), outcome) in accepted.into_iter().zip(repository_outcomes) {
            let response = match outcome {
                ScheduleTargetBatchUpdateResult::Updated(schedule) => {
                    debug_assert_eq!(schedule.id, expected.schedule_id);
                    BulkUpdateScheduleTargetsOutcome {
                        schedule_id: schedule.id,
                        status: "updated",
                        schedule: Some(*schedule),
                        error_code: None,
                    }
                }
                ScheduleTargetBatchUpdateResult::Rejected {
                    schedule_id,
                    error_code,
                } => {
                    debug_assert_eq!(schedule_id, expected.schedule_id);
                    BulkUpdateScheduleTargetsOutcome {
                        schedule_id,
                        status: "rejected",
                        schedule: None,
                        error_code: Some(error_code.to_string()),
                    }
                }
            };
            outcomes[index] = Some(response);
        }
    }

    Ok(Json(BulkUpdateScheduleTargetsResponse {
        outcomes: outcomes
            .into_iter()
            .map(|outcome| outcome.expect("every validated schedule request has an outcome"))
            .collect(),
    }))
}

pub(crate) async fn enable_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<Uuid>,
    Json(request): Json<SchedulePrivilegeMutationRequest>,
) -> Result<Json<ScheduleView>, ApiError> {
    mutate_schedule_enabled(state, headers, schedule_id, request, true).await
}

pub(crate) async fn disable_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<Uuid>,
    Json(request): Json<SchedulePrivilegeMutationRequest>,
) -> Result<Json<ScheduleView>, ApiError> {
    mutate_schedule_enabled(state, headers, schedule_id, request, false).await
}

pub(crate) async fn defer_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<Uuid>,
    Json(request): Json<DeferScheduleRequest>,
) -> Result<Json<ScheduleView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "schedules:write")
        .await?;
    validate_defer_schedule_request(&request)?;
    require_schedule_confirmed(request.confirmed)?;
    let schedule = state
        .repo
        .schedule_by_id(schedule_id)
        .await
        .map_err(map_schedule_lookup_error)?;
    require_schedule_revision(&schedule, request.expected_definition_revision)?;
    require_valid_schedule_definition(&schedule)?;
    verify_schedule_privilege_for_view(
        &state,
        "schedule.defer",
        &schedule,
        schedule.enabled,
        Some(request.deferred_until.as_str()),
        false,
        request.privilege_assertion.clone(),
    )
    .await?;
    Ok(Json(
        state
            .repo
            .defer_schedule(
                schedule_id,
                &request.deferred_until,
                request.reason.as_deref(),
                request.expected_definition_revision,
                &operator,
            )
            .await
            .map_err(map_schedule_snapshot_error)?,
    ))
}

pub(crate) async fn apply_schedule_now(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<Uuid>,
    Json(request): Json<SchedulePrivilegeMutationRequest>,
) -> Result<(StatusCode, Json<crate::model::CreateJobResponse>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if !operator_has_scope(&operator.operator.scopes, "schedules:write") {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    require_schedule_confirmed(request.confirmed)?;
    let schedule = state
        .repo
        .schedule_by_id(schedule_id)
        .await
        .map_err(map_schedule_lookup_error)?;
    require_schedule_revision(&schedule, request.expected_definition_revision)?;
    if schedule.trigger_kind == ScheduleTriggerKind::Event {
        return Err(ApiError::conflict("event_schedule_apply_now_unsupported"));
    }
    let operation = require_valid_schedule_operation(&schedule)?.clone();
    verify_schedule_privilege_for_view(
        &state,
        "schedule.apply_now",
        &schedule,
        schedule.enabled,
        schedule.deferred_until.as_deref(),
        false,
        request.privilege_assertion.clone(),
    )
    .await?;
    let job_request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: schedule.selector_expression.clone(),
        target_client_ids: schedule.target_client_ids.clone(),
        destructive: false,
        confirmed: true,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(operation),
        max_timeout_secs: Some(state.schedule_apply_now_max_timeout_secs()),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };
    create_job_from_saved_schedule(&state, &operator, job_request, schedule_id).await
}

pub(crate) async fn delete_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<Uuid>,
    Json(request): Json<SchedulePrivilegeMutationRequest>,
) -> Result<StatusCode, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "schedules:write")
        .await?;
    require_schedule_confirmed(request.confirmed)?;
    let schedule = state
        .repo
        .schedule_by_id(schedule_id)
        .await
        .map_err(map_schedule_lookup_error)?;
    require_schedule_revision(&schedule, request.expected_definition_revision)?;
    verify_schedule_privilege_for_view(
        &state,
        "schedule.delete",
        &schedule,
        false,
        schedule.deferred_until.as_deref(),
        true,
        request.privilege_assertion.clone(),
    )
    .await?;
    state
        .repo
        .soft_delete_schedule(schedule_id, request.expected_definition_revision, &operator)
        .await
        .map_err(map_schedule_snapshot_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mutate_schedule_enabled(
    state: AppState,
    headers: HeaderMap,
    schedule_id: Uuid,
    request: SchedulePrivilegeMutationRequest,
    enabled: bool,
) -> Result<Json<ScheduleView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "schedules:write")
        .await?;
    require_schedule_confirmed(request.confirmed)?;
    let schedule = state
        .repo
        .schedule_by_id(schedule_id)
        .await
        .map_err(map_schedule_lookup_error)?;
    require_schedule_revision(&schedule, request.expected_definition_revision)?;
    if enabled {
        require_event_schedule_read_scopes(&operator.operator.scopes, schedule.trigger_kind)?;
    }
    if enabled {
        require_valid_schedule_definition(&schedule)?;
    }
    if enabled && schedule.cadence_error.is_some() {
        return Err(ApiError::bad_request("schedule_cron_invalid"));
    }
    verify_schedule_privilege_for_view(
        &state,
        if enabled {
            "schedule.enable"
        } else {
            "schedule.disable"
        },
        &schedule,
        enabled,
        schedule.deferred_until.as_deref(),
        false,
        request.privilege_assertion.clone(),
    )
    .await?;
    Ok(Json(
        state
            .repo
            .set_schedule_enabled(
                schedule_id,
                enabled,
                request.expected_definition_revision,
                &operator,
            )
            .await
            .map_err(map_schedule_snapshot_error)?,
    ))
}

fn require_schedule_confirmed(confirmed: bool) -> Result<(), ApiError> {
    if confirmed {
        Ok(())
    } else {
        Err(ApiError::conflict(
            "schedule_mutation_requires_confirmation",
        ))
    }
}

fn require_schedule_revision(
    schedule: &ScheduleView,
    expected_definition_revision: i64,
) -> Result<(), ApiError> {
    if schedule.definition_revision == expected_definition_revision {
        Ok(())
    } else {
        Err(ApiError::conflict("schedule_snapshot_stale"))
    }
}

fn require_event_schedule_read_scopes(
    scopes: &[String],
    trigger_kind: ScheduleTriggerKind,
) -> Result<(), ApiError> {
    if trigger_kind == ScheduleTriggerKind::Cron
        || (operator_has_scope(scopes, "jobs:write")
            && operator_has_scope(scopes, "fleet:read")
            && operator_has_scope(scopes, "backups:read"))
    {
        Ok(())
    } else {
        Err(ApiError::forbidden("operator_scope_insufficient"))
    }
}

pub(crate) fn validate_schedule_request(request: &CreateScheduleRequest) -> Result<(), ApiError> {
    validate_schedule_definition(ScheduleDefinitionRef::from_create(request), false)
}

pub(crate) fn validate_update_schedule_request(
    request: &UpdateScheduleRequest,
) -> Result<(), ApiError> {
    validate_schedule_definition(ScheduleDefinitionRef::from_update(request), true)
}

fn validate_schedule_definition(
    request: ScheduleDefinitionRef<'_>,
    allow_empty_targets: bool,
) -> Result<(), ApiError> {
    if request.name.trim().is_empty() {
        return Err(ApiError::bad_request("schedule_name_required"));
    }
    if request.name.len() > 120 {
        return Err(ApiError::bad_request("schedule_name_too_long"));
    }
    if allow_empty_targets {
        normalized_target_client_ids_allow_empty(request.target_client_ids)?;
    } else {
        normalized_target_client_ids(request.target_client_ids)?;
    }
    if !request.selector_expression.trim().is_empty() {
        parse_selector_expression(request.selector_expression)
            .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
    }
    if !(1..=100).contains(&request.max_failures) {
        return Err(ApiError::bad_request("schedule_max_failures_out_of_range"));
    }
    match request.trigger_kind {
        ScheduleTriggerKind::Cron => {
            if request.event_expression.is_some() || request.event_argv_template.is_some() {
                return Err(ApiError::bad_request("schedule_trigger_shape_invalid"));
            }
            let operation = request
                .operation
                .ok_or_else(|| ApiError::bad_request("schedule_operation_required"))?;
            let cron_expr = request
                .cron_expr
                .ok_or_else(|| ApiError::bad_request("schedule_cron_required"))?;
            if request.timezone != Some("UTC") {
                return Err(ApiError::bad_request("schedule_timezone_must_be_utc"));
            }
            if cron_expr.split_whitespace().count() != 5 {
                return Err(ApiError::bad_request("schedule_cron_must_be_5_field"));
            }
            if next_cron_runs(cron_expr, 1).is_err() {
                return Err(ApiError::bad_request("schedule_cron_invalid"));
            }
            if !matches!(
                request.catch_up_policy,
                Some("skip_missed" | "run_once" | "run_all_limited")
            ) {
                return Err(ApiError::bad_request("schedule_catch_up_policy_invalid"));
            }
            if !request
                .catch_up_limit
                .is_some_and(|value| (1..=25).contains(&value))
            {
                return Err(ApiError::bad_request(
                    "schedule_catch_up_limit_out_of_range",
                ));
            }
            if !request
                .retry_delay_secs
                .is_some_and(|value| (1..=86_400).contains(&value))
            {
                return Err(ApiError::bad_request("schedule_retry_delay_out_of_range"));
            }
            validate_job_command(operation)?;
            validate_schedulable_job_command(operation)
        }
        ScheduleTriggerKind::Event => {
            if request.operation.is_some()
                || request.cron_expr.is_some()
                || request.timezone.is_some()
                || request.catch_up_policy.is_some()
                || request.catch_up_limit.is_some()
                || request.retry_delay_secs.is_some()
            {
                return Err(ApiError::bad_request("schedule_trigger_shape_invalid"));
            }
            let expression = request
                .event_expression
                .ok_or_else(|| ApiError::bad_request("schedule_event_expression_required"))?;
            let expression = parse_alert_event_schedule_expression(expression)?;
            validate_alert_event_argv_template(request.event_argv_template)
                .map_err(|_| ApiError::bad_request("schedule_event_argv_template_invalid"))?;
            let (triggered, _) = alert_event_expression_anchor_kinds(&expression);
            if triggered
                && alert_event_argv_template_uses_path(
                    request.event_argv_template,
                    "alert.resolution_reason",
                )
                .map_err(|_| ApiError::bad_request("schedule_event_argv_template_invalid"))?
            {
                return Err(ApiError::bad_request(
                    "schedule_event_argv_resolution_reason_not_guaranteed",
                ));
            }
            Ok(())
        }
    }
}

fn parse_alert_event_schedule_expression(
    expression: &str,
) -> Result<vpsman_common::Expression, ApiError> {
    parse_and_validate_alert_event_expression(expression).map_err(|error| match error.as_str() {
        "event expression is empty" => ApiError::bad_request("schedule_event_expression_required"),
        "event_expression_missing_lifecycle_anchor" => {
            ApiError::bad_request("schedule_event_expression_requires_alert_edge")
        }
        "event_expression_not_alert_only" => {
            ApiError::bad_request("schedule_event_expression_source_not_allowed")
        }
        _ => ApiError::bad_request("schedule_event_expression_invalid"),
    })
}

fn validate_schedulable_job_command(command: &JobCommand) -> Result<(), ApiError> {
    if matches!(command, JobCommand::RuntimeConfigSync { .. }) {
        return Err(ApiError::bad_request(
            "runtime_config_sync_is_server_issued",
        ));
    }
    if matches!(command, JobCommand::AgentStop | JobCommand::AgentRestart) {
        return Err(ApiError::bad_request("agent_lifecycle_not_schedulable"));
    }
    Ok(())
}

fn validate_defer_schedule_request(request: &DeferScheduleRequest) -> Result<(), ApiError> {
    let deferred_until = DateTime::parse_from_rfc3339(&request.deferred_until)
        .map_err(|_| ApiError::bad_request("schedule_deferred_until_invalid"))?
        .with_timezone(&Utc);
    if deferred_until <= Utc::now() {
        return Err(ApiError::bad_request(
            "schedule_deferred_until_must_be_future",
        ));
    }
    if request
        .reason
        .as_deref()
        .is_some_and(|reason| reason.len() > 240 || reason.chars().any(char::is_control))
    {
        return Err(ApiError::bad_request("schedule_defer_reason_invalid"));
    }
    Ok(())
}

async fn verify_schedule_privilege_for_definition(
    state: &AppState,
    action: &str,
    schedule_id: Option<Uuid>,
    request: ScheduleDefinitionRef<'_>,
    deferred_until: Option<&str>,
    deleted: bool,
    assertion: Option<PrivilegeAssertion>,
    target_resolution_mode: ScheduleTargetResolutionMode,
) -> Result<(), ApiError> {
    let resolved_targets = match target_resolution_mode {
        ScheduleTargetResolutionMode::PreserveFrozenTargets => {
            normalized_target_client_ids_allow_empty(request.target_client_ids)?
        }
        ScheduleTargetResolutionMode::RequireLiveTargets => {
            resolved_schedule_targets(state, request.target_client_ids).await?
        }
    };
    let (operation_payload_hash, command_type) = match request.trigger_kind {
        ScheduleTriggerKind::Cron => {
            let operation_payload = encode_json(
                request
                    .operation
                    .ok_or_else(|| ApiError::bad_request("schedule_operation_required"))?,
            )
            .map_err(|error| {
                ApiError::internal(
                    "schedule_privilege_intent_failed",
                    "The schedule privilege request could not be prepared.",
                    anyhow::Error::from(error),
                )
            })?;
            (
                payload_hash(&operation_payload),
                request
                    .operation
                    .map(job_command_type_label)
                    .unwrap_or("invalid"),
            )
        }
        ScheduleTriggerKind::Event => (
            alert_event_argv_template_hash(request.event_argv_template)
                .map_err(|_| ApiError::bad_request("schedule_event_argv_template_invalid"))?,
            "shell",
        ),
    };
    let schedule_id = schedule_id.map(|id| id.to_string());
    let privilege_intent = SchedulePrivilegeIntent::new(SchedulePrivilegeIntentInput {
        action,
        schedule_id: schedule_id.as_deref(),
        definition_revision: request.definition_revision,
        name: request.name,
        command_type,
        operation_payload_hash: &operation_payload_hash,
        selector_expression: request.selector_expression,
        resolved_targets: &resolved_targets,
        trigger_kind: match request.trigger_kind {
            ScheduleTriggerKind::Cron => "cron",
            ScheduleTriggerKind::Event => "event",
        },
        cron_expr: request.cron_expr,
        timezone: request.timezone,
        event_expression: request.event_expression,
        enabled: request.enabled,
        catch_up_policy: request.catch_up_policy,
        catch_up_limit: request.catch_up_limit,
        retry_delay_secs: request.retry_delay_secs,
        max_failures: request.max_failures,
        deferred_until,
        deleted,
    });
    verify_privilege_intent(state, &privilege_intent, assertion).await
}

async fn verify_schedule_privilege_for_view(
    state: &AppState,
    action: &str,
    schedule: &ScheduleView,
    enabled: bool,
    deferred_until: Option<&str>,
    deleted: bool,
    assertion: Option<PrivilegeAssertion>,
) -> Result<(), ApiError> {
    verify_schedule_privilege_for_stored_view(
        state,
        schedule,
        StoredSchedulePrivilegeRequest {
            action,
            selector_expression: &schedule.selector_expression,
            target_client_ids: &schedule.target_client_ids,
            enabled,
            deferred_until,
            deleted,
            assertion,
        },
    )
    .await
}

struct StoredSchedulePrivilegeRequest<'a> {
    action: &'a str,
    selector_expression: &'a str,
    target_client_ids: &'a [String],
    enabled: bool,
    deferred_until: Option<&'a str>,
    deleted: bool,
    assertion: Option<PrivilegeAssertion>,
}

fn stored_schedule_privilege_intent<'a>(
    schedule: &'a ScheduleView,
    schedule_id: &'a str,
    resolved_targets: &'a [String],
    action: &'a str,
    selector_expression: &'a str,
    enabled: bool,
    deferred_until: Option<&'a str>,
    deleted: bool,
) -> SchedulePrivilegeIntent<'a> {
    SchedulePrivilegeIntent::new(SchedulePrivilegeIntentInput {
        action,
        schedule_id: Some(schedule_id),
        definition_revision: Some(schedule.definition_revision),
        name: &schedule.name,
        command_type: &schedule.command_type,
        operation_payload_hash: &schedule.operation_payload_hash,
        selector_expression,
        resolved_targets,
        trigger_kind: match schedule.trigger_kind {
            ScheduleTriggerKind::Cron => "cron",
            ScheduleTriggerKind::Event => "event",
        },
        cron_expr: schedule.cron_expr.as_deref(),
        timezone: schedule.timezone.as_deref(),
        event_expression: schedule.event_expression.as_deref(),
        enabled,
        catch_up_policy: schedule.catch_up_policy.as_deref(),
        catch_up_limit: schedule.catch_up_limit,
        retry_delay_secs: schedule.retry_delay_secs,
        max_failures: schedule.max_failures,
        deferred_until,
        deleted,
    })
}

async fn verify_schedule_privilege_for_stored_view(
    state: &AppState,
    schedule: &ScheduleView,
    request: StoredSchedulePrivilegeRequest<'_>,
) -> Result<(), ApiError> {
    let resolved_targets = if request.target_client_ids.is_empty() {
        Vec::new()
    } else {
        normalized_target_client_ids(request.target_client_ids)?
    };
    let schedule_id = schedule.id.to_string();
    let privilege_intent = stored_schedule_privilege_intent(
        schedule,
        &schedule_id,
        &resolved_targets,
        request.action,
        request.selector_expression,
        request.enabled,
        request.deferred_until,
        request.deleted,
    );
    verify_privilege_intent(state, &privilege_intent, request.assertion).await
}

fn require_valid_schedule_operation(schedule: &ScheduleView) -> Result<&JobCommand, ApiError> {
    if schedule.operation_error.is_some() {
        return Err(ApiError::conflict("schedule_operation_invalid"));
    }
    schedule
        .operation
        .as_ref()
        .ok_or_else(|| ApiError::conflict("schedule_operation_invalid"))
}

fn require_valid_schedule_definition(schedule: &ScheduleView) -> Result<(), ApiError> {
    if schedule.operation_error.is_some() {
        return Err(ApiError::conflict("schedule_operation_invalid"));
    }
    match schedule.trigger_kind {
        ScheduleTriggerKind::Cron if schedule.operation.is_some() => Ok(()),
        ScheduleTriggerKind::Event
            if schedule.operation.is_none() && schedule.event_expression.is_some() =>
        {
            Ok(())
        }
        _ => Err(ApiError::conflict("schedule_operation_invalid")),
    }
}

async fn resolved_schedule_targets(
    state: &AppState,
    target_client_ids: &[String],
) -> Result<Vec<String>, ApiError> {
    let target_client_ids = normalized_target_client_ids_allow_empty(target_client_ids)?;
    if target_client_ids.is_empty() {
        return Ok(target_client_ids);
    }
    let resolved = state
        .repo
        .resolve_bulk_targets(&fixed_target_selection(&target_client_ids)?)
        .await
        .map_err(ApiError::internal_mapper(
            "schedule_targets_resolution_failed",
            "Schedule targets could not be resolved.",
        ))?
        .targets
        .into_iter()
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    let missing = target_client_ids
        .iter()
        .filter(|client_id| !resolved.iter().any(|resolved_id| resolved_id == *client_id))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(ApiError::conflict("schedule_fixed_targets_not_found"));
    }
    Ok(target_client_ids)
}

pub(crate) async fn require_selector_target_snapshot(
    state: &AppState,
    selector_expression: &str,
    target_client_ids: &[String],
    stale_code: &'static str,
) -> Result<(), ApiError> {
    let selector_expression = selector_expression.trim();
    if selector_expression.is_empty() {
        return Ok(());
    }
    let mut resolved = state
        .repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: selector_expression.to_string(),
        })
        .await
        .map_err(ApiError::internal_mapper(
            "schedule_targets_resolution_failed",
            "Schedule targets could not be resolved.",
        ))?
        .targets
        .into_iter()
        .map(|target| target.id)
        .collect::<Vec<_>>();
    resolved.sort();
    resolved.dedup();
    let mut submitted = normalized_target_client_ids_allow_empty(target_client_ids)?;
    submitted.sort();
    if resolved != submitted {
        return Err(ApiError::conflict(stale_code));
    }
    Ok(())
}

struct ScheduleDefinitionRef<'a> {
    name: &'a str,
    operation: Option<&'a vpsman_common::JobCommand>,
    event_argv_template: Option<&'a [String]>,
    selector_expression: &'a str,
    target_client_ids: &'a [String],
    trigger_kind: ScheduleTriggerKind,
    definition_revision: Option<i64>,
    cron_expr: Option<&'a str>,
    timezone: Option<&'a str>,
    event_expression: Option<&'a str>,
    enabled: bool,
    catch_up_policy: Option<&'a str>,
    catch_up_limit: Option<i32>,
    retry_delay_secs: Option<i64>,
    max_failures: i32,
}

impl<'a> ScheduleDefinitionRef<'a> {
    fn from_create(request: &'a CreateScheduleRequest) -> Self {
        Self {
            name: &request.name,
            operation: request.operation.as_ref(),
            event_argv_template: request.event_argv_template.as_deref(),
            selector_expression: &request.selector_expression,
            target_client_ids: &request.target_client_ids,
            trigger_kind: request.trigger_kind,
            definition_revision: None,
            cron_expr: request.cron_expr.as_deref(),
            timezone: request.timezone.as_deref(),
            event_expression: request.event_expression.as_deref(),
            enabled: request.enabled,
            catch_up_policy: request.catch_up_policy.as_deref(),
            catch_up_limit: request.catch_up_limit,
            retry_delay_secs: request.retry_delay_secs,
            max_failures: request.max_failures,
        }
    }

    fn from_update(request: &'a UpdateScheduleRequest) -> Self {
        Self {
            name: &request.name,
            operation: request.operation.as_ref(),
            event_argv_template: request.event_argv_template.as_deref(),
            selector_expression: &request.selector_expression,
            target_client_ids: &request.target_client_ids,
            trigger_kind: request.trigger_kind,
            definition_revision: Some(request.expected_definition_revision),
            cron_expr: request.cron_expr.as_deref(),
            timezone: request.timezone.as_deref(),
            event_expression: request.event_expression.as_deref(),
            enabled: request.enabled,
            catch_up_policy: request.catch_up_policy.as_deref(),
            catch_up_limit: request.catch_up_limit,
            retry_delay_secs: request.retry_delay_secs,
            max_failures: request.max_failures,
        }
    }
}

impl From<UpdateScheduleRequest> for crate::repository_schedules::ScheduleCreateInput {
    fn from(request: UpdateScheduleRequest) -> Self {
        Self {
            name: request.name,
            operation: request.operation,
            event_argv_template: request.event_argv_template,
            selector_expression: request.selector_expression,
            target_client_ids: request.target_client_ids,
            trigger_kind: request.trigger_kind,
            cron_expr: request.cron_expr,
            timezone: request.timezone,
            event_expression: request.event_expression,
            enabled: request.enabled,
            catch_up_policy: request.catch_up_policy,
            catch_up_limit: request.catch_up_limit,
            retry_delay_secs: request.retry_delay_secs,
            max_failures: request.max_failures,
            expected_definition_revision: Some(request.expected_definition_revision),
        }
    }
}

pub(crate) fn require_schedule_snapshot(
    schedule: &ScheduleView,
    expectation: &ScheduleSnapshotExpectation,
) -> Result<(), ApiError> {
    let mut stored_targets = schedule.target_client_ids.clone();
    stored_targets.sort();
    stored_targets.dedup();
    let mut expected_targets = expectation.target_client_ids.clone();
    expected_targets.sort();
    expected_targets.dedup();
    if schedule.selector_expression.trim() != expectation.selector_expression.trim()
        || stored_targets != expected_targets
        || schedule.definition_revision != expectation.definition_revision
    {
        return Err(ApiError::conflict("schedule_snapshot_stale"));
    }
    Ok(())
}

fn same_target_client_ids(left: &[String], right: &[String]) -> bool {
    let mut left = left.to_vec();
    left.sort();
    left.dedup();
    let mut right = right.to_vec();
    right.sort();
    right.dedup();
    left == right
}

pub(crate) fn map_schedule_snapshot_error(error: anyhow::Error) -> ApiError {
    if error.to_string().contains("schedule_snapshot_stale") {
        ApiError::conflict("schedule_snapshot_stale")
    } else {
        ApiError::internal(
            "schedule_mutation_failed",
            "The schedule change could not be completed.",
            error,
        )
    }
}

fn map_schedule_lookup_error(error: anyhow::Error) -> ApiError {
    if error.to_string().contains("schedule_not_found")
        || error
            .downcast_ref::<sqlx::Error>()
            .is_some_and(|error| matches!(error, sqlx::Error::RowNotFound))
    {
        ApiError::not_found("schedule_not_found")
    } else {
        ApiError::internal(
            "schedule_unavailable",
            "The schedule could not be loaded.",
            error,
        )
    }
}
