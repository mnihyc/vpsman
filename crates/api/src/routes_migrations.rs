use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use vpsman_common::{payload_hash, JobCommand};

use crate::{
    error::ApiError,
    model::{
        CreateMigrationLinkRequest, CreateMigrationRunRequest, CreateMigrationRunResponse,
        ListQuery, MigrationLinkStatus, MigrationLinkView, RestorePlanStatus,
    },
    privilege::{verify_privilege_intent, DbPrivilegeIntent},
    routes_jobs::{
        create_job_with_operator, effective_job_timeout_secs, validate_restore_archive_binding,
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
    Json(request): Json<CreateMigrationRunRequest>,
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
    preflight_migration_restore_job(&state, &request).await?;
    let migration_link =
        record_migration_link_or_conflict(&state, &request.link, &restore_plan, &operator).await?;
    let restore_plan_id = request.link.restore_plan_id;
    let restore_job_result = create_job_with_operator(&state, &operator, request.job).await;
    let (_, Json(restore_job)) = match restore_job_result {
        Ok(response) => response,
        Err(error) => {
            state
                .repo
                .delete_migration_link_for_restore_plan(restore_plan_id)
                .await?;
            return Err(error);
        }
    };
    Ok((
        StatusCode::CREATED,
        Json(CreateMigrationRunResponse {
            migration_link,
            restore_job,
        }),
    ))
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
    request: &CreateMigrationRunRequest,
) -> Result<(), ApiError> {
    if !request.job.privileged {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    effective_job_timeout_secs(request.job.timeout_secs, state.max_job_timeout_secs())?;
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
    if !state.gateway.configured() {
        return Err(ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "gateway_control_url_missing",
            error: anyhow::anyhow!("gateway_control_url_missing"),
        });
    }
    Ok(())
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
