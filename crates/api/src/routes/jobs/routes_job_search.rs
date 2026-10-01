use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};

use crate::{
    error::ApiError,
    repository_job_search::{
        search_fields, JobSearch, JobSearchPage, JobSearchRequest, JobSearchValue,
        JobSearchValuesQuery, SearchField,
    },
    security::{operator_has_scope, role_allows, SCOPE_FLEET_READ, SCOPE_SCHEDULES_READ},
    state::AppState,
};

pub(crate) async fn search_jobs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<JobSearchRequest>,
) -> Result<Json<JobSearchPage>, ApiError> {
    state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    let search = JobSearch::parse(&request)
        .map_err(|error| ApiError::bad_request_with_message("invalid_job_search", error))?;
    if search.uses_field("actor") {
        state.require_operator_role(&headers, "admin").await?;
    }
    if search.uses_field("schedule") {
        state
            .require_operator_scope(&headers, SCOPE_SCHEDULES_READ)
            .await?;
    }
    let page = state.repo.search_jobs(&search).await.map_err(|error| {
        if let Some(sqlx::Error::Database(db)) = error.downcast_ref::<sqlx::Error>() {
            if db.code().as_deref() == Some("2201B") {
                return ApiError::bad_request_with_message(
                    "invalid_job_search",
                    format!("Invalid PostgreSQL regular expression: {}", db.message()),
                );
            }
        }
        ApiError::internal(
            "job_search_unavailable",
            "Job history could not be searched.",
            error,
        )
    })?;
    Ok(Json(page))
}

pub(crate) async fn job_search_fields(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SearchField>>, ApiError> {
    let session = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    let fields = search_fields()
        .into_iter()
        .filter(|field| {
            (field.name != "actor" || role_allows(&session.operator.role, "admin"))
                && (field.name != "schedule"
                    || operator_has_scope(&session.operator.scopes, SCOPE_SCHEDULES_READ))
        })
        .collect();
    Ok(Json(fields))
}

pub(crate) async fn job_search_values(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<JobSearchValuesQuery>,
) -> Result<Json<Vec<JobSearchValue>>, ApiError> {
    state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    if query.field == "actor" {
        state.require_operator_role(&headers, "admin").await?;
    }
    if query.field == "schedule" {
        state
            .require_operator_scope(&headers, SCOPE_SCHEDULES_READ)
            .await?;
    }
    if !matches!(
        query.field.as_str(),
        "target"
            | "client_id"
            | "actor"
            | "actor_id"
            | "schedule"
            | "schedule_id"
            | "source_schedule_id"
    ) {
        return Err(ApiError::bad_request_with_message(
            "invalid_job_search",
            "This field has no remote value hints.",
        ));
    }
    Ok(Json(state.repo.job_search_values(&query).await.map_err(
        ApiError::internal_mapper(
            "job_search_hints_unavailable",
            "Job search hints could not be loaded.",
        ),
    )?))
}
