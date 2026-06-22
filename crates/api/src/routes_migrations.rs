use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;
use vpsman_common::{encode_json, payload_hash, JobCommand, DEFAULT_MAX_JOB_TIMEOUT_SECS};
use vpsman_server_core::JOB_STATUS_RUNNING;

use crate::{
    error::ApiError,
    model::{
        CreateJobResponse, CreateMigrationLinkRequest, CreateMigrationRunRequest,
        CreateMigrationRunResponse, ListQuery, MigrationLinkStatus, MigrationLinkView,
        RestorePlanStatus,
    },
    privilege::{
        verify_privilege_intent, DbPrivilegeIntent, JobPrivilegeIntent, JobPrivilegeIntentInput,
    },
    routes_jobs::{
        create_job_target_counts, effective_job_max_timeout_secs, request_fingerprint_for_job,
        target_capabilities_from_agents, validate_restore_archive_binding,
    },
    security::{operator_has_scope, SCOPE_BACKUPS_READ},
    state::AppState,
};

const MAX_MIGRATION_NOTE_BYTES: usize = 1024;

pub(crate) async fn list_migration_links(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<MigrationLinkView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_BACKUPS_READ)
        .await?;
    Ok(Json(state.repo.query_migration_links(&query).await?))
}

pub(crate) async fn create_migration_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateMigrationLinkRequest>,
) -> Result<(StatusCode, Json<MigrationLinkView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    validate_create_migration_link(&request)?;
    let restore_plan = state
        .repo
        .find_restore_plan(request.restore_plan_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("migration_restore_plan_not_found"))?;
    if restore_plan.status != RestorePlanStatus::PlannedMetadataOnly.as_str() {
        return Err(ApiError::conflict(
            "migration_restore_plan_not_metadata_only",
        ));
    }
    verify_migration_link_privilege(&state, &request, &restore_plan).await?;
    Ok((
        StatusCode::CREATED,
        Json(record_migration_link_or_conflict(&state, &request, &restore_plan, &operator).await?),
    ))
}

pub(crate) async fn create_migration_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<CreateMigrationRunRequest>,
) -> Result<(StatusCode, Json<CreateMigrationRunResponse>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "backups:write")
        .await?;
    if !operator_has_scope(&operator.operator.scopes, "jobs:write") {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    validate_create_migration_link(&request.link)?;
    let restore_plan = state
        .repo
        .find_restore_plan(request.link.restore_plan_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("migration_restore_plan_not_found"))?;
    if restore_plan.status != RestorePlanStatus::PlannedMetadataOnly.as_str() {
        return Err(ApiError::conflict(
            "migration_restore_plan_not_metadata_only",
        ));
    }
    ensure_migration_run_job_matches_plan(&request, &restore_plan)?;
    verify_migration_link_privilege(&state, &request.link, &restore_plan).await?;
    if state
        .repo
        .migration_link_exists_for_restore_plan(request.link.restore_plan_id)
        .await?
    {
        return Err(ApiError::conflict("migration_link_already_exists"));
    }
    let plan = preflight_migration_restore_job(&state, &mut request).await?;
    let migration_link = state
        .repo
        .record_migration_run_restore_job(
            &request.link,
            &restore_plan,
            &operator,
            plan.job_id,
            &request.job,
            &plan.command_hash,
            &plan.request_fingerprint,
            &plan.resolved_targets,
        )
        .await
        .map_err(|error| {
            if error.to_string().contains("migration_link_already_exists") {
                ApiError::conflict("migration_link_already_exists")
            } else if error
                .to_string()
                .contains("job_id_reused_with_different_request")
            {
                ApiError::conflict("job_id_reused_with_different_request")
            } else {
                ApiError::from(error)
            }
        })?;
    crate::job_dispatcher::wake_job_dispatcher(state.clone());
    let restore_job = migration_restore_job_response(&state, &plan).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateMigrationRunResponse {
            migration_link,
            restore_job,
        }),
    ))
}

struct MigrationRestoreJobPlan {
    job_id: Uuid,
    command_hash: String,
    request_fingerprint: String,
    resolved_targets: Vec<String>,
    max_timeout_secs: u64,
}

async fn record_migration_link_or_conflict(
    state: &AppState,
    request: &CreateMigrationLinkRequest,
    restore_plan: &MigrationLinkRestorePlan,
    operator: &crate::model::AuthContext,
) -> Result<MigrationLinkView, ApiError> {
    state
        .repo
        .record_migration_link(
            request,
            restore_plan,
            operator,
            MigrationLinkStatus::LinkedMetadataOnly,
        )
        .await
        .map_err(|error| {
            if error.to_string().contains("migration_link_already_exists") {
                ApiError::conflict("migration_link_already_exists")
            } else {
                ApiError::from(error)
            }
        })
}

