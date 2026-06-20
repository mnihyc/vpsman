use anyhow::anyhow;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header::CONTENT_TYPE, HeaderMap, HeaderValue, Response, StatusCode},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use vpsman_common::{encode_json, payload_hash, JobCommand, PrivilegeAssertion};

use crate::{
    backup_auto_artifacts::backup_artifact_object_key,
    backup_handoff::{backup_artifact_streaming_max_bytes, stage_retained_backup_artifact_stdout},
    backup_upload_sessions::backup_upload_sessions,
    error::ApiError,
    job_request::{
        fixed_target_selection, job_command_type_label, normalized_target_client_ids,
        validate_file_path,
    },
    model::{
        BackupArtifactHandoffRequest, BackupArtifactHandoffView, BackupArtifactUploadChunkRequest,
        BackupArtifactUploadCommitRequest, BackupArtifactUploadSessionCreateRequest,
        BackupArtifactUploadSessionView, BackupArtifactView, BackupPolicyPruneRequest,
        BackupPolicyPruneResponse, BackupPolicyView, BackupRequestStatus, BackupRequestView,
        BulkResolveRequest, CreateBackupPolicyRequest, CreateBackupRequest, CreateScheduleRequest,
        ListQuery, RecordBackupArtifactMetadataRequest, UploadBackupArtifactRequest, WsEvent,
    },
    privilege::{
        verify_privilege_intent, JobPrivilegeIntent, JobPrivilegeIntentInput,
        SchedulePrivilegeIntent, SchedulePrivilegeIntentInput,
    },
    routes_file_transfers::{map_verified_object_error, streaming_artifact_file_body},
    routes_schedules::validate_schedule_request,
    security::{operator_has_scope, SCOPE_BACKUPS_READ},
    selector_expression::id_selector_expression,
    state::AppState,
    unix_now,
};

const MAX_BACKUP_PATHS: usize = 64;
const MAX_BACKUP_NOTE_BYTES: usize = 1024;
const MAX_BACKUP_ARTIFACT_OBJECT_KEY_BYTES: usize = 1024;
const MAX_BACKUP_ARTIFACT_SIZE_BYTES: i64 = 1_099_511_627_776;
pub(crate) const MAX_BACKUP_ARTIFACT_UPLOAD_BODY_BYTES: usize = 24 * 1024 * 1024;
pub(crate) const MAX_BACKUP_ARTIFACT_UPLOAD_BYTES: usize = 16 * 1024 * 1024;

pub(crate) async fn list_backup_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<BackupRequestView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_BACKUPS_READ)
        .await?;
    Ok(Json(state.repo.query_backup_requests(&query).await?))
}

pub(crate) async fn list_backup_artifacts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<BackupArtifactView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_BACKUPS_READ)
        .await?;
    Ok(Json(state.repo.query_backup_artifacts(&query).await?))
}

pub(crate) async fn list_backup_policies(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<BackupPolicyView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_BACKUPS_READ)
        .await?;
    Ok(Json(state.repo.list_backup_policies().await?))
}

pub(crate) async fn create_backup_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<CreateBackupPolicyRequest>,
) -> Result<(StatusCode, Json<BackupPolicyView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    if !operator_has_scope(&operator.operator.scopes, "schedules:write") {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    validate_create_backup_policy_request(&request)?;
    request.target_client_ids = normalized_target_client_ids(&request.target_client_ids)?;
    verify_backup_policy_privilege(&state, &request, request.privilege_assertion.clone()).await?;
    Ok((
        StatusCode::CREATED,
        Json(state.repo.create_backup_policy(request, &operator).await?),
    ))
}

async fn verify_backup_policy_privilege(
    state: &AppState,
    request: &CreateBackupPolicyRequest,
    assertion: Option<PrivilegeAssertion>,
) -> Result<(), ApiError> {
    let resolved_targets =
        resolved_backup_policy_targets(state, &request.target_client_ids).await?;
    let operation = backup_policy_command(request);
    let operation_payload = encode_json(&operation).map_err(|error| {
        ApiError::from(anyhow!("failed to encode backup policy command: {error}"))
    })?;
    let operation_payload_hash = payload_hash(&operation_payload);
    let command_type = job_command_type_label(&operation);
    let privilege_intent = SchedulePrivilegeIntent::new(SchedulePrivilegeIntentInput {
        action: "backup_policy.create",
        schedule_id: None,
        name: &request.name,
        command_type,
        operation_payload_hash: &operation_payload_hash,
        selector_expression: &request.selector_expression,
        resolved_targets: &resolved_targets,
        cron_expr: &request.cron_expr,
        timezone: &request.timezone,
        enabled: request.enabled,
        catch_up_policy: &request.catch_up_policy,
        catch_up_limit: request.catch_up_limit,
        retry_delay_secs: request.retry_delay_secs,
        max_failures: request.max_failures,
        deferred_until: None,
        deleted: false,
    });
    verify_privilege_intent(state, &privilege_intent, assertion).await
}

async fn resolved_backup_policy_targets(
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
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(ApiError::conflict("backup_policy_fixed_targets_not_found"));
    }
    Ok(target_client_ids)
}

