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
        validate_job_command,
    },
    model::{
        CreateJobRequest, CreateScheduleRequest, DeferScheduleRequest, ListQuery,
        SchedulePrivilegeMutationRequest, ScheduleView, UpdateScheduleRequest,
        UpdateScheduleTargetsRequest,
    },
    privilege::{verify_privilege_intent, SchedulePrivilegeIntent, SchedulePrivilegeIntentInput},
    repository_schedules::next_cron_runs,
    routes_jobs::create_job_from_saved_schedule,
    selector_expression::parse_selector_expression,
    state::AppState,
};
use vpsman_common::{encode_json, payload_hash, PrivilegeAssertion};

pub(crate) async fn list_schedules(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<ScheduleView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.query_schedules(&query).await?))
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
    request.target_client_ids = normalized_target_client_ids(&request.target_client_ids)?;
    verify_schedule_privilege_for_definition(
        &state,
        "schedule.create",
        None,
        ScheduleDefinitionRef::from_create(&request),
        None,
        false,
        request.privilege_assertion.clone(),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(state.repo.create_schedule(request, &operator).await?),
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
    request.target_client_ids = normalized_target_client_ids(&request.target_client_ids)?;
    verify_schedule_privilege_for_definition(
        &state,
        "schedule.update",
        Some(schedule_id),
        ScheduleDefinitionRef::from_update(&request),
        None,
        false,
        request.privilege_assertion.clone(),
    )
    .await?;
    Ok(Json(
        state
            .repo
            .update_schedule_record(schedule_id, request.into(), &operator)
            .await?,
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
    let target_client_ids = normalized_target_client_ids(&request.target_client_ids)?;
    let selector_expression = request.selector_expression.trim().to_string();
    if !selector_expression.is_empty() {
        parse_selector_expression(&selector_expression)
            .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
    }
    let schedule = state.repo.schedule_by_id(schedule_id).await?;
    verify_schedule_privilege_for_definition(
        &state,
        "schedule.targets.update",
        Some(schedule_id),
        ScheduleDefinitionRef {
            name: &schedule.name,
            operation: &schedule.operation,
            selector_expression: &selector_expression,
            target_client_ids: &target_client_ids,
            cron_expr: &schedule.cron_expr,
            timezone: &schedule.timezone,
            enabled: schedule.enabled,
            catch_up_policy: &schedule.catch_up_policy,
            catch_up_limit: schedule.catch_up_limit,
            retry_delay_secs: schedule.retry_delay_secs,
            max_failures: schedule.max_failures,
        },
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
                selector_expression,
                target_client_ids,
                &operator,
            )
            .await?,
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
    let schedule = state.repo.schedule_by_id(schedule_id).await?;
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
            .await?,
    ))
}

pub(crate) async fn apply_schedule_now(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(schedule_id): Path<Uuid>,
) -> Result<(StatusCode, Json<crate::model::CreateJobResponse>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    let schedule = state.repo.schedule_by_id(schedule_id).await?;
    if !schedule.enabled {
        return Err(ApiError::conflict("schedule_apply_now_requires_enabled"));
    }
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: schedule.selector_expression.clone(),
        target_client_ids: schedule.target_client_ids.clone(),
        destructive: false,
        confirmed: true,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(schedule.operation.clone()),
        timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        reconnect_policy: None,
    };
    create_job_from_saved_schedule(&state, &operator, request, schedule_id).await
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
    let schedule = state.repo.schedule_by_id(schedule_id).await?;
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
        .await?;
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
    let schedule = state.repo.schedule_by_id(schedule_id).await?;
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
            .await?,
    ))
}

pub(crate) fn validate_schedule_request(request: &CreateScheduleRequest) -> Result<(), ApiError> {
    validate_schedule_definition(ScheduleDefinitionRef::from_create(request))
}

pub(crate) fn validate_update_schedule_request(
    request: &UpdateScheduleRequest,
) -> Result<(), ApiError> {
    validate_schedule_definition(ScheduleDefinitionRef::from_update(request))
}

fn validate_schedule_definition(request: ScheduleDefinitionRef<'_>) -> Result<(), ApiError> {
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
    normalized_target_client_ids(request.target_client_ids)?;
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
    validate_job_command(request.operation)
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
) -> Result<(), ApiError> {
    let resolved_targets = resolved_schedule_targets(state, request.target_client_ids).await?;
    let operation_payload = encode_json(request.operation)
        .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
    let operation_payload_hash = payload_hash(&operation_payload);
    let command_type = job_command_type_label(request.operation);
    let privilege_intent = SchedulePrivilegeIntent::new(SchedulePrivilegeIntentInput {
        action,
        schedule_id,
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
    verify_schedule_privilege_for_definition(
        state,
        action,
        Some(schedule.id),
        ScheduleDefinitionRef::from_view(schedule, enabled),
        deferred_until,
        deleted,
        assertion,
    )
    .await
}

async fn resolved_schedule_targets(
    state: &AppState,
    target_client_ids: &[String],
) -> Result<Vec<String>, ApiError> {
    let target_client_ids = normalized_target_client_ids(target_client_ids)?;
    let resolved = state
        .repo
        .resolve_bulk_targets(&fixed_target_selection(&target_client_ids)?)
        .await?
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

    fn from_view(schedule: &'a ScheduleView, enabled: bool) -> Self {
        Self {
            name: &schedule.name,
            operation: &schedule.operation,
            selector_expression: &schedule.selector_expression,
            target_client_ids: &schedule.target_client_ids,
            cron_expr: &schedule.cron_expr,
            timezone: &schedule.timezone,
            enabled,
            catch_up_policy: &schedule.catch_up_policy,
            catch_up_limit: schedule.catch_up_limit,
            retry_delay_secs: schedule.retry_delay_secs,
            max_failures: schedule.max_failures,
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
