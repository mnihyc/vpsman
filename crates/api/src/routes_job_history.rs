use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Response},
    Json,
};
use uuid::Uuid;

use crate::{
    error::ApiError,
    model::{
        AuditLogView, AuthProofRotationHistoryView, HistoryQuery, JobHistoryView, JobOutputView,
        JobTargetView, ListQuery, NetworkObservationTrendView, NetworkObservationView,
        ProcessSupervisorInventoryView,
    },
    model_command_templates::JobOutputComparisonView,
    state::AppState,
    util::limit_or_default,
};

pub(crate) async fn list_jobs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<JobHistoryView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.query_jobs(&query).await?))
}

pub(crate) async fn list_auth_proof_rotations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<AuthProofRotationHistoryView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .list_auth_proof_rotation_history(limit_or_default(query.limit))
            .await?,
    ))
}

pub(crate) async fn get_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<JobHistoryView>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let job = state
        .repo
        .get_job(job_id)
        .await?
        .ok_or_else(|| ApiError::not_found("job_not_found"))?;
    Ok(Json(job))
}

pub(crate) async fn list_job_targets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Vec<JobTargetView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.list_job_targets(job_id).await?))
}

pub(crate) async fn list_job_outputs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Vec<JobOutputView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.list_job_outputs(job_id).await?))
}

pub(crate) async fn compare_job_outputs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Vec<JobOutputComparisonView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.compare_job_outputs(job_id).await?))
}

pub(crate) async fn download_job_output_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((job_id, client_id, seq)): Path<(Uuid, String, i32)>,
) -> Result<Response<Body>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::not_found("job_output_artifact_not_available"))?;
    let artifact = state
        .repo
        .get_job_output_artifact_ref(job_id, &client_id, seq)
        .await?
        .ok_or_else(|| ApiError::not_found("job_output_artifact_not_found"))?;
    let bytes = store.get(&artifact.object_key).await?;
    let actual_hash = vpsman_common::payload_hash(&bytes);
    if actual_hash != artifact.sha256_hex || bytes.len() as i64 != artifact.size_bytes {
        return Err(ApiError::conflict("job_output_artifact_integrity_mismatch"));
    }
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"job-output-{job_id}-{seq}.bin\""
        ))
        .map_err(|_| ApiError::conflict("job_output_artifact_filename_invalid"))?,
    );
    response.headers_mut().insert(
        "x-vpsman-artifact-sha256",
        HeaderValue::from_str(&artifact.sha256_hex)
            .map_err(|_| ApiError::conflict("job_output_artifact_hash_invalid"))?,
    );
    Ok(response)
}

pub(crate) async fn list_process_supervisor_inventory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<ProcessSupervisorInventoryView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .list_process_supervisor_inventory(limit_or_default(query.limit))
            .await?,
    ))
}

pub(crate) async fn list_audit_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<AuditLogView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.query_audit_logs(&query).await?))
}

pub(crate) async fn list_network_observations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<NetworkObservationView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .list_network_observations(limit_or_default(query.limit))
            .await?,
    ))
}

pub(crate) async fn list_network_observation_trends(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<NetworkObservationTrendView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .list_network_observation_trends(limit_or_default(query.limit))
            .await?,
    ))
}
