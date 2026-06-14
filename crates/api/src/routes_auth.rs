use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use uuid::Uuid;

use crate::{
    error::ApiError,
    model::{
        is_valid_operator_timezone, AuthResponse, BootstrapOperatorRequest, CreateOperatorRequest,
        HistoryQuery, LoginRequest, OperatorPreferences, OperatorSessionView, OperatorView,
        RefreshRequest, TotpConfirmRequest, TotpDisableRequest, TotpSetupOutcome, TotpSetupRequest,
        TotpSetupResponse, TotpUpdateOutcome,
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
    match state.repo.bootstrap_operator(&request).await {
        Ok(response) => Ok(Json(response)),
        Err(error) if error.to_string() == "operator_already_bootstrapped" => {
            Err(ApiError::conflict("operator_already_bootstrapped"))
        }
        Err(error) => Err(error.into()),
    }
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
        TotpUpdateOutcome::Updated(operator) => Ok(Json(*operator)),
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
        TotpUpdateOutcome::Updated(operator) => Ok(Json(*operator)),
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

pub(crate) async fn update_operator_preferences(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<OperatorPreferences>,
) -> Result<Json<OperatorView>, ApiError> {
    validate_operator_preferences(&request)?;
    let operator = state.require_operator(&headers).await?;
    Ok(Json(
        state
            .repo
            .update_operator_preferences(&operator, request.normalized())
            .await?,
    ))
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

fn validate_operator_preferences(preferences: &OperatorPreferences) -> Result<(), ApiError> {
    if !matches!(
        preferences.vps_name_display_mode.trim(),
        "name" | "name_id_suffix"
    ) {
        return Err(ApiError::bad_request("invalid_vps_name_display_mode"));
    }
    if !matches!(preferences.language.trim(), "en") {
        return Err(ApiError::bad_request("unsupported_operator_language"));
    }
    if !matches!(
        preferences.sidebar_subpanel_default.trim(),
        "active" | "all"
    ) {
        return Err(ApiError::bad_request("invalid_sidebar_subpanel_default"));
    }
    if let Some(timezone) = preferences.timezone.as_deref() {
        let timezone = timezone.trim();
        if !timezone.is_empty() && !is_valid_operator_timezone(timezone) {
            return Err(ApiError::bad_request("invalid_timezone"));
        }
    }
    if preferences.dashboard_curve_exclusions.len() > 50 {
        return Err(ApiError::bad_request("too_many_dashboard_curve_exclusions"));
    }
    if preferences
        .dashboard_curve_exclusions
        .iter()
        .any(|value| value.trim().len() > 128)
    {
        return Err(ApiError::bad_request("dashboard_curve_exclusion_too_long"));
    }
    if !(3..=16).contains(&preferences.dashboard_resource_top_limit) {
        return Err(ApiError::bad_request(
            "invalid_dashboard_resource_top_limit",
        ));
    }
    if !(3..=16).contains(&preferences.dashboard_network_top_limit) {
        return Err(ApiError::bad_request("invalid_dashboard_network_top_limit"));
    }
    if !matches!(
        preferences.bulk_output_compare_mode.trim(),
        "binary" | "text"
    ) {
        return Err(ApiError::bad_request("invalid_bulk_output_compare_mode"));
    }
    if let Some(key) = preferences.gateway_server_public_key_hex.as_deref() {
        if key.len() != 64 || !key.as_bytes().iter().all(u8::is_ascii_hexdigit) {
            return Err(ApiError::bad_request(
                "invalid_gateway_server_public_key_hex",
            ));
        }
    }
    if !preferences.gateway_endpoints.trim().is_empty()
        && !validate_gateway_endpoints_format(preferences.gateway_endpoints.trim())
    {
        return Err(ApiError::bad_request("invalid_gateway_endpoints"));
    }
    Ok(())
}

fn validate_gateway_endpoints_format(value: &str) -> bool {
    value
        .lines()
        .all(|line| validate_gateway_endpoint_entry(line.trim()))
}

fn validate_gateway_endpoint_entry(entry: &str) -> bool {
    if entry.is_empty() {
        return true;
    }
    let parts: Vec<&str> = entry.splitn(3, '=').collect();
    if parts.len() != 3 {
        return false;
    }
    let label = parts[0];
    let addr = parts[1];
    let priority = parts[2];
    if label.is_empty()
        || label.len() > 64
        || !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return false;
    }
    if addr.is_empty() || addr.len() > 256 || addr.contains(char::is_control) {
        return false;
    }
    if priority.parse::<u16>().is_err() {
        return false;
    }
    true
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