pub(crate) async fn prune_backup_policies(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BackupPolicyPruneRequest>,
) -> Result<Json<BackupPolicyPruneResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    if !request.dry_run && !request.confirmed {
        return Err(ApiError::bad_request(
            "backup_policy_prune_confirmation_required",
        ));
    }
    if !request.dry_run
        && request
            .preview_hash
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err(ApiError::bad_request(
            "backup_policy_prune_preview_hash_required",
        ));
    }
    let metadata_only = request.metadata_only.unwrap_or(false);
    if !request.dry_run && !metadata_only && state.backup_object_store.is_none() {
        return Err(ApiError::bad_request(
            "backup_policy_prune_object_store_required",
        ));
    }
    let preview_outputs = collect_backup_policy_prune_outputs(&state, &request, false).await?;
    let preview_hash = backup_policy_prune_preview_hash(
        request.schedule_id,
        request.metadata_only,
        &preview_outputs,
    )?;
    if request.dry_run {
        return Ok(Json(BackupPolicyPruneResponse {
            dry_run: true,
            metadata_only_requested: request.metadata_only,
            preview_hash,
            policies: preview_outputs,
        }));
    }
    if request
        .preview_hash
        .as_deref()
        .is_some_and(|submitted| submitted.trim() != preview_hash)
    {
        return Err(ApiError::conflict(
            "backup_policy_prune_preview_hash_mismatch",
        ));
    }
    let outputs = collect_backup_policy_prune_outputs(&state, &request, true).await?;
    state
        .repo
        .record_backup_policy_prune_audit(
            &operator,
            request.dry_run,
            request.metadata_only,
            &outputs,
        )
        .await?;
    Ok(Json(BackupPolicyPruneResponse {
        dry_run: false,
        metadata_only_requested: request.metadata_only,
        preview_hash,
        policies: outputs,
    }))
}

async fn collect_backup_policy_prune_outputs(
    state: &AppState,
    request: &BackupPolicyPruneRequest,
    execute: bool,
) -> Result<Vec<crate::model::BackupPolicyPrunePolicyView>, ApiError> {
    let metadata_only = request.metadata_only.unwrap_or(false);
    if execute && !metadata_only && state.backup_object_store.is_none() {
        return Err(ApiError::bad_request(
            "backup_policy_prune_object_store_required",
        ));
    }
    let mut policies = state.repo.list_backup_policies().await?;
    if let Some(schedule_id) = request.schedule_id {
        policies.retain(|policy| policy.schedule_id == schedule_id);
    }
    if policies.is_empty() {
        return Err(ApiError::bad_request("backup_policy_not_found"));
    }
    let mut outputs = Vec::new();
    for policy in policies {
        let cutoff_unix = unix_now().saturating_sub(policy.retention_days.max(1) as u64 * 86_400);
        let candidates = state
            .repo
            .list_backup_policy_prune_candidates(&policy, cutoff_unix)
            .await?;
        let matched_rows = candidates.len() as i64;
        let mut pruned_rows = 0_i64;
        let mut object_keys = if !execute || metadata_only {
            candidates
                .iter()
                .map(|candidate| candidate.object_key.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let mut object_delete_attempted = false;
        let mut object_delete_errors = Vec::new();
        if execute {
            if metadata_only {
                pruned_rows = state
                    .repo
                    .prune_backup_policy_candidates_metadata(&candidates)
                    .await?;
            } else if !candidates.is_empty() {
                object_delete_attempted = true;
                if let Some(store) = state.backup_object_store.as_ref() {
                    for candidate in &candidates {
                        let rows = state
                            .repo
                            .prune_backup_policy_candidate_metadata(candidate)
                            .await?;
                        pruned_rows += rows;
                        if rows == 0 {
                            continue;
                        }
                        object_keys.push(candidate.object_key.clone());
                        match store.delete_confirmed(&candidate.object_key).await {
                            Ok(()) => {}
                            Err(error) => {
                                object_delete_errors
                                    .push(format!("{}: {error}", candidate.object_key));
                                break;
                            }
                        }
                    }
                }
            }
        }
        let status = if !execute {
            "dry_run"
        } else if !object_delete_errors.is_empty() {
            "partial_error"
        } else if pruned_rows == 0 {
            "no_matches"
        } else {
            "pruned"
        };
        let output = state.repo.backup_policy_prune_view(
            &policy,
            cutoff_unix,
            matched_rows,
            pruned_rows,
            object_keys,
            object_delete_attempted,
            object_delete_errors,
            metadata_only,
            status,
        );
        outputs.push(output);
    }
    Ok(outputs)
}

fn backup_policy_prune_preview_hash(
    schedule_id: Option<uuid::Uuid>,
    metadata_only: Option<bool>,
    outputs: &[crate::model::BackupPolicyPrunePolicyView],
) -> Result<String, ApiError> {
    let payload = serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "schedule_id": schedule_id,
        "metadata_only_requested": metadata_only,
        "policies": outputs.iter().map(|policy| {
            serde_json::json!({
                "schedule_id": policy.schedule_id,
                "retention_days": policy.retention_days,
                "keep_last": policy.keep_last,
                "cutoff_unix": policy.cutoff_unix,
                "matched_rows": policy.matched_rows,
                "object_keys": policy.object_keys,
                "metadata_only": policy.metadata_only,
                "status": policy.status,
            })
        }).collect::<Vec<_>>(),
    }))
    .map_err(|error| ApiError::from(anyhow!("backup_policy_prune_preview_hash_failed: {error}")))?;
    Ok(payload_hash(&payload))
}

