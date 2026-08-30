use std::{future::Future, time::Duration};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::{
    job_request::{fixed_target_selection, normalized_target_client_ids},
    model::{
        BulkResolveRequest, RuntimeConfigBulkApplyRequest, RuntimeConfigBulkApplyResponse,
        RuntimeConfigBulkPreviewRequest, RuntimeConfigBulkPreviewView,
        RuntimeConfigOverrideApplyRequest, RuntimeConfigOverrideApplyResponse,
        RuntimeConfigOverridePreviewRequest, RuntimeConfigOverridePreviewView,
        RuntimeConfigOverrideReplacement, RuntimeConfigWorkspaceView,
    },
    privilege::{verify_privilege_intent, DbPrivilegeIntent},
    runtime_config::dispatch_runtime_config_for_clients,
    runtime_config_workspace::{
        load_runtime_config_workspace, preview_runtime_config_bulk, preview_runtime_config_override,
    },
    security::{operator_has_scope, require_vps_rule_selector_scope, SCOPE_CONFIG_READ},
    selector_expression::parse_selector_expression,
    state::AppState,
    ApiError,
};

const RUNTIME_CONFIG_GUARD_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
const RUNTIME_CONFIG_LOCKED_REVIEW_BASE_SECS: u64 = 10;
const RUNTIME_CONFIG_LOCKED_REVIEW_MAX_SECS: u64 = 60;

pub(crate) async fn get_runtime_config_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
) -> Result<Json<RuntimeConfigWorkspaceView>, ApiError> {
    state
        .require_operator_scope(&headers, SCOPE_CONFIG_READ)
        .await?;
    validate_client_id(&client_id)?;
    Ok(Json(
        load_runtime_config_workspace(&state, &client_id).await?,
    ))
}

pub(crate) async fn preview_single_runtime_config_override(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<RuntimeConfigOverridePreviewRequest>,
) -> Result<Json<RuntimeConfigOverridePreviewView>, ApiError> {
    require_config_editor(&state, &headers).await?;
    validate_client_id(&client_id)?;
    validate_reason(request.reason.as_deref())?;
    Ok(Json(
        preview_runtime_config_override(&state, &client_id, &request.candidate).await?,
    ))
}

pub(crate) async fn apply_single_runtime_config_override(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<RuntimeConfigOverrideApplyRequest>,
) -> Result<Json<RuntimeConfigOverrideApplyResponse>, ApiError> {
    let operator = require_config_editor(&state, &headers).await?;
    validate_client_id(&client_id)?;
    validate_reason(request.reason.as_deref())?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "runtime_config_override_confirmation_required",
        ));
    }
    let preview = preview_runtime_config_override(&state, &client_id, &request.candidate).await?;
    require_single_preview_match(&request, &preview)?;
    verify_privilege_intent(
        &state,
        &DbPrivilegeIntent::new(
            "runtime_config.override.apply",
            &format!("client:{client_id}"),
            None,
            std::slice::from_ref(&client_id),
            true,
            Some(&preview.preview_hash),
        ),
        request.privilege_assertion.clone(),
    )
    .await?;
    let desired_state_guard =
        acquire_runtime_config_desired_state_guard(&state, std::slice::from_ref(&client_id))
            .await?;
    let preview = bounded_runtime_config_locked_review(
        locked_review_timeout(1),
        preview_runtime_config_override(&state, &client_id, &request.candidate),
    )
    .await?;
    require_single_preview_match(&request, &preview)?;
    let reason = normalized_reason(
        request.reason.as_deref(),
        "operator_runtime_config_override",
    );
    let overrides = state
        .repo
        .replace_runtime_config_overrides_cas_locked(
            desired_state_guard,
            &[RuntimeConfigOverrideReplacement {
                client_id: client_id.clone(),
                expected_revision: preview.override_revision.clone(),
                toml: preview.canonical_toml.clone(),
            }],
            &reason,
            &operator,
        )
        .await
        .map_err(runtime_config_override_mutation_error)?;
    let override_record = overrides.into_iter().next();
    let sync = if preview.changes.is_empty() && !preview.recovery_sync_required {
        Vec::new()
    } else {
        dispatch_runtime_config_for_clients(&state, &operator, [client_id], &reason).await
    };
    Ok(Json(RuntimeConfigOverrideApplyResponse {
        preview,
        override_record,
        sync,
    }))
}

pub(crate) async fn preview_bulk_runtime_config_overrides(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RuntimeConfigBulkPreviewRequest>,
) -> Result<Json<RuntimeConfigBulkPreviewView>, ApiError> {
    let operator = require_config_editor(&state, &headers).await?;
    validate_reason(request.reason.as_deref())?;
    let selector_expression = request.selector_expression.trim().to_string();
    let target_client_ids = resolve_preview_targets(
        &state,
        &operator.operator.scopes,
        &selector_expression,
        &request.target_client_ids,
    )
    .await?;
    let (preview, _) = preview_runtime_config_bulk(
        &state,
        &selector_expression,
        &target_client_ids,
        &request.patch,
    )
    .await?;
    Ok(Json(preview))
}

