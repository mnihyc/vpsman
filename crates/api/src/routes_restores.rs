use anyhow::anyhow;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use vpsman_common::{encode_json, payload_hash, CommandEnvelope, JobCommand};

use crate::{
    error::ApiError,
    job_request::validate_file_path,
    model::{
        BulkResolveRequest, CreateRestorePlanRequest, ListQuery, RestorePlanStatus, RestorePlanView,
    },
    privilege::{verify_privilege_intent, JobPrivilegeIntent, JobPrivilegeIntentInput},
    selector_expression::id_selector_expression,
    state::AppState,
    unix_now,
};

const MAX_RESTORE_PATHS: usize = 64;
const MAX_RESTORE_NOTE_BYTES: usize = 1024;

pub(crate) async fn list_restore_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<RestorePlanView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.query_restore_plans(&query).await?))
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
    let resolved_targets = vec![request.target_client_id.clone()];
    let selector_expression = id_selector_expression(&request.target_client_id);
    let privilege_intent = JobPrivilegeIntent::new(JobPrivilegeIntentInput {
        selector_expression: &selector_expression,
        command_type: "restore",
        operation_payload_hash: &command_hash,
        resolved_targets: &resolved_targets,
        timeout_secs: 30,
        force_unprivileged: false,
        privileged: true,
    });
    if let Err(error) = verify_privilege_intent(
        &state,
        &privilege_intent,
        request.privilege_assertion.clone(),
    )
    .await
    {
        state
            .repo
            .record_rejected_restore_plan(
                &request,
                Some(&command_hash),
                &operator,
                "restore_privilege_verification_failed",
            )
            .await?;
        return Err(error);
    }
    let envelope = metadata_command_envelope(&request.target_client_id, &command_hash);

    Ok((
        StatusCode::CREATED,
        Json(
            state
                .repo
                .record_restore_plan(
                    &request,
                    &source_backup,
                    &command_hash,
                    &envelope,
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
        if path_contains_dot_segment(path) {
            return Err(ApiError::bad_request("restore_path_invalid"));
        }
        validate_file_path(path)?;
    }
    if let Some(destination_root) = &request.destination_root {
        if path_contains_dot_segment(destination_root) {
            return Err(ApiError::bad_request("restore_destination_root_invalid"));
        }
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

fn path_contains_dot_segment(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "." || segment == "..")
}

async fn ensure_single_restore_target(
    state: &AppState,
    request: &CreateRestorePlanRequest,
) -> Result<(), ApiError> {
    let resolved = state
        .repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: id_selector_expression(&request.target_client_id),
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

fn metadata_command_envelope(client_id: &str, command_hash: &str) -> CommandEnvelope {
    let now = unix_now();
    CommandEnvelope {
        command_id: uuid::Uuid::new_v4(),
        scope: format!("client:{client_id}"),
        payload_hash_hex: command_hash.to_string(),
        signed_unix: now,
        expires_unix: now.saturating_add(300),
        server_signature: Vec::new(),
    }
}