pub(crate) async fn create_backup_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateBackupRequest>,
) -> Result<(StatusCode, Json<BackupRequestView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    validate_create_backup_request(&request)?;
    ensure_single_backup_client(&state, &request).await?;

    let command = backup_command(&request);
    let payload = encode_json(&command)
        .map_err(|error| ApiError::from(anyhow!("failed to encode backup command: {error}")))?;
    let command_hash = payload_hash(&payload);
    let resolved_targets = vec![request.client_id.clone()];
    let selector_expression = id_selector_expression(&request.client_id);
    let privilege_intent = JobPrivilegeIntent::new(JobPrivilegeIntentInput {
        selector_expression: &selector_expression,
        command_type: "backup",
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
            .record_rejected_backup_request(
                &request,
                &command_hash,
                &operator,
                "backup_privilege_verification_failed",
            )
            .await?;
        return Err(error);
    }
    let command_scope = format!("client:{}", request.client_id);

    Ok((
        StatusCode::CREATED,
        Json(
            state
                .repo
                .record_backup_request(
                    &request,
                    &command_hash,
                    &command_scope,
                    &operator,
                    BackupRequestStatus::RequestedMetadataOnly,
                )
                .await?,
        ),
    ))
}

pub(crate) async fn record_backup_artifact_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(backup_request_id): Path<uuid::Uuid>,
    Json(request): Json<RecordBackupArtifactMetadataRequest>,
) -> Result<(StatusCode, Json<BackupArtifactView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    validate_backup_artifact_metadata_request(&request)?;
    let backup_request = state
        .repo
        .find_backup_request(backup_request_id)
        .await?
        .ok_or_else(|| ApiError::not_found("backup_request_not_found"))?;
    if backup_request.artifact_id.is_some() {
        return Err(ApiError::conflict("backup_artifact_already_recorded"));
    }

    let artifact = state
        .repo
        .record_backup_artifact_metadata(&backup_request, &request, &operator)
        .await
        .map_err(|error| {
            if error
                .to_string()
                .contains("backup_artifact_already_recorded")
            {
                ApiError::conflict("backup_artifact_already_recorded")
            } else {
                ApiError::from(error)
            }
        })?;
    state.publish(WsEvent::BackupArtifactRecorded {
        backup_request_id,
        client_id: backup_request.client_id,
        artifact_id: artifact.id,
    });
    Ok((StatusCode::CREATED, Json(artifact)))
}

