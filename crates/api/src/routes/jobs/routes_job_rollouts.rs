use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    error::ApiError,
    model::{JobRolloutView, UpdateJobRolloutRequest},
    security::SCOPE_JOBS_READ,
    state::AppState,
};

#[derive(Debug, Deserialize)]
pub(crate) struct JobRolloutListQuery {
    pub(crate) limit: Option<i64>,
}

pub(crate) async fn list_job_rollouts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<JobRolloutListQuery>,
) -> Result<Json<Vec<JobRolloutView>>, ApiError> {
    state
        .require_operator_role_and_scope(&headers, "viewer", SCOPE_JOBS_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .list_job_rollouts(query.limit.unwrap_or(100))
            .await
            .map_err(ApiError::internal_mapper(
                "job_rollouts_unavailable",
                "Job rollouts could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn get_job_rollout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<JobRolloutView>, ApiError> {
    state
        .require_operator_role_and_scope(&headers, "viewer", SCOPE_JOBS_READ)
        .await?;
    state
        .repo
        .get_job_rollout(job_id)
        .await
        .map_err(ApiError::internal_mapper(
            "job_rollout_unavailable",
            "The job rollout could not be loaded.",
        ))?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("job_rollout_not_found"))
}

pub(crate) async fn pause_job_rollout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Json(request): Json<UpdateJobRolloutRequest>,
) -> Result<Json<JobRolloutView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    let rollout = state
        .repo
        .pause_job_rollout(job_id, &operator, request.reason.as_deref())
        .await
        .map_err(map_rollout_error)?;
    Ok(Json(rollout))
}

pub(crate) async fn resume_job_rollout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Json(request): Json<UpdateJobRolloutRequest>,
) -> Result<Json<JobRolloutView>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    require_rollout_resume_confirmation(request.confirmed)?;
    let rollout = state
        .repo
        .resume_job_rollout(job_id, &operator, request.reason.as_deref())
        .await
        .map_err(map_rollout_error)?;
    crate::job_dispatcher::wake_job_dispatcher();
    Ok(Json(rollout))
}

pub(crate) fn require_rollout_resume_confirmation(confirmed: bool) -> Result<(), ApiError> {
    if !confirmed {
        return Err(ApiError::conflict(
            "job_rollout_resume_requires_confirmation",
        ));
    }
    Ok(())
}

fn map_rollout_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("job_rollout_not_found") {
        return ApiError::not_found("job_rollout_not_found");
    }
    if message.contains("job_rollout_terminal") {
        return ApiError::conflict("job_rollout_terminal");
    }
    ApiError::internal(
        "job_rollout_mutation_failed",
        "The job rollout change could not be completed.",
        error,
    )
}
