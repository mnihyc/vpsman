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
    job_request::{
        fixed_target_selection, job_command_type_label, normalized_target_client_ids,
        normalized_target_client_ids_allow_empty, validate_job_command,
    },
    model::{
        BulkResolveRequest, CreateJobRequest, CreateScheduleRequest, DeferScheduleRequest,
        ListQuery, SchedulePrivilegeMutationRequest, ScheduleView, UpdateScheduleRequest,
        UpdateScheduleTargetsRequest,
    },
    privilege::{verify_privilege_intent, SchedulePrivilegeIntent, SchedulePrivilegeIntentInput},
    repository_schedules::next_cron_runs,
    repository_schedules::ScheduleSnapshotExpectation,
    routes_jobs::create_job_from_saved_schedule,
    security::{operator_has_scope, SCOPE_SCHEDULES_READ},
    selector_expression::parse_selector_expression,
    state::AppState,
    util::limit_or_default,
};
use vpsman_common::{encode_json, payload_hash, JobCommand, PrivilegeAssertion};

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
    Ok(Json(state.repo.query_schedules(&query).await.map_err(
        ApiError::internal_mapper("schedules_unavailable", "Schedules could not be loaded."),
    )?))
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
    validate_schedule_request(&request)?;
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

pub(crate) async fn update_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<Uuid>,
    Json(mut request): Json<UpdateScheduleRequest>,
) -> Result<Json<ScheduleView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "schedules:write")
        .await?;
    validate_update_schedule_request(&request)?;
    require_schedule_confirmed(request.confirmed)?;
    request.target_client_ids =
        normalized_target_client_ids_allow_empty(&request.target_client_ids)?;
    request.expected_target_client_ids =
        normalized_target_client_ids_allow_empty(&request.expected_target_client_ids)?;
    let expectation = ScheduleSnapshotExpectation {
        selector_expression: request.expected_selector_expression.clone(),
        target_client_ids: request.expected_target_client_ids.clone(),
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
    require_valid_schedule_operation(&schedule)?;
    let selector_expression = schedule.selector_expression.trim().to_string();
    if selector_expression.is_empty() {
        return Err(ApiError::conflict("schedule_selector_missing"));
    }
    parse_selector_expression(&selector_expression)
        .map_err(|_| ApiError::conflict("schedule_selector_invalid"))?;
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
    };
    let mut current_target_client_ids = schedule.target_client_ids.clone();
    current_target_client_ids.sort();
    current_target_client_ids.dedup();
    if current_target_client_ids == target_client_ids {
        return Err(ApiError::conflict("schedule_targets_already_current"));
    }
    verify_schedule_privilege_for_stored_view(
        &state,
        "schedule.targets.update",
        &schedule,
        &selector_expression,
        &target_client_ids,
        schedule.enabled,
        schedule.deferred_until.as_deref(),
        false,
        request.privilege_assertion.clone(),
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
    require_valid_schedule_operation(&schedule)?;
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
                &operator,
            )
            .await
            .map_err(ApiError::internal_mapper(
                "schedule_defer_failed",
                "The schedule could not be deferred.",
            ))?,
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
        .soft_delete_schedule(schedule_id, &operator)
        .await
        .map_err(ApiError::internal_mapper(
            "schedule_delete_failed",
            "The schedule could not be deleted.",
        ))?;
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
    if enabled {
        require_valid_schedule_operation(&schedule)?;
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
            .set_schedule_enabled(schedule_id, enabled, &operator)
            .await
            .map_err(ApiError::internal_mapper(
                "schedule_enabled_state_update_failed",
                "The schedule enabled state could not be changed.",
            ))?,
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
    if request.timezone != "UTC" {
        return Err(ApiError::bad_request("schedule_timezone_must_be_utc"));
    }
    if request.cron_expr.split_whitespace().count() != 5 {
        return Err(ApiError::bad_request("schedule_cron_must_be_5_field"));
    }
    if next_cron_runs(request.cron_expr, 1).is_err() {
        return Err(ApiError::bad_request("schedule_cron_invalid"));
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
    if !matches!(
        request.catch_up_policy,
        "skip_missed" | "run_once" | "run_all_limited"
    ) {
        return Err(ApiError::bad_request("schedule_catch_up_policy_invalid"));
    }
    if !(1..=25).contains(&request.catch_up_limit) {
        return Err(ApiError::bad_request(
            "schedule_catch_up_limit_out_of_range",
        ));
    }
    if !(1..=86_400).contains(&request.retry_delay_secs) {
        return Err(ApiError::bad_request("schedule_retry_delay_out_of_range"));
    }
    if !(1..=100).contains(&request.max_failures) {
        return Err(ApiError::bad_request("schedule_max_failures_out_of_range"));
    }
    validate_job_command(request.operation)?;
    validate_schedulable_job_command(request.operation)
}

fn validate_schedulable_job_command(command: &JobCommand) -> Result<(), ApiError> {
    if matches!(command, JobCommand::RuntimeConfigSync { .. }) {
        return Err(ApiError::bad_request(
            "runtime_config_sync_is_server_issued",
        ));
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
    let operation_payload = encode_json(request.operation).map_err(|error| {
        ApiError::internal(
            "schedule_privilege_intent_failed",
            "The schedule privilege request could not be prepared.",
            anyhow::Error::from(error),
        )
    })?;
    let operation_payload_hash = payload_hash(&operation_payload);
    let command_type = job_command_type_label(request.operation);
    let schedule_id = schedule_id.map(|id| id.to_string());
    let privilege_intent = SchedulePrivilegeIntent::new(SchedulePrivilegeIntentInput {
        action,
        schedule_id: schedule_id.as_deref(),
        name: request.name,
        command_type,
        operation_payload_hash: &operation_payload_hash,
        selector_expression: request.selector_expression,
        resolved_targets: &resolved_targets,
        cron_expr: request.cron_expr,
        timezone: request.timezone,
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
        action,
        schedule,
        &schedule.selector_expression,
        &schedule.target_client_ids,
        enabled,
        deferred_until,
        deleted,
        assertion,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn verify_schedule_privilege_for_stored_view(
    state: &AppState,
    action: &str,
    schedule: &ScheduleView,
    selector_expression: &str,
    target_client_ids: &[String],
    enabled: bool,
    deferred_until: Option<&str>,
    deleted: bool,
    assertion: Option<PrivilegeAssertion>,
) -> Result<(), ApiError> {
    let resolved_targets = if target_client_ids.is_empty() {
        Vec::new()
    } else {
        normalized_target_client_ids(target_client_ids)?
    };
    let schedule_id = schedule.id.to_string();
    let privilege_intent = SchedulePrivilegeIntent::new(SchedulePrivilegeIntentInput {
        action,
        schedule_id: Some(&schedule_id),
        name: &schedule.name,
        command_type: &schedule.command_type,
        operation_payload_hash: &schedule.operation_payload_hash,
        selector_expression,
        resolved_targets: &resolved_targets,
        cron_expr: &schedule.cron_expr,
        timezone: &schedule.timezone,
        enabled,
        catch_up_policy: &schedule.catch_up_policy,
        catch_up_limit: schedule.catch_up_limit,
        retry_delay_secs: schedule.retry_delay_secs,
        max_failures: schedule.max_failures,
        deferred_until,
        deleted,
    });
    verify_privilege_intent(state, &privilege_intent, assertion).await
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
    operation: &'a vpsman_common::JobCommand,
    selector_expression: &'a str,
    target_client_ids: &'a [String],
    cron_expr: &'a str,
    timezone: &'a str,
    enabled: bool,
    catch_up_policy: &'a str,
    catch_up_limit: i32,
    retry_delay_secs: i64,
    max_failures: i32,
}

impl<'a> ScheduleDefinitionRef<'a> {
    fn from_create(request: &'a CreateScheduleRequest) -> Self {
        Self {
            name: &request.name,
            operation: &request.operation,
            selector_expression: &request.selector_expression,
            target_client_ids: &request.target_client_ids,
            cron_expr: &request.cron_expr,
            timezone: &request.timezone,
            enabled: request.enabled,
            catch_up_policy: &request.catch_up_policy,
            catch_up_limit: request.catch_up_limit,
            retry_delay_secs: request.retry_delay_secs,
            max_failures: request.max_failures,
        }
    }

    fn from_update(request: &'a UpdateScheduleRequest) -> Self {
        Self {
            name: &request.name,
            operation: &request.operation,
            selector_expression: &request.selector_expression,
            target_client_ids: &request.target_client_ids,
            cron_expr: &request.cron_expr,
            timezone: &request.timezone,
            enabled: request.enabled,
            catch_up_policy: &request.catch_up_policy,
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
            selector_expression: request.selector_expression,
            target_client_ids: request.target_client_ids,
            cron_expr: request.cron_expr,
            timezone: request.timezone,
            enabled: request.enabled,
            catch_up_policy: request.catch_up_policy,
            catch_up_limit: request.catch_up_limit,
            retry_delay_secs: request.retry_delay_secs,
            max_failures: request.max_failures,
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
