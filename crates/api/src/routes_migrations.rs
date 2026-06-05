use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::{
    error::ApiError,
    model::{
        CreateMigrationLinkRequest, ListQuery, MigrationLinkStatus, MigrationLinkView,
        RestorePlanStatus,
    },
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
