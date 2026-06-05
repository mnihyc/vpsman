use anyhow::anyhow;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use vpsman_common::{encode_json, payload_hash, JobCommand};

use crate::{
    error::ApiError,
    job_request::{validate_file_path, validate_unsigned_command_envelope},
    model::{
        BulkResolveRequest, CreateRestorePlanRequest, HistoryQuery, RestorePlanStatus,
        RestorePlanView,
    },
    state::AppState,
    util::limit_or_default,
};

const MAX_RESTORE_PATHS: usize = 64;
const MAX_RESTORE_NOTE_BYTES: usize = 1024;

pub(crate) async fn list_restore_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<RestorePlanView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .list_restore_plans(limit_or_default(query.limit))
            .await?,
    ))
}

pub(crate) async fn create_restore_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateRestorePlanRequest>,
) -> Result<(StatusCode, Json<RestorePlanView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    validate_create_restore_plan(&request)?;
    ensure_single_restore_target(&state, &request).await?;
    let source_backup = state
        .repo
        .find_backup_request(request.source_backup_request_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("restore_source_backup_not_found"))?;

    let command = restore_command(&request);
    let payload = encode_json(&command)
        .map_err(|error| ApiError::from(anyhow!("failed to encode restore command: {error}")))?;
    let command_hash = payload_hash(&payload);
    let envelope = match request.envelope.as_ref() {
        Some(envelope) => envelope,
        None => {
            state
                .repo
                .record_rejected_restore_plan(
                    &request,
                    Some(&command_hash),
                    &operator,
                    "restore_proof_required",
                )
                .await?;
            return Err(ApiError::forbidden("restore_proof_required"));
        }
    };
    if validate_unsigned_command_envelope(envelope, &request.target_client_id, &command_hash)
        .is_err()
    {
        state
            .repo
            .record_rejected_restore_plan(
                &request,
                Some(&command_hash),
                &operator,
                "invalid_restore_proof_envelope",
            )
            .await?;
        return Err(ApiError::forbidden("invalid_restore_proof_envelope"));
    }

    Ok((
        StatusCode::CREATED,
        Json(
            state
                .repo
                .record_restore_plan(
                    &request,
                    &source_backup,
                    &command_hash,
                    envelope,
                    &operator,
                    RestorePlanStatus::PlannedMetadataOnly,
                )
                .await?,
        ),
    ))
}

pub(crate) fn validate_create_restore_plan(
    request: &CreateRestorePlanRequest,
) -> Result<(), ApiError> {
    if request.target_client_id.trim().is_empty() {
        return Err(ApiError::bad_request("restore_target_client_required"));
    }
    if !request.include_config && request.paths.is_empty() {
        return Err(ApiError::bad_request("restore_scope_required"));
    }
    if request.paths.len() > MAX_RESTORE_PATHS {
        return Err(ApiError::bad_request("restore_path_limit_exceeded"));
    }
    for path in &request.paths {
        validate_file_path(path)?;
    }
    if let Some(destination_root) = &request.destination_root {
        validate_file_path(destination_root)?;
    }
    if request
        .note
        .as_ref()
        .is_some_and(|note| note.len() > MAX_RESTORE_NOTE_BYTES)
    {
        return Err(ApiError::bad_request("restore_note_too_long"));
    }
    if !request.confirmed {
        return Err(ApiError::conflict("restore_confirmation_required"));
    }
    Ok(())
}

async fn ensure_single_restore_target(
    state: &AppState,
    request: &CreateRestorePlanRequest,
) -> Result<(), ApiError> {
    let resolved = state
        .repo
        .resolve_bulk_targets(&BulkResolveRequest {
            clients: vec![request.target_client_id.clone()],
            tags: Vec::new(),
            tag_mode: None,
            destructive: false,
            confirmed: true,
        })
        .await?;
    if resolved.target_count == 1 && resolved.targets[0].id == request.target_client_id {
        Ok(())
    } else {
        Err(ApiError::bad_request("restore_target_client_not_found"))
    }
}

fn restore_command(request: &CreateRestorePlanRequest) -> JobCommand {
    JobCommand::Restore {
        source_backup_request_id: request.source_backup_request_id,
        paths: request.paths.clone(),
        include_config: request.include_config,
        destination_root: request.destination_root.clone(),
        archive_path: None,
        archive_base64: None,
        archive_size_bytes: None,
        archive_sha256_hex: None,
        dry_run: false,
        post_restore_argv: Vec::new(),
    }
}