pub(crate) async fn apply_bulk_runtime_config_overrides(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RuntimeConfigBulkApplyRequest>,
) -> Result<Json<RuntimeConfigBulkApplyResponse>, ApiError> {
    let operator = require_config_editor(&state, &headers).await?;
    validate_reason(request.reason.as_deref())?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "runtime_config_bulk_confirmation_required",
        ));
    }
    // Apply never re-resolves the selector. The reviewed IDs are the complete
    // mutation set; selector_expression remains immutable audit context only.
    let target_client_ids = verified_fixed_target_ids(&state, &request.target_client_ids).await?;
    let selector_expression = request.selector_expression.trim().to_string();
    if !selector_expression.is_empty() {
        let expression = parse_selector_expression(&selector_expression)
            .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
            .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
        require_vps_rule_selector_scope(&operator.operator.scopes, &expression)?;
    }
    let (preview, _) = preview_runtime_config_bulk(
        &state,
        &selector_expression,
        &target_client_ids,
        &request.patch,
    )
    .await?;
    if preview.preview_hash != request.preview_hash {
        return Err(ApiError::conflict("runtime_config_bulk_review_stale"));
    }
    verify_privilege_intent(
        &state,
        &DbPrivilegeIntent::new(
            "runtime_config.override.bulk_apply",
            "runtime_config",
            (!selector_expression.is_empty()).then_some(selector_expression.as_str()),
            &target_client_ids,
            true,
            Some(&preview.preview_hash),
        ),
        request.privilege_assertion.clone(),
    )
    .await?;
    let desired_state_guard =
        acquire_runtime_config_desired_state_guard(&state, &target_client_ids).await?;
    let (preview, candidates) = bounded_runtime_config_locked_review(
        locked_review_timeout(target_client_ids.len()),
        preview_runtime_config_bulk(
            &state,
            &selector_expression,
            &target_client_ids,
            &request.patch,
        ),
    )
    .await?;
    if preview.preview_hash != request.preview_hash {
        return Err(ApiError::conflict("runtime_config_bulk_review_stale"));
    }
    let replacements = candidates
        .iter()
        .filter(|candidate| !candidate.no_op)
        .map(|candidate| RuntimeConfigOverrideReplacement {
            client_id: candidate.client_id.clone(),
            expected_revision: candidate.expected_revision.clone(),
            toml: candidate.canonical_toml.clone(),
        })
        .collect::<Vec<_>>();
    let reason = normalized_reason(
        request.reason.as_deref(),
        "operator_bulk_runtime_config_override",
    );
    let overrides = state
        .repo
        .replace_runtime_config_overrides_cas_locked(
            desired_state_guard,
            &replacements,
            &reason,
            &operator,
        )
        .await
        .map_err(runtime_config_override_mutation_error)?;
    let runtime_changed_client_ids = candidates
        .iter()
        .filter(|candidate| !candidate.no_op && !candidate.storage_only)
        .map(|candidate| candidate.client_id.clone())
        .collect::<Vec<_>>();
    let sync = if runtime_changed_client_ids.is_empty() {
        Vec::new()
    } else {
        dispatch_runtime_config_for_clients(&state, &operator, runtime_changed_client_ids, &reason)
            .await
    };
    Ok(Json(RuntimeConfigBulkApplyResponse {
        preview,
        overrides,
        sync_job_ids: sync.iter().filter_map(|outcome| outcome.job_id).collect(),
        sync,
    }))
}

async fn acquire_runtime_config_desired_state_guard(
    state: &AppState,
    client_ids: &[String],
) -> Result<crate::repository_runtime_config::RuntimeConfigDesiredStateGuard, ApiError> {
    match tokio::time::timeout(
        RUNTIME_CONFIG_GUARD_ACQUIRE_TIMEOUT,
        state.repo.lock_runtime_config_desired_state(client_ids),
    )
    .await
    {
        Ok(Ok(guard)) => Ok(guard),
        Ok(Err(error)) => Err(runtime_config_desired_state_guard_error(error)),
        Err(_) => Err(runtime_config_locked_phase_unavailable(
            "runtime_config_desired_state_busy",
        )),
    }
}

async fn bounded_runtime_config_locked_review<T>(
    timeout: Duration,
    review: impl Future<Output = Result<T, ApiError>>,
) -> Result<T, ApiError> {
    tokio::time::timeout(timeout, review)
        .await
        .map_err(|_| runtime_config_locked_phase_unavailable("runtime_config_desired_state_busy"))?
}

fn locked_review_timeout(target_count: usize) -> Duration {
    let scaled_secs = RUNTIME_CONFIG_LOCKED_REVIEW_BASE_SECS
        .saturating_add(u64::try_from(target_count).unwrap_or(u64::MAX) / 5)
        .min(RUNTIME_CONFIG_LOCKED_REVIEW_MAX_SECS);
    Duration::from_secs(scaled_secs)
}

fn runtime_config_locked_phase_unavailable(code: &'static str) -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code,
        error: anyhow::anyhow!(code),
        public_message: Some(
            "Runtime configuration inputs are busy; no override was changed. Retry the reviewed apply."
                .to_string(),
        ),
    }
}

