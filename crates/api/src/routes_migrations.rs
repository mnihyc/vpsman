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
    routes_jobs::create_job_with_operator,
    security::operator_has_scope,
    state::AppState,
};

const MAX_MIGRATION_NOTE_BYTES: usize = 1024;

pub(crate) async fn list_migration_links(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<MigrationLinkView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
        Json(
            state
                .repo
                .record_migration_link(
                    &request,
                    &restore_plan,
                    &operator,
                    MigrationLinkStatus::LinkedMetadataOnly,
                )
                .await?,
        ),
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
    if state
        .repo
        .query_migration_links(&ListQuery {
            limit: Some(1000),
            offset: None,
            q: None,
            sort: None,
            dir: None,
        })
        .await?
        .iter()
        .any(|link| link.restore_plan_id == restore_plan.id)
    {
        return Err(ApiError::conflict("migration_link_already_exists"));
    }
    verify_migration_link_privilege(&state, &request.link, &restore_plan).await?;
    let (_, Json(restore_job)) = create_job_with_operator(&state, &operator, request.job).await?;
    let migration_link = state
        .repo
        .record_migration_link(
            &request.link,
            &restore_plan,
            &operator,
            MigrationLinkStatus::LinkedMetadataOnly,
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateMigrationRunResponse {
            migration_link,
            restore_job,
        }),
    ))
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
