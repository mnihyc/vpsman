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
    security::{operator_has_scope, SCOPE_JOBS_READ},
    state::AppState,
    util::limit_or_default,
};

#[derive(Debug, Deserialize)]
pub(crate) struct ServerJobListQuery {
    pub(crate) limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CancelServerJobRequest {
    #[serde(default)]
    pub(crate) confirmed: bool,
}

pub(crate) async fn preview_artifact_cleanup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ArtifactCleanupPreviewRequest>,
) -> Result<Json<ArtifactCleanupPreviewView>, ApiError> {
    let operator = state.require_operator_role(&headers, "operator").await?;
    let domains = normalize_artifact_cleanup_domains(&request.domains)?;
    ensure_artifact_cleanup_domain_authority(&operator.operator.scopes, &domains)?;
    Ok(Json(
        state
            .repo
            .preview_artifact_cleanup(&request.expression, &domains)
            .await
            .map_err(artifact_cleanup_error)?,
    ))
}

pub(crate) async fn create_artifact_cleanup_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ArtifactCleanupCreateRequest>,
) -> Result<(StatusCode, Json<ServerJobView>), ApiError> {
    let operator = state.require_operator_role(&headers, "operator").await?;
    let domains = normalize_artifact_cleanup_domains(&request.domains)?;
    ensure_artifact_cleanup_domain_authority(&operator.operator.scopes, &domains)?;
    if !request.confirmed {
        return Err(ApiError::conflict("artifact_cleanup_confirmation_required"));
    }
    let job = state
        .repo
        .create_artifact_cleanup_job(
            &request.expression,
            &domains,
            &request.preview_hash,
            &operator,
        )
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
            .await
            .map_err(ApiError::internal_mapper(
                "server_jobs_unavailable",
                "Server jobs could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn cancel_server_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
    Json(request): Json<CancelServerJobRequest>,
) -> Result<Json<ServerJobView>, ApiError> {
    state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "server_job_cancel_requires_confirmation",
        ));
    }
    let job = state
        .repo
        .cancel_server_job(job_id)
        .await
        .map_err(ApiError::internal_mapper(
            "server_job_cancel_failed",
            "The server job could not be canceled.",
        ))?
        .ok_or_else(|| ApiError::not_found("server_job_not_found_or_not_queued"))?;
    Ok(Json(job))
}

fn artifact_cleanup_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("artifact_cleanup_preview_hash_mismatch") {
        return ApiError::conflict("artifact_cleanup_preview_hash_mismatch");
    }
    if message.contains("artifact_cleanup_match_limit_exceeded") {
        return ApiError::bad_request_with_message(
            "artifact_cleanup_review_scope_too_large",
            "This cleanup matches too many artifacts for one reviewed action. Narrow the domains or selector, then review it again.",
        );
    }
    if message.contains("artifact_cleanup_review_numeric_invalid") {
        return ApiError::conflict_with_message(
            "artifact_cleanup_review_data_invalid",
            "Cleanup review stopped because matched artifact size metadata is invalid. Repair the artifact metadata or narrow the selector before reviewing again.",
        );
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
    ApiError::internal(
        "artifact_cleanup_failed",
        "The artifact cleanup could not be completed.",
        error,
    )
}

fn normalize_artifact_cleanup_domains(raw: &[String]) -> Result<Vec<String>, ApiError> {
    if raw.is_empty() {
        return Err(ApiError::bad_request("artifact_cleanup_domains_required"));
    }
    let mut selected = Vec::new();
    for value in raw {
        match value.trim() {
            "job_output" | "file_transfer" | "backup_artifact" => {
                selected.push(value.trim().to_string())
            }
            _ => return Err(ApiError::bad_request("artifact_cleanup_domain_invalid")),
        }
    }
    let mut normalized = Vec::new();
    for domain in ["job_output", "file_transfer", "backup_artifact"] {
        if selected.iter().any(|selected| selected == domain) {
            normalized.push(domain.to_string());
        }
    }
    if normalized.is_empty() {
        return Err(ApiError::bad_request("artifact_cleanup_domains_required"));
    }
    Ok(normalized)
}

fn ensure_artifact_cleanup_domain_authority(
    scopes: &[String],
    domains: &[String],
) -> Result<(), ApiError> {
    for domain in domains {
        if !operator_has_scope(scopes, artifact_cleanup_required_scope(domain)) {
            return Err(ApiError::forbidden("operator_scope_insufficient"));
        }
    }
    Ok(())
}

fn artifact_cleanup_required_scope(domain: &str) -> &'static str {
    match domain {
        "backup_artifact" => "backups:write",
        "job_output" | "file_transfer" => "jobs:write",
        _ => "jobs:write",
    }
}
