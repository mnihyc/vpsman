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
    job_request::{job_command_type_label, validate_job_command},
    model::{
        BulkResolveRequest, CreateJobRequest, CreateScheduleRequest, DeferScheduleRequest,
        ListQuery, SchedulePrivilegeMutationRequest, ScheduleView, UpdateScheduleRequest,
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
    Json(request): Json<CreateScheduleRequest>,
) -> Result<(StatusCode, Json<ScheduleView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "schedules:write")
        .await?;
    validate_schedule_request(&request)?;
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
    Json(request): Json<UpdateScheduleRequest>,
) -> Result<Json<ScheduleView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "schedules:write")
        .await?;
    validate_update_schedule_request(&request)?;
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
        selector_expression: schedule.selector_expression.clone(),
        destructive: false,
        confirmed: true,
        command: String::new(),
        argv: Vec::new(),
        operation: Some(schedule.operation.clone()),
        timeout_secs: Some(30),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        idempotency_key: Some(format!(
            "schedule-apply-now:{}:{}",
            schedule.id,
            Uuid::new_v4()
        )),
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
    if request.selector_expression.trim().is_empty() {
        return Err(ApiError::bad_request("schedule_targets_required"));
    }
    parse_selector_expression(request.selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?;
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
    let resolved_targets = resolved_schedule_targets(state, request.selector_expression).await?;
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
    selector_expression: &str,
) -> Result<Vec<String>, ApiError> {
    let resolved_targets = state
        .repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: selector_expression.to_string(),
        })
        .await?
        .targets
        .into_iter()
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    if resolved_targets.is_empty() {
        return Err(ApiError::conflict("schedule_targets_resolved_empty"));
    }
    Ok(resolved_targets)
}

struct ScheduleDefinitionRef<'a> {
    name: &'a str,
    operation: &'a vpsman_common::JobCommand,
    selector_expression: &'a str,
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
