use axum::{
    extract::{Query, State},
    http::HeaderMap,
    http::StatusCode,
    Json,
};

use crate::{
    error::ApiError,
    job_request::validate_job_command,
    model::{CreateScheduleRequest, ListQuery, ScheduleView},
    state::AppState,
};

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
    Ok((
        StatusCode::CREATED,
        Json(state.repo.create_schedule(request, &operator).await?),
    ))
}

pub(crate) fn validate_schedule_request(request: &CreateScheduleRequest) -> Result<(), ApiError> {
    if request.name.trim().is_empty() {
        return Err(ApiError::bad_request("schedule_name_required"));
    }
    if request.name.len() > 120 {
        return Err(ApiError::bad_request("schedule_name_too_long"));
    }
    if request.interval_secs == 0 || request.interval_secs > 31_536_000 {
        return Err(ApiError::bad_request("schedule_interval_out_of_range"));
    }
    if request.clients.is_empty() && request.tags.is_empty() {
        return Err(ApiError::bad_request("schedule_targets_required"));
    }
    if !matches!(
        request.catch_up_policy.as_str(),
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
    validate_job_command(&request.operation)
}