pub(crate) async fn upload_backup_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(backup_request_id): Path<uuid::Uuid>,
    Json(request): Json<UploadBackupArtifactRequest>,
) -> Result<(StatusCode, Json<BackupArtifactView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    validate_backup_artifact_upload_request(&request)?;
    let store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::conflict("backup_object_store_not_configured"))?;
    let backup_request = state
        .repo
        .find_backup_request(backup_request_id)
        .await?
        .ok_or_else(|| ApiError::not_found("backup_request_not_found"))?;
    if backup_request.artifact_id.is_some() {
        return Err(ApiError::conflict("backup_artifact_already_recorded"));
    }

    let artifact_bytes = BASE64
        .decode(request.artifact_base64.trim())
        .map_err(|_| ApiError::bad_request("backup_artifact_base64_invalid"))?;
    validate_encrypted_backup_artifact(&artifact_bytes, &backup_request.client_id)?;
    let sha256_hex = payload_hash(&artifact_bytes);
    let size_bytes = i64::try_from(artifact_bytes.len())
        .map_err(|_| ApiError::bad_request("backup_artifact_size_invalid"))?;

    store
        .put_new(&request.object_key, &artifact_bytes)
        .await
        .map_err(|error| {
            let error_text = error.to_string();
            if error_text.contains("object already exists") || error_text.contains("File exists") {
                ApiError::conflict("backup_artifact_object_exists")
            } else {
                ApiError::from(error)
            }
        })?;
    let metadata_request = RecordBackupArtifactMetadataRequest {
        object_key: request.object_key.clone(),
        sha256_hex,
        encrypted: true,
        size_bytes,
        confirmed: request.confirmed,
    };
    match state
        .repo
        .record_backup_artifact_metadata(&backup_request, &metadata_request, &operator)
        .await
    {
        Ok(artifact) => {
            state.publish(WsEvent::BackupArtifactRecorded {
                backup_request_id,
                client_id: backup_request.client_id,
                artifact_id: artifact.id,
            });
            Ok((StatusCode::CREATED, Json(artifact)))
        }
        Err(error) => {
            store.delete_best_effort(&request.object_key).await;
            if error
                .to_string()
                .contains("backup_artifact_already_recorded")
            {
                Err(ApiError::conflict("backup_artifact_already_recorded"))
            } else {
                Err(ApiError::from(error))
            }
        }
    }
}

pub(crate) async fn create_backup_artifact_upload_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(backup_request_id): Path<uuid::Uuid>,
    Json(request): Json<BackupArtifactUploadSessionCreateRequest>,
) -> Result<(StatusCode, Json<BackupArtifactUploadSessionView>), ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    let _store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::conflict("backup_object_store_not_configured"))?;
    let backup_request = state
        .repo
        .find_backup_request(backup_request_id)
        .await?
        .ok_or_else(|| ApiError::not_found("backup_request_not_found"))?;
    if backup_request.artifact_id.is_some() {
        return Err(ApiError::conflict("backup_artifact_already_recorded"));
    }

    Ok((
        StatusCode::CREATED,
        Json(
            backup_upload_sessions()
                .create(backup_request_id, backup_request.client_id, request)
                .await?,
        ),
    ))
}

pub(crate) async fn upload_backup_artifact_session_chunk(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((backup_request_id, upload_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Json(request): Json<BackupArtifactUploadChunkRequest>,
) -> Result<Json<BackupArtifactUploadSessionView>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    Ok(Json(
        backup_upload_sessions()
            .write_chunk(backup_request_id, upload_id, request)
            .await?,
    ))
}

pub(crate) async fn commit_backup_artifact_upload_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((backup_request_id, upload_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Json(request): Json<BackupArtifactUploadCommitRequest>,
) -> Result<(StatusCode, Json<BackupArtifactView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "backup_artifact_upload_commit_confirmation_required",
        ));
    }
    let store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::conflict("backup_object_store_not_configured"))?;
    let backup_request = state
        .repo
        .find_backup_request(backup_request_id)
        .await?
        .ok_or_else(|| ApiError::not_found("backup_request_not_found"))?;
    if backup_request.artifact_id.is_some() {
        return Err(ApiError::conflict("backup_artifact_already_recorded"));
    }

    let prepared = backup_upload_sessions()
        .prepare_commit(
            backup_request_id,
            upload_id,
            &backup_request.client_id,
            request,
        )
        .await?;
    if state
        .repo
        .backup_artifact_object_key_exists(&prepared.object_key)
        .await?
    {
        return Err(ApiError::conflict("backup_artifact_object_exists"));
    }
    store
        .put_file_idempotent(
            &prepared.object_key,
            &prepared.staging_path,
            &prepared.sha256_hex,
            prepared
                .size_bytes
                .try_into()
                .map_err(|_| ApiError::bad_request("backup_artifact_size_invalid"))?,
        )
        .await
        .map_err(|error| {
            let error_text = error.to_string();
            if error_text.contains("object already exists") || error_text.contains("File exists") {
                ApiError::conflict("backup_artifact_object_exists")
            } else {
                ApiError::from(error)
            }
        })?;
    let metadata_request = RecordBackupArtifactMetadataRequest {
        object_key: prepared.object_key.clone(),
        sha256_hex: prepared.sha256_hex.clone(),
        encrypted: true,
        size_bytes: prepared.size_bytes,
        confirmed: true,
    };
    match state
        .repo
        .record_backup_artifact_metadata(&backup_request, &metadata_request, &operator)
        .await
    {
        Ok(artifact) => {
            backup_upload_sessions().finish(prepared.upload_id).await;
            state.publish(WsEvent::BackupArtifactRecorded {
                backup_request_id,
                client_id: backup_request.client_id,
                artifact_id: artifact.id,
            });
            Ok((StatusCode::CREATED, Json(artifact)))
        }
        Err(error) => {
            if error
                .to_string()
                .contains("backup_artifact_already_recorded")
            {
                Err(ApiError::conflict("backup_artifact_already_recorded"))
            } else {
                Err(ApiError::from(error))
            }
        }
    }
}

