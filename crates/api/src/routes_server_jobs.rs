use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    error::ApiError,
    model::{
        ArtifactCleanupCreateRequest, ArtifactCleanupPreviewRequest, ArtifactCleanupPreviewView,
        ServerJobView,
    },
    security::SCOPE_JOBS_READ,
    state::AppState,
    util::limit_or_default,
};

#[derive(Debug, Deserialize)]
pub(crate) struct ServerJobListQuery {
    pub(crate) limit: Option<i64>,
}

pub(crate) async fn preview_artifact_cleanup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ArtifactCleanupPreviewRequest>,
) -> Result<Json<ArtifactCleanupPreviewView>, ApiError> {
    state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    Ok(Json(
        state
            .repo
            .preview_artifact_cleanup(&request.expression)
            .await
            .map_err(artifact_cleanup_error)?,
    ))
}

pub(crate) async fn create_artifact_cleanup_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ArtifactCleanupCreateRequest>,
) -> Result<(StatusCode, Json<ServerJobView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict("artifact_cleanup_confirmation_required"));
    }
    let job = state
        .repo
        .create_artifact_cleanup_job(&request.expression, &request.preview_hash, &operator)
        .await
        .map_err(artifact_cleanup_error)?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

pub(crate) async fn list_server_jobs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ServerJobListQuery>,
) -> Result<Json<Vec<ServerJobView>>, ApiError> {
    state
        .require_operator_role_and_scope(&headers, "operator", SCOPE_JOBS_READ)
        .await?;
    Ok(Json(
        state
            .repo
            .list_server_jobs(limit_or_default(query.limit))
            .await?,
    ))
}

pub(crate) async fn cancel_server_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<ServerJobView>, ApiError> {
    state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    let job = state
        .repo
        .cancel_server_job(job_id)
        .await?
        .ok_or_else(|| ApiError::not_found("server_job_not_found_or_not_queued"))?;
    Ok(Json(job))
}

fn artifact_cleanup_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("artifact_cleanup_preview_hash_mismatch") {
        return ApiError::conflict("artifact_cleanup_preview_hash_mismatch");
    }
    if message.contains("artifact_cleanup_expression_required") {
        return ApiError::bad_request("artifact_cleanup_expression_required");
    }
    if message.contains("artifact_cleanup_expression_invalid") {
        return ApiError::bad_request("artifact_cleanup_expression_invalid");
    }
    if message.contains("expression") || message.contains("unexpected") {
        return ApiError::bad_request("artifact_cleanup_expression_invalid");
    }
    ApiError::from(error)
}