async fn preflight_migration_restore_job(
    state: &AppState,
    request: &mut CreateMigrationRunRequest,
) -> Result<MigrationRestoreJobPlan, ApiError> {
    if !request.job.privileged {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    let job_id = request
        .job
        .job_id
        .ok_or_else(|| ApiError::conflict("job_id_required"))?;
    if job_id == Uuid::nil() {
        return Err(ApiError::bad_request("job_id_invalid"));
    }
    let effective_max_timeout_secs =
        effective_job_max_timeout_secs(request.job.max_timeout_secs, state.max_job_timeout_secs())?;
    request.job.max_timeout_secs = Some(effective_max_timeout_secs);
    let selection = request.job.target_selection()?;
    let resolved_agents = state.repo.resolve_bulk_targets(&selection).await?.targets;
    let targets = request.job.fixed_target_ids()?;
    if targets
        .iter()
        .any(|target| !resolved_agents.iter().any(|agent| agent.id == *target))
    {
        return Err(ApiError::conflict("fixed_target_not_found"));
    }
    let command = request.job.job_command()?;
    validate_restore_archive_binding(state, &command, &targets).await?;
    let command_payload = encode_json(&command).map_err(|error| {
        ApiError::from(anyhow::anyhow!(
            "failed to encode migration restore job command: {error}"
        ))
    })?;
    let command_hash = payload_hash(&command_payload);
    let request_fingerprint =
        request_fingerprint_for_job(&request.job, &command_hash, &targets, None)?;
    let privilege_intent = JobPrivilegeIntent::new(JobPrivilegeIntentInput {
        selector_expression: &request.job.selector_expression,
        command_type: request.job.command_type_label(),
        operation_payload_hash: &command_hash,
        resolved_targets: &targets,
        max_timeout_secs: request
            .job
            .max_timeout_secs
            .unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS),
        force_unprivileged: request.job.force_unprivileged,
        privileged: request.job.privileged,
    });
    verify_privilege_intent(
        state,
        &privilege_intent,
        request.job.privilege_assertion.clone(),
    )
    .await?;
    let target_capabilities = target_capabilities_from_agents(&resolved_agents);
    let (dispatch_targets, capability_skips) = vpsman_server_core::split_targets_by_capability(
        &command,
        &targets,
        &target_capabilities,
        request.job.force_unprivileged,
    );
    if !capability_skips.is_empty() || dispatch_targets.len() != targets.len() {
        return Err(ApiError::conflict(
            "migration_restore_target_capability_missing",
        ));
    }
    if !state.gateway.configured() {
        return Err(ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "gateway_control_url_missing",
            error: anyhow::anyhow!("gateway_control_url_missing"),
        });
    }
    Ok(MigrationRestoreJobPlan {
        job_id,
        command_hash,
        request_fingerprint,
        resolved_targets: targets,
        max_timeout_secs: effective_max_timeout_secs,
    })
}

async fn migration_restore_job_response(
    state: &AppState,
    plan: &MigrationRestoreJobPlan,
) -> Result<CreateJobResponse, ApiError> {
    let target_counts = create_job_target_counts(state, plan.job_id).await?;
    Ok(CreateJobResponse {
        job_id: plan.job_id,
        target_count: plan.resolved_targets.len(),
        status: JOB_STATUS_RUNNING.to_string(),
        max_timeout_secs: plan.max_timeout_secs,
        max_job_timeout_secs: state.max_job_timeout_secs(),
        control_deadline_extra_secs: state
            .dispatcher_runtime_config()
            .control_deadline_extra_secs(),
        target_counts,
    })
}

pub(crate) fn validate_create_migration_link(
    request: &CreateMigrationLinkRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::conflict("migration_confirmation_required"));
    }
    if request
        .note
        .as_ref()
        .is_some_and(|note| note.len() > MAX_MIGRATION_NOTE_BYTES)
    {
        return Err(ApiError::bad_request("migration_note_too_long"));
    }
    Ok(())
}

fn ensure_migration_run_job_matches_plan(
    request: &CreateMigrationRunRequest,
    restore_plan: &crate::model::RestorePlanView,
) -> Result<(), ApiError> {
    if !request.job.confirmed {
        return Err(ApiError::bad_request(
            "migration_restore_job_confirmation_required",
        ));
    }
    if request.job.command.trim() != "restore" {
        return Err(ApiError::bad_request(
            "migration_restore_job_command_invalid",
        ));
    }
    let targets = request.job.fixed_target_ids()?;
    if targets != [restore_plan.target_client_id.clone()] {
        return Err(ApiError::conflict("migration_restore_job_target_mismatch"));
    }
    let command = request.job.job_command()?;
    let JobCommand::Restore {
        source_backup_request_id,
        paths,
        include_config,
        destination_root,
        ..
    } = command
    else {
        return Err(ApiError::bad_request(
            "migration_restore_job_command_invalid",
        ));
    };
    if source_backup_request_id != restore_plan.source_backup_request_id
        || paths != restore_plan.paths
        || include_config != restore_plan.include_config
        || destination_root != restore_plan.destination_root
    {
        return Err(ApiError::conflict("migration_restore_job_plan_mismatch"));
    }
    Ok(())
}

async fn verify_migration_link_privilege(
    state: &AppState,
    request: &CreateMigrationLinkRequest,
    restore_plan: &MigrationLinkRestorePlan,
) -> Result<(), ApiError> {
    let payload = serde_json::to_vec(&serde_json::json!({
        "version": 1,
        "restore_plan_id": request.restore_plan_id,
        "source_backup_request_id": restore_plan.source_backup_request_id,
        "source_client_id": restore_plan.source_client_id,
        "target_client_id": restore_plan.target_client_id,
        "paths": restore_plan.paths,
        "include_config": restore_plan.include_config,
        "destination_root": restore_plan.destination_root,
        "note": request.note,
    }))
    .map_err(|error| {
        ApiError::from(anyhow::anyhow!(
            "migration_link_payload_hash_failed: {error}"
        ))
    })?;
    let payload_hash = payload_hash(&payload);
    let targets = vec![
        restore_plan.source_client_id.clone(),
        restore_plan.target_client_id.clone(),
    ];
    let target = request.restore_plan_id.to_string();
    let intent = DbPrivilegeIntent::new(
        "migration.link",
        &target,
        None,
        &targets,
        true,
        Some(&payload_hash),
    );
    verify_privilege_intent(state, &intent, request.privilege_assertion.clone()).await
}

type MigrationLinkRestorePlan = crate::model::RestorePlanView;