pub(crate) async fn abort_backup_artifact_upload_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((backup_request_id, upload_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Json(request): Json<BackupArtifactUploadCommitRequest>,
) -> Result<Json<BackupArtifactUploadSessionView>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    Ok(Json(
        backup_upload_sessions()
            .abort(backup_request_id, upload_id, request.confirmed)
            .await?,
    ))
}

pub(crate) async fn create_backup_artifact_handoff(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(backup_request_id): Path<uuid::Uuid>,
    Json(request): Json<BackupArtifactHandoffRequest>,
) -> Result<(StatusCode, Json<BackupArtifactHandoffView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "backup_artifact_handoff_confirmation_required",
        ));
    }
    let store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::conflict("backup_object_store_not_configured"))?;
    let backup_request = state
        .repo
        .find_backup_request(backup_request_id)
        .await?
        .ok_or_else(|| ApiError::not_found("backup_request_not_found"))?;
    if backup_request.artifact_id.is_some() {
        return Err(ApiError::conflict("backup_artifact_already_recorded"));
    }
    let candidate = state
        .repo
        .find_backup_artifact_output_candidate(&backup_request, request.job_id)
        .await?
        .ok_or_else(|| ApiError::conflict("backup_artifact_handoff_source_missing"))?;
    let prepared = stage_retained_backup_artifact_stdout(&state, &candidate.outputs).await?;
    let artifact_bytes = match tokio::fs::read(&prepared.staging_path).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let _ = tokio::fs::remove_file(&prepared.staging_path).await;
            return Err(ApiError::conflict(
                "backup_artifact_handoff_staging_read_failed",
            ));
        }
    };
    if let Err(error) = validate_encrypted_backup_artifact_with_limit(
        &artifact_bytes,
        &backup_request.client_id,
        backup_artifact_streaming_max_bytes(),
    ) {
        let _ = tokio::fs::remove_file(&prepared.staging_path).await;
        return Err(error);
    }
    let object_key = backup_artifact_object_key(&backup_request.client_id, backup_request.id);
    if state
        .repo
        .backup_artifact_object_key_exists(&object_key)
        .await?
    {
        let _ = tokio::fs::remove_file(&prepared.staging_path).await;
        return Err(ApiError::conflict("backup_artifact_object_exists"));
    }
    if let Err(error) = store
        .put_file_idempotent(
            &object_key,
            &prepared.staging_path,
            &prepared.sha256_hex,
            prepared
                .size_bytes
                .try_into()
                .map_err(|_| ApiError::bad_request("backup_artifact_size_invalid"))?,
        )
        .await
        .map_err(ApiError::from)
    {
        let _ = tokio::fs::remove_file(&prepared.staging_path).await;
        return Err(error);
    }
    let metadata_request = RecordBackupArtifactMetadataRequest {
        object_key,
        sha256_hex: prepared.sha256_hex.clone(),
        encrypted: true,
        size_bytes: prepared.size_bytes,
        confirmed: true,
    };
    let result = match state
        .repo
        .record_backup_artifact_metadata(&backup_request, &metadata_request, &operator)
        .await
    {
        Ok(artifact) => {
            state.publish(WsEvent::BackupArtifactRecorded {
                backup_request_id,
                client_id: backup_request.client_id,
                artifact_id: artifact.id,
            });
            Ok((
                StatusCode::CREATED,
                Json(BackupArtifactHandoffView {
                    artifact,
                    source_job_id: candidate.job_id,
                    source_chunk_count: prepared.source_chunk_count,
                    source: "retained_job_outputs_streamed".to_string(),
                }),
            ))
        }
        Err(error) => {
            store.delete_best_effort(&metadata_request.object_key).await;
            if error
                .to_string()
                .contains("backup_artifact_already_recorded")
            {
                Err(ApiError::conflict("backup_artifact_already_recorded"))
            } else {
                Err(ApiError::from(error))
            }
        }
    };
    let _ = tokio::fs::remove_file(&prepared.staging_path).await;
    result
}

