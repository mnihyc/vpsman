use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use uuid::Uuid;

use crate::{
    error::ApiError,
    model::{
        AuthResponse, BootstrapOperatorRequest, CreateOperatorRequest, HistoryQuery, LoginRequest,
        OperatorSessionView, OperatorView, RefreshRequest, TotpConfirmRequest, TotpDisableRequest,
        TotpSetupOutcome, TotpSetupRequest, TotpSetupResponse, TotpUpdateOutcome,
    },
    security::{normalize_operator_scopes, validate_operator_credentials, validate_operator_role},
    state::AppState,
};

pub(crate) async fn bootstrap_operator(
    State(state): State<AppState>,
    Json(request): Json<BootstrapOperatorRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    validate_operator_credentials(&request.username, &request.password)?;
    if state.repo.operator_count().await? > 0 {
        return Err(ApiError::conflict("operator_already_bootstrapped"));
    }
    Ok(Json(state.repo.bootstrap_operator(&request).await?))
}

pub(crate) async fn login_operator(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    validate_operator_credentials(&request.username, &request.password)?;
    state
        .repo
        .login_operator(&request)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::unauthorized("invalid_operator_credentials"))
}

pub(crate) async fn refresh_operator_session(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    state
        .repo
        .refresh_operator_session(&request.refresh_token)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::unauthorized("invalid_refresh_token"))
}

pub(crate) async fn setup_operator_totp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<TotpSetupRequest>,
) -> Result<Json<TotpSetupResponse>, ApiError> {
    let operator = state.require_operator(&headers).await?;
    if request.password.len() < 12 {
        return Err(ApiError::bad_request("password_too_short"));
    }
    match state
        .repo
        .setup_operator_totp(&operator, &request.password)
        .await?
    {
        TotpSetupOutcome::Created(response) => Ok(Json(response)),
        TotpSetupOutcome::AlreadyEnabled => Err(ApiError::conflict("totp_already_enabled")),
        TotpSetupOutcome::InvalidPassword => {
            Err(ApiError::unauthorized("invalid_operator_credentials"))
        }
        TotpSetupOutcome::OperatorMissing => Err(ApiError::not_found("operator_not_found")),
    }
}

pub(crate) async fn confirm_operator_totp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<TotpConfirmRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    let operator = state.require_operator(&headers).await?;
    validate_totp_update_request(&request.password, &request.code)?;
    match state
        .repo
        .confirm_operator_totp(&operator, &request.password, &request.code)
        .await?
    {
        TotpUpdateOutcome::Updated(operator) => Ok(Json(operator)),
        TotpUpdateOutcome::InvalidCredentials => {
            Err(ApiError::unauthorized("invalid_totp_credentials"))
        }
        TotpUpdateOutcome::NotConfigured => Err(ApiError::conflict("totp_not_configured")),
        TotpUpdateOutcome::OperatorMissing => Err(ApiError::not_found("operator_not_found")),
    }
}

pub(crate) async fn disable_operator_totp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<TotpDisableRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    let operator = state.require_operator(&headers).await?;
    validate_totp_update_request(&request.password, &request.code)?;
    match state
        .repo
        .disable_operator_totp(&operator, &request.password, &request.code)
        .await?
    {
        TotpUpdateOutcome::Updated(operator) => Ok(Json(operator)),
        TotpUpdateOutcome::InvalidCredentials => {
            Err(ApiError::unauthorized("invalid_totp_credentials"))
        }
        TotpUpdateOutcome::NotConfigured => Err(ApiError::conflict("totp_not_configured")),
        TotpUpdateOutcome::OperatorMissing => Err(ApiError::not_found("operator_not_found")),
    }
}

pub(crate) async fn current_operator(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<OperatorView>, ApiError> {
    Ok(Json(state.require_operator(&headers).await?.operator))
}

fn validate_totp_update_request(password: &str, code: &str) -> Result<(), ApiError> {
    if password.len() < 12 {
        return Err(ApiError::bad_request("password_too_short"));
    }
    let code = code.trim().replace(' ', "");
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::bad_request("invalid_totp_code"));
    }
    Ok(())
}

pub(crate) async fn list_operators(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<OperatorView>>, ApiError> {
    let _operator = state.require_operator_role(&headers, "admin").await?;
    Ok(Json(state.repo.list_operators().await?))
}

pub(crate) async fn create_operator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateOperatorRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    let operator = state.require_operator_role(&headers, "admin").await?;
    validate_operator_credentials(&request.username, &request.password)?;
    validate_operator_role(&request.role)?;
    let _scopes = normalize_operator_scopes(&request.role, &request.scopes)?;
    if state
        .repo
        .operator_by_username(&request.username)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict("operator_username_exists"));
    }
    Ok(Json(state.repo.create_operator(&request, &operator).await?))
}

pub(crate) async fn list_operator_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<OperatorSessionView>>, ApiError> {
    let operator = state.require_operator_role(&headers, "admin").await?;
    Ok(Json(
        state
            .repo
            .list_operator_sessions(query.limit.unwrap_or(50), operator.session_id)
            .await?,
    ))
}

pub(crate) async fn revoke_operator_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<Uuid>,
) -> Result<Json<OperatorSessionView>, ApiError> {
    let operator = state.require_operator_role(&headers, "admin").await?;
    state
        .repo
        .revoke_operator_session(session_id, &operator)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("operator_session_not_found"))
}