fn runtime_config_desired_state_guard_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("runtime_config_desired_state_busy")
        || message.contains("lock timeout")
        || message.contains("pool timed out")
    {
        runtime_config_locked_phase_unavailable("runtime_config_desired_state_busy")
    } else {
        ApiError::from(error)
    }
}

async fn require_config_editor(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<crate::model::AuthContext, ApiError> {
    let operator = state
        .require_operator_role_and_scope(headers, "operator", "config:write")
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_CONFIG_READ) {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    Ok(operator)
}

async fn resolve_preview_targets(
    state: &AppState,
    operator_scopes: &[String],
    selector_expression: &str,
    fixed_ids: &[String],
) -> Result<Vec<String>, ApiError> {
    if !fixed_ids.is_empty() {
        if !selector_expression.is_empty() {
            let expression = parse_selector_expression(selector_expression)
                .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
                .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
            require_vps_rule_selector_scope(operator_scopes, &expression)?;
        }
        return verified_fixed_target_ids(state, fixed_ids).await;
    }
    if selector_expression.is_empty() {
        return verified_fixed_target_ids(state, fixed_ids).await;
    }
    let expression = parse_selector_expression(selector_expression)
        .map_err(|_| ApiError::bad_request("invalid_selector_expression"))?
        .ok_or_else(|| ApiError::bad_request("invalid_selector_expression"))?;
    require_vps_rule_selector_scope(operator_scopes, &expression)?;
    let mut target_client_ids = state
        .repo
        .resolve_bulk_targets(&BulkResolveRequest {
            selector_expression: selector_expression.to_string(),
        })
        .await
        .map_err(ApiError::internal_mapper(
            "runtime_config_bulk_targets_resolve_failed",
            "Runtime configuration targets could not be resolved.",
        ))?
        .targets
        .into_iter()
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    target_client_ids.sort();
    normalized_target_client_ids(&target_client_ids)
}

async fn verified_fixed_target_ids(
    state: &AppState,
    target_client_ids: &[String],
) -> Result<Vec<String>, ApiError> {
    let target_client_ids = normalized_target_client_ids(target_client_ids)?;
    let resolved = state
        .repo
        .resolve_bulk_targets(&fixed_target_selection(&target_client_ids)?)
        .await
        .map_err(ApiError::internal_mapper(
            "runtime_config_bulk_targets_unavailable",
            "Runtime configuration targets could not be loaded.",
        ))?
        .targets
        .into_iter()
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    if target_client_ids
        .iter()
        .any(|client_id| !resolved.contains(client_id))
    {
        return Err(ApiError::conflict(
            "runtime_config_bulk_target_no_longer_available",
        ));
    }
    Ok(target_client_ids)
}

fn require_single_preview_match(
    request: &RuntimeConfigOverrideApplyRequest,
    preview: &RuntimeConfigOverridePreviewView,
) -> Result<(), ApiError> {
    if request.expected_override_revision != preview.override_revision
        || request.expected_desired_hash != preview.desired_hash
        || request.preview_hash != preview.preview_hash
    {
        return Err(ApiError::conflict("runtime_config_override_review_stale"));
    }
    Ok(())
}

fn validate_client_id(client_id: &str) -> Result<(), ApiError> {
    if client_id.is_empty()
        || client_id.len() > 128
        || client_id.chars().any(|character| character.is_control())
    {
        return Err(ApiError::bad_request("invalid_client_id"));
    }
    Ok(())
}

fn validate_reason(reason: Option<&str>) -> Result<(), ApiError> {
    if reason.is_some_and(|reason| {
        reason.len() > vpsman_common::MAX_RUNTIME_CONFIG_REASON_BYTES
            || reason.chars().any(char::is_control)
    }) {
        return Err(ApiError::bad_request("runtime_config_reason_invalid"));
    }
    Ok(())
}

fn normalized_reason(reason: Option<&str>, fallback: &str) -> String {
    reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn runtime_config_override_mutation_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("review_stale") {
        ApiError::conflict("runtime_config_override_review_stale")
    } else if message.contains("target_no_longer_available") {
        ApiError::conflict("runtime_config_target_no_longer_available")
    } else {
        ApiError::internal(
            "runtime_config_override_mutation_failed",
            "The runtime configuration override could not be saved.",
            error,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn locked_review_timeout_fails_closed_without_mutation() {
        let error = bounded_runtime_config_locked_review(
            Duration::from_millis(5),
            std::future::pending::<Result<(), ApiError>>(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, "runtime_config_desired_state_busy");
    }

    #[test]
    fn bulk_locked_review_timeout_is_bounded() {
        assert_eq!(locked_review_timeout(1), Duration::from_secs(10));
        assert_eq!(locked_review_timeout(500), Duration::from_secs(60));
        assert_eq!(locked_review_timeout(usize::MAX), Duration::from_secs(60));
    }

    #[test]
    fn exact_guard_lock_timeouts_are_retryable() {
        for message in [
            "runtime_config_desired_state_busy",
            "canceling statement due to lock timeout",
        ] {
            let error = runtime_config_desired_state_guard_error(anyhow::anyhow!(message));
            assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        }
    }
}