pub(crate) async fn download_backup_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(backup_request_id): Path<uuid::Uuid>,
) -> Result<Response<Body>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_BACKUPS_READ)
        .await?;
    let store = state
        .backup_object_store
        .as_ref()
        .ok_or_else(|| ApiError::conflict("backup_object_store_not_configured"))?;
    let backup_request = state
        .repo
        .find_backup_request(backup_request_id)
        .await?
        .ok_or_else(|| ApiError::not_found("backup_request_not_found"))?;
    let artifact_id = backup_request
        .artifact_id
        .ok_or_else(|| ApiError::conflict("backup_artifact_not_recorded"))?;
    let artifact = state
        .repo
        .find_backup_artifact(artifact_id)
        .await?
        .ok_or_else(|| ApiError::not_found("backup_artifact_not_found"))?;
    if artifact.client_id != backup_request.client_id {
        return Err(ApiError::conflict("backup_artifact_client_mismatch"));
    }
    let expected_size = u64::try_from(artifact.size_bytes)
        .map_err(|_| ApiError::conflict("backup_artifact_object_size_mismatch"))?;
    let object_file = store
        .verified_object_file(
            &artifact.object_key,
            &artifact.sha256_hex,
            expected_size,
            state.artifact_max_bytes(),
        )
        .await
        .map_err(|error| {
            map_verified_object_error(
                error,
                "backup_artifact_object_not_found",
                "backup_artifact_object_hash_mismatch",
            )
        })?;
    let body = streaming_artifact_file_body(
        object_file.path,
        "backup_artifact_object_not_found",
        object_file.cleanup_after_stream,
    )
    .await?;

    let mut response = Response::new(body);
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        "x-vpsman-backup-artifact-id",
        HeaderValue::from_str(&artifact.id.to_string())
            .map_err(|error| ApiError::from(anyhow!("invalid artifact id header: {error}")))?,
    );
    response.headers_mut().insert(
        "x-vpsman-backup-artifact-sha256",
        HeaderValue::from_str(&artifact.sha256_hex)
            .map_err(|error| ApiError::from(anyhow!("invalid artifact sha header: {error}")))?,
    );
    response.headers_mut().insert(
        "content-length",
        HeaderValue::from_str(&expected_size.to_string())
            .map_err(|error| ApiError::from(anyhow!("invalid artifact size header: {error}")))?,
    );
    Ok(response)
}

pub(crate) fn validate_create_backup_request(
    request: &CreateBackupRequest,
) -> Result<(), ApiError> {
    if request.client_id.trim().is_empty() {
        return Err(ApiError::bad_request("backup_client_required"));
    }
    if !request.include_config && request.paths.is_empty() {
        return Err(ApiError::bad_request("backup_scope_required"));
    }
    if request.paths.len() > MAX_BACKUP_PATHS {
        return Err(ApiError::bad_request("backup_path_limit_exceeded"));
    }
    for path in &request.paths {
        validate_file_path(path)?;
    }
    if let Some(recipient_public_key_hex) = &request.recipient_public_key_hex {
        validate_backup_recipient_public_key_hex(recipient_public_key_hex)?;
    }
    if request
        .note
        .as_ref()
        .is_some_and(|note| note.len() > MAX_BACKUP_NOTE_BYTES)
    {
        return Err(ApiError::bad_request("backup_note_too_long"));
    }
    if !request.confirmed {
        return Err(ApiError::conflict("backup_confirmation_required"));
    }
    Ok(())
}

pub(crate) fn validate_create_backup_policy_request(
    request: &CreateBackupPolicyRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict("backup_policy_confirmation_required"));
    }
    let schedule_request = CreateScheduleRequest {
        name: request.name.clone(),
        operation: JobCommand::Backup {
            paths: request.paths.clone(),
            include_config: request.include_config,
            recipient_public_key_hex: request
                .recipient_public_key_hex
                .clone()
                .map(|value| value.to_ascii_lowercase()),
        },
        selector_expression: request.selector_expression.clone(),
        target_client_ids: request.target_client_ids.clone(),
        cron_expr: request.cron_expr.clone(),
        timezone: request.timezone.clone(),
        enabled: request.enabled,
        catch_up_policy: request.catch_up_policy.clone(),
        catch_up_limit: request.catch_up_limit,
        retry_delay_secs: request.retry_delay_secs,
        max_failures: request.max_failures,
        privilege_assertion: None,
        confirmed: true,
    };
    validate_schedule_request(&schedule_request)?;
    let retention_days = request.retention_days.unwrap_or(30);
    if !(1..=3650).contains(&retention_days) {
        return Err(ApiError::bad_request(
            "backup_policy_retention_days_out_of_range",
        ));
    }
    let keep_last = request.keep_last.unwrap_or(7);
    if !(1..=1000).contains(&keep_last) {
        return Err(ApiError::bad_request(
            "backup_policy_keep_last_out_of_range",
        ));
    }
    if let Some(rotation_generation) = &request.rotation_generation {
        validate_backup_policy_generation(rotation_generation)?;
    }
    Ok(())
}

pub(crate) fn validate_backup_artifact_metadata_request(
    request: &RecordBackupArtifactMetadataRequest,
) -> Result<(), ApiError> {
    validate_backup_artifact_object_key(&request.object_key)?;
    if !is_sha256_hex(&request.sha256_hex) {
        return Err(ApiError::bad_request("backup_artifact_invalid_sha256"));
    }
    if !request.encrypted {
        return Err(ApiError::bad_request("backup_artifact_must_be_encrypted"));
    }
    if !(1..=MAX_BACKUP_ARTIFACT_SIZE_BYTES).contains(&request.size_bytes) {
        return Err(ApiError::bad_request("backup_artifact_size_invalid"));
    }
    if !request.confirmed {
        return Err(ApiError::conflict("backup_artifact_confirmation_required"));
    }
    Ok(())
}

pub(crate) fn validate_backup_artifact_upload_request(
    request: &UploadBackupArtifactRequest,
) -> Result<(), ApiError> {
    validate_backup_artifact_object_key(&request.object_key)?;
    if request.artifact_base64.trim().is_empty() {
        return Err(ApiError::bad_request("backup_artifact_body_required"));
    }
    let max_base64_len = MAX_BACKUP_ARTIFACT_UPLOAD_BYTES.div_ceil(3) * 4 + 256;
    if request.artifact_base64.len() > max_base64_len {
        return Err(ApiError::bad_request("backup_artifact_upload_too_large"));
    }
    if !request.confirmed {
        return Err(ApiError::conflict(
            "backup_artifact_upload_confirmation_required",
        ));
    }
    Ok(())
}

async fn ensure_single_backup_client(
    state: &AppState,
    request: &CreateBackupRequest,
) -> Result<(), ApiError> {
    let resolved = state
        .repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: id_selector_expression(&request.client_id),
        })
        .await?;
    if resolved.target_count == 1 && resolved.targets[0].id == request.client_id {
        Ok(())
    } else {
        Err(ApiError::bad_request("backup_client_not_found"))
    }
}

fn backup_command(request: &CreateBackupRequest) -> JobCommand {
    JobCommand::Backup {
        paths: request.paths.clone(),
        include_config: request.include_config,
        recipient_public_key_hex: request
            .recipient_public_key_hex
            .clone()
            .map(|value| value.to_ascii_lowercase()),
    }
}

fn backup_policy_command(request: &CreateBackupPolicyRequest) -> JobCommand {
    JobCommand::Backup {
        paths: request.paths.clone(),
        include_config: request.include_config,
        recipient_public_key_hex: request
            .recipient_public_key_hex
            .clone()
            .map(|value| value.to_ascii_lowercase()),
    }
}

fn validate_backup_recipient_public_key_hex(value: &str) -> Result<(), ApiError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "backup_recipient_public_key_hex_invalid",
        ))
    }
}

fn validate_backup_policy_generation(value: &str) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 120 {
        return Err(ApiError::bad_request(
            "backup_policy_rotation_generation_invalid",
        ));
    }
    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
    }) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "backup_policy_rotation_generation_invalid",
        ))
    }
}

pub(crate) fn validate_backup_artifact_object_key(object_key: &str) -> Result<(), ApiError> {
    if object_key.trim().is_empty() {
        return Err(ApiError::bad_request("backup_artifact_object_key_required"));
    }
    if object_key.len() > MAX_BACKUP_ARTIFACT_OBJECT_KEY_BYTES || object_key.as_bytes().contains(&0)
    {
        return Err(ApiError::bad_request("backup_artifact_object_key_invalid"));
    }
    if object_key.starts_with('/')
        || object_key.contains('\\')
        || object_key
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(ApiError::bad_request("backup_artifact_object_key_invalid"));
    }
    Ok(())
}

pub(crate) fn validate_encrypted_backup_artifact(
    bytes: &[u8],
    expected_client_id: &str,
) -> Result<(), ApiError> {
    validate_encrypted_backup_artifact_with_limit(
        bytes,
        expected_client_id,
        MAX_BACKUP_ARTIFACT_UPLOAD_BYTES,
    )
}

pub(crate) fn validate_encrypted_backup_artifact_with_limit(
    bytes: &[u8],
    expected_client_id: &str,
    max_size_bytes: usize,
) -> Result<(), ApiError> {
    if bytes.is_empty() || bytes.len() > max_size_bytes {
        return Err(ApiError::bad_request("backup_artifact_size_invalid"));
    }
    let artifact: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| ApiError::bad_request("backup_artifact_json_invalid"))?;
    if artifact.get("format").and_then(|value| value.as_str()) != Some("vpsman.backup_artifact.v1")
    {
        return Err(ApiError::bad_request("backup_artifact_format_invalid"));
    }
    if artifact.get("version").and_then(|value| value.as_u64()) != Some(1) {
        return Err(ApiError::bad_request("backup_artifact_version_invalid"));
    }
    if artifact.get("client_id").and_then(|value| value.as_str()) != Some(expected_client_id) {
        return Err(ApiError::bad_request("backup_artifact_client_mismatch"));
    }
    if artifact.get("cipher").and_then(|value| value.as_str()) != Some("x25519-chacha20poly1305") {
        return Err(ApiError::bad_request("backup_artifact_cipher_invalid"));
    }
    if artifact.get("compression").and_then(|value| value.as_str()) != Some("lz4-size-prepended") {
        return Err(ApiError::bad_request("backup_artifact_compression_invalid"));
    }
    let ciphertext_base64 = artifact
        .get("ciphertext_base64")
        .and_then(|value| value.as_str())
        .ok_or_else(|| ApiError::bad_request("backup_artifact_ciphertext_required"))?;
    let ciphertext = BASE64
        .decode(ciphertext_base64)
        .map_err(|_| ApiError::bad_request("backup_artifact_ciphertext_invalid"))?;
    if ciphertext.is_empty() {
        return Err(ApiError::bad_request("backup_artifact_ciphertext_required"));
    }
    let ciphertext_sha256_hex = artifact
        .get("ciphertext_sha256_hex")
        .and_then(|value| value.as_str())
        .ok_or_else(|| ApiError::bad_request("backup_artifact_ciphertext_sha256_required"))?;
    if ciphertext_sha256_hex != payload_hash(&ciphertext) {
        return Err(ApiError::bad_request(
            "backup_artifact_ciphertext_sha256_mismatch",
        ));
    }
    validate_hex_field(
        &artifact,
        "recipient_public_key_sha256_hex",
        64,
        "backup_artifact_recipient_public_key_sha256_hex_required",
        "backup_artifact_recipient_public_key_sha256_hex_invalid",
    )?;
    validate_hex_field(
        &artifact,
        "ephemeral_public_key_hex",
        64,
        "backup_artifact_ephemeral_public_key_hex_required",
        "backup_artifact_ephemeral_public_key_hex_invalid",
    )?;
    validate_hex_field(
        &artifact,
        "nonce_hex",
        24,
        "backup_artifact_nonce_hex_required",
        "backup_artifact_nonce_hex_invalid",
    )?;
    Ok(())
}

fn validate_hex_field(
    artifact: &serde_json::Value,
    field: &'static str,
    expected_len: usize,
    required_code: &'static str,
    invalid_code: &'static str,
) -> Result<(), ApiError> {
    let value = artifact
        .get(field)
        .and_then(|value| value.as_str())
        .ok_or_else(|| ApiError::bad_request(required_code))?;
    if value.len() != expected_len || !value.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return Err(ApiError::bad_request(invalid_code));
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}
