use std::{
    collections::{HashMap, HashSet},
    net::{Ipv6Addr, SocketAddr},
};

use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{header::USER_AGENT, HeaderMap, StatusCode},
    Json,
};
use uuid::Uuid;

use crate::{
    error::ApiError,
    gateway_client::GatewayControlResponseError,
    model::{
        is_valid_operator_timezone, AuthContext, AuthResponse, BootstrapOperatorRequest,
        BootstrapStatusResponse, BulkOperatorMutationItem, BulkOperatorMutationOutcome,
        BulkOperatorMutationResponse, BulkOperatorSessionRevokeItem,
        BulkOperatorSessionRevokeOutcome, BulkOperatorSessionRevokeRequest,
        BulkOperatorSessionRevokeResponse, BulkOperatorStatusRequest, BulkOperatorTotpClearRequest,
        CreateOperatorRequest, HistoryQuery, LoginRequest, OperatorAuthEventQuery,
        OperatorAuthEventView, OperatorLifecycleRequest, OperatorLifecycleStatus,
        OperatorPasswordResetRequest, OperatorPreferences, OperatorSessionRevokeRequest,
        OperatorSessionView, OperatorView, RefreshRequest, TotpConfirmRequest, TotpDisableRequest,
        TotpSetupOutcome, TotpSetupRequest, TotpSetupResponse, TotpUpdateOutcome,
        UpdateOperatorRequest,
    },
    privilege::{verify_privilege_intent, DbPrivilegeIntent},
    repository_auth::{AccessBatchMutationOutcome, OperatorLoginAttempt},
    security::{
        bearer_token, normalize_operator_scopes, validate_operator_credentials,
        validate_operator_role, DEFAULT_REFRESH_TOKEN_TTL_SECS, MAX_REFRESH_TOKEN_TTL_SECS,
        MIN_REFRESH_TOKEN_TTL_SECS,
    },
    state::AppState,
};
use vpsman_common::{
    operator_db_payload_hash, GatewayPrivilegeVerification, GatewayPrivilegeVerificationBatchItem,
    OperatorDbPayloadInput, PrivilegeAssertion, GATEWAY_CONTROL_BATCH_MAX_ITEMS,
};

pub(crate) async fn bootstrap_status(
    State(state): State<AppState>,
) -> Result<Json<BootstrapStatusResponse>, ApiError> {
    Ok(Json(BootstrapStatusResponse {
        bootstrap_required: state.repo.operator_count().await.map_err(
            ApiError::internal_mapper(
                "operator_bootstrap_status_unavailable",
                "The operator bootstrap status could not be loaded.",
            ),
        )? == 0,
    }))
}

pub(crate) async fn bootstrap_operator(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<BootstrapOperatorRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    validate_operator_credentials(&request.username, &request.password)?;
    if state
        .repo
        .operator_count()
        .await
        .map_err(ApiError::internal_mapper(
            "operator_bootstrap_status_unavailable",
            "The operator bootstrap status could not be loaded.",
        ))?
        > 0
    {
        return Err(ApiError::conflict("operator_already_bootstrapped"));
    }
    let remote_ip = state.operator_client_ip(peer, &headers);
    match state
        .repo
        .bootstrap_operator_with_auth_event(
            &request,
            &remote_ip,
            headers
                .get(USER_AGENT)
                .and_then(|value| value.to_str().ok()),
        )
        .await
    {
        Ok(response) => Ok(Json(response)),
        Err(error) if error.to_string() == "operator_already_bootstrapped" => {
            Err(ApiError::conflict("operator_already_bootstrapped"))
        }
        Err(error) => Err(ApiError::internal(
            "operator_bootstrap_failed",
            "The initial operator could not be created.",
            error,
        )),
    }
}

pub(crate) async fn login_operator(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    validate_operator_credentials(&request.username, &request.password)?;
    let remote_ip = state.operator_client_ip(peer, &headers);
    match state
        .repo
        .login_operator_with_throttle(
            &request,
            &remote_ip,
            headers
                .get(USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            &state.operator_auth_throttle_config(),
        )
        .await
        .map_err(ApiError::internal_mapper(
            "operator_login_failed",
            "The operator login could not be completed.",
        ))? {
        OperatorLoginAttempt::Authenticated(response) => Ok(Json(*response)),
        OperatorLoginAttempt::InvalidCredentials => {
            Err(ApiError::unauthorized("invalid_operator_credentials"))
        }
        OperatorLoginAttempt::Throttled => {
            Err(ApiError::too_many_requests("operator_login_throttled"))
        }
    }
}

pub(crate) async fn refresh_operator_session(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    state
        .repo
        .refresh_operator_session(&request.refresh_token)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_session_refresh_failed",
            "The operator session could not be refreshed.",
        ))?
        .map(Json)
        .ok_or_else(|| ApiError::unauthorized("invalid_refresh_token"))
}

pub(crate) async fn logout_operator_session(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let access_token =
        bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("missing_bearer_token"))?;
    let remote_ip = state.operator_client_ip(peer, &headers);
    let user_agent = headers
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok());
    if !state
        .repo
        .logout_operator_session(access_token, &remote_ip, user_agent)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_session_logout_failed",
            "The operator session could not be logged out.",
        ))?
    {
        return Err(ApiError::unauthorized("invalid_operator_session"));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn setup_operator_totp(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<TotpSetupRequest>,
) -> Result<Json<TotpSetupResponse>, ApiError> {
    let operator = state.require_operator(&headers).await?;
    if request.password.len() < 12 {
        return Err(ApiError::bad_request("password_too_short"));
    }
    let remote_ip = state.operator_client_ip(peer, &headers);
    ensure_totp_management_not_locked(&state, &operator, &remote_ip).await?;
    match state
        .repo
        .setup_operator_totp(&operator, &request.password)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_totp_setup_failed",
            "Authenticator setup could not be completed.",
        ))? {
        TotpSetupOutcome::Created(response) => {
            state
                .repo
                .clear_operator_auth_management_success(&operator.operator.username, &remote_ip)
                .await
                .map_err(ApiError::internal_mapper(
                    "operator_auth_throttle_clear_failed",
                    "The authenticator security state could not be cleared.",
                ))?;
            Ok(Json(response))
        }
        TotpSetupOutcome::AlreadyEnabled => Err(ApiError::conflict("totp_already_enabled")),
        TotpSetupOutcome::InvalidPassword => {
            record_totp_management_failure(&state, &operator, &remote_ip).await?;
            Err(ApiError::bad_request_with_message(
                "invalid_totp_credentials",
                "The current password is incorrect.",
            ))
        }
        TotpSetupOutcome::OperatorMissing => Err(ApiError::not_found("operator_not_found")),
    }
}

pub(crate) async fn confirm_operator_totp(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<TotpConfirmRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    let operator = state.require_operator(&headers).await?;
    validate_totp_update_request(&request.password, &request.code)?;
    let remote_ip = state.operator_client_ip(peer, &headers);
    ensure_totp_management_not_locked(&state, &operator, &remote_ip).await?;
    match state
        .repo
        .confirm_operator_totp(&operator, &request.password, &request.code)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_totp_confirmation_failed",
            "Authenticator confirmation could not be completed.",
        ))? {
        TotpUpdateOutcome::Updated(updated) => {
            state
                .repo
                .clear_operator_auth_management_success(&operator.operator.username, &remote_ip)
                .await
                .map_err(ApiError::internal_mapper(
                    "operator_auth_throttle_clear_failed",
                    "The authenticator security state could not be cleared.",
                ))?;
            Ok(Json(*updated))
        }
        TotpUpdateOutcome::AlreadyEnabled => Err(ApiError::conflict("totp_already_enabled")),
        TotpUpdateOutcome::InvalidCredentials => {
            record_totp_management_failure(&state, &operator, &remote_ip).await?;
            Err(ApiError::bad_request_with_message(
                "invalid_totp_credentials",
                "The current password or authenticator code is incorrect.",
            ))
        }
        TotpUpdateOutcome::NotConfigured => Err(ApiError::conflict("totp_not_configured")),
        TotpUpdateOutcome::OperatorMissing => Err(ApiError::not_found("operator_not_found")),
    }
}

pub(crate) async fn disable_operator_totp(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<TotpDisableRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    let operator = state.require_operator(&headers).await?;
    validate_totp_update_request(&request.password, &request.code)?;
    let remote_ip = state.operator_client_ip(peer, &headers);
    ensure_totp_management_not_locked(&state, &operator, &remote_ip).await?;
    match state
        .repo
        .disable_operator_totp(&operator, &request.password, &request.code)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_totp_disable_failed",
            "Authenticator removal could not be completed.",
        ))? {
        TotpUpdateOutcome::Updated(updated) => {
            state
                .repo
                .clear_operator_auth_management_success(&operator.operator.username, &remote_ip)
                .await
                .map_err(ApiError::internal_mapper(
                    "operator_auth_throttle_clear_failed",
                    "The authenticator security state could not be cleared.",
                ))?;
            Ok(Json(*updated))
        }
        TotpUpdateOutcome::AlreadyEnabled => Err(ApiError::conflict("totp_already_enabled")),
        TotpUpdateOutcome::InvalidCredentials => {
            record_totp_management_failure(&state, &operator, &remote_ip).await?;
            Err(ApiError::bad_request_with_message(
                "invalid_totp_credentials",
                "The current password or authenticator code is incorrect.",
            ))
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

async fn ensure_totp_management_not_locked(
    state: &AppState,
    operator: &AuthContext,
    remote_ip: &str,
) -> Result<(), ApiError> {
    if state
        .repo
        .operator_auth_identity_locked(&operator.operator.username, remote_ip)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_auth_throttle_unavailable",
            "The authenticator security state could not be loaded.",
        ))?
    {
        return Err(ApiError::too_many_requests("operator_auth_throttled"));
    }
    Ok(())
}

async fn record_totp_management_failure(
    state: &AppState,
    operator: &AuthContext,
    remote_ip: &str,
) -> Result<(), ApiError> {
    state
        .repo
        .record_operator_totp_management_failure(
            &operator.operator.username,
            remote_ip,
            &state.operator_auth_throttle_config(),
        )
        .await
        .map_err(ApiError::internal_mapper(
            "operator_auth_failure_record_failed",
            "The authenticator failure could not be recorded.",
        ))?;
    Ok(())
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
            .await
            .map_err(ApiError::internal_mapper(
                "operator_preferences_update_failed",
                "The operator preferences could not be saved.",
            ))?,
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
        preferences.fleet_location_display_mode.trim(),
        "country_only" | "country_region"
    ) {
        return Err(ApiError::bad_request("invalid_fleet_location_display_mode"));
    }
    if !matches!(
        preferences.byte_unit_display_mode.trim(),
        "decimal" | "binary"
    ) {
        return Err(ApiError::bad_request("invalid_byte_unit_display_mode"));
    }
    if !matches!(
        preferences.sidebar_subpanel_default.trim(),
        "active" | "all"
    ) {
        return Err(ApiError::bad_request("invalid_sidebar_subpanel_default"));
    }
    if !matches!(preferences.review_prompt_mode.trim(), "inline" | "overlay") {
        return Err(ApiError::bad_request("invalid_review_prompt_mode"));
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
    if preferences.fleet_tag_visibility_overrides.len() > 500 {
        return Err(ApiError::bad_request(
            "too_many_fleet_tag_visibility_overrides",
        ));
    }
    if preferences
        .fleet_tag_visibility_overrides
        .keys()
        .any(|tag| !validate_preference_tag_name(tag))
    {
        return Err(ApiError::bad_request("invalid_fleet_tag_visibility_tag"));
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
    if !matches!(
        preferences.agent_install_mode.trim(),
        "root" | "user" | "staged"
    ) {
        return Err(ApiError::bad_request("invalid_agent_install_mode"));
    }
    validate_tunnel_allocation_preference(
        preferences.tunnel_ipv4_allocation_pool_cidr.trim(),
        TunnelAllocationPreferenceFamily::Ipv4,
        "invalid_tunnel_ipv4_allocation_pool_cidr",
    )?;
    validate_tunnel_allocation_preference(
        preferences.tunnel_ipv6_allocation_pool_cidr.trim(),
        TunnelAllocationPreferenceFamily::Ipv6,
        "invalid_tunnel_ipv6_allocation_pool_cidr",
    )?;
    Ok(())
}

fn validate_preference_tag_name(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 128
        && !tag.starts_with("id:")
        && !tag.starts_with("name:")
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn validate_gateway_endpoints_format(value: &str) -> bool {
    let entries = value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    !entries.is_empty()
        && entries.len() <= 16
        && entries.into_iter().all(validate_gateway_endpoint_entry)
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
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
    {
        return false;
    }
    if !validate_gateway_tcp_address(addr) {
        return false;
    }
    if priority.is_empty()
        || priority.len() > 5
        || !priority.bytes().all(|byte| byte.is_ascii_digit())
        || priority.parse::<u16>().is_err()
    {
        return false;
    }
    true
}

fn validate_gateway_tcp_address(value: &str) -> bool {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_whitespace) {
        return false;
    }
    let (host, port) = if let Some(bracketed) = value.strip_prefix('[') {
        let Some((host, port)) = bracketed.rsplit_once("]:") else {
            return false;
        };
        if !validate_gateway_ipv6_literal(host) {
            return false;
        }
        (host, port)
    } else {
        let Some((host, port)) = value.rsplit_once(':') else {
            return false;
        };
        if host.contains(':') || !validate_gateway_host(host) {
            return false;
        }
        (host, port)
    };
    !host.is_empty()
        && !port.is_empty()
        && port.len() <= 5
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port > 0)
}

fn validate_gateway_host(value: &str) -> bool {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return validate_gateway_ipv4_literal(value);
    }
    if value.is_empty() || value.len() > 253 {
        return false;
    }
    let value = value.strip_suffix('.').unwrap_or(value);
    !value.is_empty()
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
}

fn validate_gateway_ipv4_literal(value: &str) -> bool {
    let octets = value.split('.').collect::<Vec<_>>();
    octets.len() == 4
        && octets.iter().all(|octet| {
            !octet.is_empty()
                && octet.len() <= 3
                && octet.bytes().all(|byte| byte.is_ascii_digit())
                && (octet.len() == 1 || !octet.starts_with('0'))
                && octet.parse::<u16>().is_ok_and(|octet| octet <= 255)
        })
}

fn validate_gateway_ipv6_literal(value: &str) -> bool {
    if !value.contains('.') {
        return value.parse::<Ipv6Addr>().is_ok();
    }
    let Some((prefix, ipv4_tail)) = value.rsplit_once(':') else {
        return false;
    };
    if !validate_gateway_ipv4_literal(ipv4_tail) {
        return false;
    }
    let octets = ipv4_tail
        .split('.')
        .map(|octet| octet.parse::<u16>().expect("validated IPv4 octet"))
        .collect::<Vec<_>>();
    let normalized = format!(
        "{prefix}:{:x}:{:x}",
        (octets[0] << 8) | octets[1],
        (octets[2] << 8) | octets[3],
    );
    normalized.parse::<Ipv6Addr>().is_ok()
}

#[derive(Clone, Copy)]
enum TunnelAllocationPreferenceFamily {
    Ipv4,
    Ipv6,
}

fn validate_tunnel_allocation_preference(
    value: &str,
    family: TunnelAllocationPreferenceFamily,
    error_code: &'static str,
) -> Result<(), ApiError> {
    if value.is_empty() {
        return Ok(());
    }
    let Ok(parsed) = value.parse::<ipnet::IpNet>() else {
        return Err(ApiError::bad_request(error_code));
    };
    let valid = match (family, parsed) {
        (TunnelAllocationPreferenceFamily::Ipv4, ipnet::IpNet::V4(net)) => net.prefix_len() <= 31,
        (TunnelAllocationPreferenceFamily::Ipv6, ipnet::IpNet::V6(net)) => net.prefix_len() <= 127,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(ApiError::bad_request(error_code))
    }
}

pub(crate) async fn list_operators(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<OperatorView>>, ApiError> {
    let _operator = state.require_operator_role(&headers, "admin").await?;
    Ok(Json(state.repo.list_operators().await.map_err(
        ApiError::internal_mapper(
            "operators_unavailable",
            "The operator accounts could not be loaded.",
        ),
    )?))
}

pub(crate) async fn create_operator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateOperatorRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    let operator = state.require_operator_role(&headers, "admin").await?;
    require_confirmed(request.confirmed)?;
    validate_operator_credentials(&request.username, &request.password)?;
    validate_operator_role(&request.role)?;
    let _scopes = normalize_operator_scopes(&request.role, &request.scopes)?;
    let session_refresh_ttl_secs = request
        .session_refresh_ttl_secs
        .unwrap_or(DEFAULT_REFRESH_TOKEN_TTL_SECS);
    validate_session_refresh_ttl(session_refresh_ttl_secs)?;
    if request.role.trim() == "admin" && !request.admin_risk_acknowledged {
        return Err(ApiError::bad_request("admin_risk_acknowledgement_required"));
    }
    if state
        .repo
        .operator_by_username(&request.username)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_unavailable",
            "The operator account could not be loaded.",
        ))?
        .is_some()
    {
        return Err(ApiError::conflict("operator_username_exists"));
    }
    verify_operator_management_privilege(
        &state,
        "operator.create",
        request.username.trim(),
        Some(request.username.trim()),
        Some(request.role.trim()),
        &request.scopes,
        Some(session_refresh_ttl_secs),
        None,
        request.admin_risk_acknowledged,
        request.privilege_assertion.clone(),
    )
    .await?;
    Ok(Json(
        state
            .repo
            .create_operator(&request, &operator)
            .await
            .map_err(ApiError::internal_mapper(
                "operator_create_failed",
                "The operator account could not be created.",
            ))?,
    ))
}

pub(crate) async fn update_operator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(operator_id): Path<Uuid>,
    Json(request): Json<UpdateOperatorRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    let actor = state.require_operator_role(&headers, "admin").await?;
    require_confirmed(request.confirmed)?;
    validate_operator_role(&request.role)?;
    let _scopes = normalize_operator_scopes(&request.role, &request.scopes)?;
    validate_session_refresh_ttl(request.session_refresh_ttl_secs)?;
    let target = state
        .repo
        .operator_by_id(operator_id)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_unavailable",
            "The operator account could not be loaded.",
        ))?
        .filter(|operator| operator.status != "deleted")
        .ok_or_else(|| ApiError::not_found("operator_not_found"))?;
    require_admin_risk_if_needed(
        &target.role,
        Some(&request.role),
        request.admin_risk_acknowledged,
    )?;
    let target = operator_id.to_string();
    verify_operator_management_privilege(
        &state,
        "operator.update",
        &target,
        None,
        Some(request.role.trim()),
        &request.scopes,
        Some(request.session_refresh_ttl_secs),
        None,
        request.admin_risk_acknowledged,
        request.privilege_assertion.clone(),
    )
    .await?;
    state
        .repo
        .update_operator(operator_id, &request, &actor)
        .await
        .map_err(operator_management_error)?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("operator_not_found"))
}

pub(crate) async fn disable_operator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(operator_id): Path<Uuid>,
    Json(request): Json<OperatorLifecycleRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    set_operator_lifecycle_status(state, headers, operator_id, "disabled", request).await
}

pub(crate) async fn bulk_set_operator_statuses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkOperatorStatusRequest>,
) -> Result<Json<BulkOperatorMutationResponse>, ApiError> {
    let actor = state.require_operator_role(&headers, "admin").await?;
    Ok(Json(
        mutate_operator_statuses(&state, &actor, request).await?,
    ))
}

pub(crate) async fn enable_operator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(operator_id): Path<Uuid>,
    Json(request): Json<OperatorLifecycleRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    set_operator_lifecycle_status(state, headers, operator_id, "active", request).await
}

pub(crate) async fn delete_operator(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(operator_id): Path<Uuid>,
    Json(request): Json<OperatorLifecycleRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    set_operator_lifecycle_status(state, headers, operator_id, "deleted", request).await
}

async fn set_operator_lifecycle_status(
    state: AppState,
    headers: HeaderMap,
    operator_id: Uuid,
    status: &str,
    request: OperatorLifecycleRequest,
) -> Result<Json<OperatorView>, ApiError> {
    let actor = state.require_operator_role(&headers, "admin").await?;
    let status = match status {
        "active" => OperatorLifecycleStatus::Active,
        "disabled" => OperatorLifecycleStatus::Disabled,
        "deleted" => OperatorLifecycleStatus::Deleted,
        _ => return Err(ApiError::bad_request("invalid_operator_status")),
    };
    let response = mutate_operator_statuses(
        &state,
        &actor,
        BulkOperatorStatusRequest {
            status,
            items: vec![BulkOperatorMutationItem {
                operator_id,
                privilege_assertion: request.privilege_assertion,
            }],
            confirmed: request.confirmed,
            admin_risk_acknowledged: request.admin_risk_acknowledged,
        },
    )
    .await?;
    singleton_operator_mutation(response)
}

async fn mutate_operator_statuses(
    state: &AppState,
    actor: &AuthContext,
    request: BulkOperatorStatusRequest,
) -> Result<BulkOperatorMutationResponse, ApiError> {
    validate_operator_batch(request.confirmed, &request.items)?;
    let operator_ids = request
        .items
        .iter()
        .map(|item| item.operator_id)
        .collect::<Vec<_>>();
    let snapshots = state
        .repo
        .operator_batch_authority_snapshots(&operator_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_unavailable",
            "The operator accounts could not be loaded.",
        ))?;
    let snapshots = snapshots
        .into_iter()
        .map(|snapshot| (snapshot.operator_id, snapshot))
        .collect::<HashMap<_, _>>();
    let mut outcomes = HashMap::new();
    let mut prepared = Vec::new();
    for item in &request.items {
        let Some(snapshot) = snapshots.get(&item.operator_id) else {
            outcomes.insert(
                item.operator_id,
                rejected_operator_outcome(item.operator_id, "operator_not_found"),
            );
            continue;
        };
        if snapshot.status == "deleted"
            || (request.status == OperatorLifecycleStatus::Active && snapshot.status != "disabled")
        {
            outcomes.insert(
                item.operator_id,
                rejected_operator_outcome(item.operator_id, "operator_not_found"),
            );
            continue;
        }
        if let Err(error) =
            require_admin_risk_if_needed(&snapshot.role, None, request.admin_risk_acknowledged)
        {
            outcomes.insert(
                item.operator_id,
                rejected_operator_outcome(item.operator_id, error.code),
            );
            continue;
        }
        let Some(assertion) = item.privilege_assertion.clone() else {
            outcomes.insert(
                item.operator_id,
                rejected_operator_outcome(item.operator_id, "privilege_assertion_required"),
            );
            continue;
        };
        prepared.push(prepare_operator_privilege_verification(
            item.operator_id,
            request.status.privilege_action(),
            Some(request.status.as_str()),
            request.admin_risk_acknowledged,
            assertion,
        )?);
    }
    let approved = verify_operator_privilege_batch(state, prepared, &mut outcomes).await?;
    let repository_outcomes = if approved.is_empty() {
        Vec::new()
    } else {
        state
            .repo
            .set_operator_statuses(&approved, request.status.as_str(), actor)
            .await
            .map_err(operator_management_error)?
    };
    merge_operator_repository_outcomes(&approved, repository_outcomes, &mut outcomes)?;
    ordered_operator_response(&operator_ids, outcomes)
}

fn singleton_operator_mutation(
    mut response: BulkOperatorMutationResponse,
) -> Result<Json<OperatorView>, ApiError> {
    let outcome = response.outcomes.pop().ok_or_else(|| {
        ApiError::internal(
            "operator_mutation_result_invalid",
            "The operator account change returned no outcome.",
            anyhow::anyhow!("singleton operator mutation outcome missing"),
        )
    })?;
    if let Some(result) = outcome.result {
        Ok(Json(result))
    } else {
        Err(operator_outcome_error(
            outcome
                .error_code
                .as_deref()
                .unwrap_or("operator_management_failed"),
        ))
    }
}

fn operator_outcome_error(code: &str) -> ApiError {
    match code {
        "operator_not_found" => ApiError::not_found("operator_not_found"),
        "last_active_admin_required" => ApiError::conflict("last_active_admin_required"),
        "admin_risk_acknowledgement_required" => {
            ApiError::bad_request("admin_risk_acknowledgement_required")
        }
        "privilege_assertion_required" => ApiError::forbidden("privilege_assertion_required"),
        "privilege_verification_failed" => ApiError::forbidden("privilege_verification_failed"),
        _ => ApiError::internal(
            "operator_management_failed",
            "The operator account change could not be completed.",
            anyhow::anyhow!("unexpected operator mutation outcome: {code}"),
        ),
    }
}

fn validate_operator_batch(
    confirmed: bool,
    items: &[BulkOperatorMutationItem],
) -> Result<(), ApiError> {
    require_confirmed(confirmed)?;
    if items.is_empty() || items.len() > GATEWAY_CONTROL_BATCH_MAX_ITEMS {
        return Err(ApiError::bad_request("operator_batch_targets_invalid"));
    }
    let mut unique = HashSet::with_capacity(items.len());
    if items.iter().any(|item| !unique.insert(item.operator_id)) {
        return Err(ApiError::bad_request("operator_batch_targets_duplicate"));
    }
    Ok(())
}

fn prepare_operator_privilege_verification(
    target_id: Uuid,
    action: &str,
    status: Option<&str>,
    admin_risk_acknowledged: bool,
    assertion: PrivilegeAssertion,
) -> Result<(Uuid, GatewayPrivilegeVerificationBatchItem), ApiError> {
    let target = target_id.to_string();
    let payload_hash = operator_db_payload_hash(OperatorDbPayloadInput {
        action,
        target: &target,
        username: None,
        role: None,
        scopes: &[],
        session_refresh_ttl_secs: None,
        status,
        admin_risk_acknowledged,
    })
    .map_err(|error| {
        ApiError::internal(
            "operator_privilege_intent_failed",
            "The operator privilege request could not be prepared.",
            anyhow::Error::from(error),
        )
    })?;
    let targets = vec![target.clone()];
    let intent = DbPrivilegeIntent::new(action, &target, None, &targets, true, Some(&payload_hash));
    let intent = serde_json::to_string(&intent).map_err(|error| {
        ApiError::internal(
            "operator_privilege_intent_failed",
            "The operator privilege request could not be prepared.",
            error.into(),
        )
    })?;
    Ok((
        target_id,
        GatewayPrivilegeVerificationBatchItem {
            request_id: target,
            verification: GatewayPrivilegeVerification { intent, assertion },
        },
    ))
}

async fn verify_prepared_operator_privileges(
    state: &AppState,
    prepared: Vec<(Uuid, GatewayPrivilegeVerificationBatchItem)>,
) -> Result<(Vec<Uuid>, Vec<Uuid>), ApiError> {
    if prepared.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    if !state.gateway.privilege_configured() {
        return Err(ApiError::conflict("gateway_control_url_missing"));
    }
    let expected = prepared
        .iter()
        .map(|(target_id, item)| (*target_id, item.request_id.clone()))
        .collect::<Vec<_>>();
    state.refresh_gateway_dispatch_timeouts();
    let result = state
        .gateway
        .verify_privileges(prepared.into_iter().map(|(_, item)| item).collect())
        .await
        .map_err(operator_privilege_batch_error)?;
    if result.results.len() != expected.len()
        || result
            .results
            .iter()
            .zip(&expected)
            .any(|(result, (_, request_id))| &result.request_id != request_id)
    {
        return Err(ApiError::internal(
            "privilege_verification_result_invalid",
            "The gateway returned an invalid operator privilege result set.",
            anyhow::anyhow!("bulk operator privilege results did not preserve request order"),
        ));
    }
    let mut approved = Vec::with_capacity(expected.len());
    let mut rejected = Vec::new();
    for (result, (target_id, _)) in result.results.into_iter().zip(expected) {
        if result.approved {
            approved.push(target_id);
        } else {
            rejected.push(target_id);
        }
    }
    Ok((approved, rejected))
}

fn operator_privilege_batch_error(error: anyhow::Error) -> ApiError {
    if error.to_string().contains("ReplayProtectionSaturated") {
        ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "privilege_replay_protection_saturated",
            error,
            public_message: Some(
                "Privilege verification is temporarily saturated; wait for an assertion to expire and review request volume before retrying."
                    .to_string(),
            ),
        }
    } else if error
        .downcast_ref::<GatewayControlResponseError>()
        .is_some_and(|response| matches!(response.status_code, 403 | 409))
    {
        ApiError::forbidden("privilege_verification_failed")
    } else {
        ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "privilege_verification_unavailable",
            error,
            public_message: Some(
                "The gateway could not verify privilege material; the action remains locked."
                    .to_string(),
            ),
        }
    }
}

async fn verify_operator_privilege_batch(
    state: &AppState,
    prepared: Vec<(Uuid, GatewayPrivilegeVerificationBatchItem)>,
    outcomes: &mut HashMap<Uuid, BulkOperatorMutationOutcome>,
) -> Result<Vec<Uuid>, ApiError> {
    let (approved, rejected) = verify_prepared_operator_privileges(state, prepared).await?;
    for operator_id in rejected {
        outcomes.insert(
            operator_id,
            rejected_operator_outcome(operator_id, "privilege_verification_failed"),
        );
    }
    Ok(approved)
}

async fn verify_operator_session_privilege_batch(
    state: &AppState,
    prepared: Vec<(Uuid, GatewayPrivilegeVerificationBatchItem)>,
    outcomes: &mut HashMap<Uuid, BulkOperatorSessionRevokeOutcome>,
) -> Result<Vec<Uuid>, ApiError> {
    let (approved, rejected) = verify_prepared_operator_privileges(state, prepared).await?;
    for session_id in rejected {
        outcomes.insert(
            session_id,
            rejected_operator_session_outcome(session_id, "privilege_verification_failed"),
        );
    }
    Ok(approved)
}

fn merge_operator_repository_outcomes(
    approved: &[Uuid],
    repository_outcomes: Vec<AccessBatchMutationOutcome<OperatorView>>,
    outcomes: &mut HashMap<Uuid, BulkOperatorMutationOutcome>,
) -> Result<(), ApiError> {
    if repository_outcomes.len() != approved.len()
        || repository_outcomes
            .iter()
            .zip(approved)
            .any(|(outcome, expected)| access_outcome_target(outcome) != *expected)
    {
        return Err(ApiError::internal(
            "operator_mutation_result_invalid",
            "The operator account change returned an invalid result set.",
            anyhow::anyhow!("operator repository outcomes did not preserve approved order"),
        ));
    }
    for outcome in repository_outcomes {
        match outcome {
            AccessBatchMutationOutcome::Applied { target_id, result } => {
                outcomes.insert(
                    target_id,
                    BulkOperatorMutationOutcome {
                        operator_id: target_id,
                        status: "succeeded".to_string(),
                        result: Some(result),
                        error_code: None,
                        error_message: None,
                    },
                );
            }
            AccessBatchMutationOutcome::Rejected { target_id, code } => {
                outcomes.insert(target_id, rejected_operator_outcome(target_id, code));
            }
        }
    }
    Ok(())
}

fn merge_operator_session_repository_outcomes(
    approved: &[Uuid],
    repository_outcomes: Vec<AccessBatchMutationOutcome<OperatorSessionView>>,
    outcomes: &mut HashMap<Uuid, BulkOperatorSessionRevokeOutcome>,
) -> Result<(), ApiError> {
    if repository_outcomes.len() != approved.len()
        || repository_outcomes
            .iter()
            .zip(approved)
            .any(|(outcome, expected)| access_outcome_target(outcome) != *expected)
    {
        return Err(ApiError::internal(
            "operator_session_mutation_result_invalid",
            "The operator session change returned an invalid result set.",
            anyhow::anyhow!("operator session repository outcomes did not preserve approved order"),
        ));
    }
    for outcome in repository_outcomes {
        match outcome {
            AccessBatchMutationOutcome::Applied { target_id, result } => {
                outcomes.insert(
                    target_id,
                    BulkOperatorSessionRevokeOutcome {
                        session_id: target_id,
                        status: "succeeded".to_string(),
                        result: Some(result),
                        error_code: None,
                        error_message: None,
                    },
                );
            }
            AccessBatchMutationOutcome::Rejected { target_id, code } => {
                outcomes.insert(
                    target_id,
                    rejected_operator_session_outcome(target_id, code),
                );
            }
        }
    }
    Ok(())
}

fn access_outcome_target<T>(outcome: &AccessBatchMutationOutcome<T>) -> Uuid {
    match outcome {
        AccessBatchMutationOutcome::Applied { target_id, .. }
        | AccessBatchMutationOutcome::Rejected { target_id, .. } => *target_id,
    }
}

fn ordered_operator_response(
    operator_ids: &[Uuid],
    mut outcomes: HashMap<Uuid, BulkOperatorMutationOutcome>,
) -> Result<BulkOperatorMutationResponse, ApiError> {
    let ordered = operator_ids
        .iter()
        .map(|operator_id| {
            outcomes.remove(operator_id).ok_or_else(|| {
                ApiError::internal(
                    "operator_mutation_result_invalid",
                    "The operator account change returned an incomplete result set.",
                    anyhow::anyhow!("operator outcome missing for {operator_id}"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BulkOperatorMutationResponse { outcomes: ordered })
}

fn ordered_operator_session_response(
    session_ids: &[Uuid],
    mut outcomes: HashMap<Uuid, BulkOperatorSessionRevokeOutcome>,
) -> Result<BulkOperatorSessionRevokeResponse, ApiError> {
    let ordered = session_ids
        .iter()
        .map(|session_id| {
            outcomes.remove(session_id).ok_or_else(|| {
                ApiError::internal(
                    "operator_session_mutation_result_invalid",
                    "The operator session change returned an incomplete result set.",
                    anyhow::anyhow!("operator session outcome missing for {session_id}"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BulkOperatorSessionRevokeResponse { outcomes: ordered })
}

fn rejected_operator_outcome(operator_id: Uuid, code: &str) -> BulkOperatorMutationOutcome {
    BulkOperatorMutationOutcome {
        operator_id,
        status: "rejected".to_string(),
        result: None,
        error_code: Some(code.to_string()),
        error_message: Some(operator_mutation_error_message(code).to_string()),
    }
}

fn rejected_operator_session_outcome(
    session_id: Uuid,
    code: &str,
) -> BulkOperatorSessionRevokeOutcome {
    BulkOperatorSessionRevokeOutcome {
        session_id,
        status: "rejected".to_string(),
        result: None,
        error_code: Some(code.to_string()),
        error_message: Some(operator_mutation_error_message(code).to_string()),
    }
}

fn operator_mutation_error_message(code: &str) -> &'static str {
    match code {
        "operator_not_found" => "The operator account is unavailable for this action.",
        "operator_session_not_found" => "The operator session was not found.",
        "last_active_admin_required" => "At least one active administrator must remain.",
        "admin_risk_acknowledgement_required" => "Administrator risk acknowledgement is required.",
        "privilege_assertion_required" => "A privilege assertion is required.",
        "privilege_verification_failed" => "The privilege assertion was rejected.",
        _ => "The requested access change could not be completed.",
    }
}

pub(crate) async fn reset_operator_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(operator_id): Path<Uuid>,
    Json(request): Json<OperatorPasswordResetRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    let actor = state.require_operator_role(&headers, "admin").await?;
    require_confirmed(request.confirmed)?;
    validate_operator_credentials("operator", &request.password)?;
    let target = state
        .repo
        .operator_by_id(operator_id)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_unavailable",
            "The operator account could not be loaded.",
        ))?
        .filter(|operator| operator.status != "deleted")
        .ok_or_else(|| ApiError::not_found("operator_not_found"))?;
    require_admin_risk_if_needed(&target.role, None, request.admin_risk_acknowledged)?;
    let target = operator_id.to_string();
    verify_operator_management_privilege(
        &state,
        "operator.password_reset",
        &target,
        None,
        None,
        &[],
        None,
        None,
        request.admin_risk_acknowledged,
        request.privilege_assertion.clone(),
    )
    .await?;
    state
        .repo
        .reset_operator_password(operator_id, &request.password, &actor)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_password_reset_failed",
            "The operator password could not be reset.",
        ))?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("operator_not_found"))
}

pub(crate) async fn clear_operator_totp(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(operator_id): Path<Uuid>,
    Json(request): Json<OperatorLifecycleRequest>,
) -> Result<Json<OperatorView>, ApiError> {
    let actor = state.require_operator_role(&headers, "admin").await?;
    let response = mutate_operator_totp_clears(
        &state,
        &actor,
        BulkOperatorTotpClearRequest {
            items: vec![BulkOperatorMutationItem {
                operator_id,
                privilege_assertion: request.privilege_assertion,
            }],
            confirmed: request.confirmed,
            admin_risk_acknowledged: request.admin_risk_acknowledged,
        },
    )
    .await?;
    singleton_operator_mutation(response)
}

pub(crate) async fn bulk_clear_operator_totps(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkOperatorTotpClearRequest>,
) -> Result<Json<BulkOperatorMutationResponse>, ApiError> {
    let actor = state.require_operator_role(&headers, "admin").await?;
    Ok(Json(
        mutate_operator_totp_clears(&state, &actor, request).await?,
    ))
}

async fn mutate_operator_totp_clears(
    state: &AppState,
    actor: &AuthContext,
    request: BulkOperatorTotpClearRequest,
) -> Result<BulkOperatorMutationResponse, ApiError> {
    validate_operator_batch(request.confirmed, &request.items)?;
    let operator_ids = request
        .items
        .iter()
        .map(|item| item.operator_id)
        .collect::<Vec<_>>();
    let snapshots = state
        .repo
        .operator_batch_authority_snapshots(&operator_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_unavailable",
            "The operator accounts could not be loaded.",
        ))?;
    let snapshots = snapshots
        .into_iter()
        .map(|snapshot| (snapshot.operator_id, snapshot))
        .collect::<HashMap<_, _>>();
    let mut outcomes = HashMap::new();
    let mut prepared = Vec::new();
    for item in &request.items {
        let Some(snapshot) = snapshots
            .get(&item.operator_id)
            .filter(|snapshot| snapshot.status != "deleted")
        else {
            outcomes.insert(
                item.operator_id,
                rejected_operator_outcome(item.operator_id, "operator_not_found"),
            );
            continue;
        };
        if let Err(error) =
            require_admin_risk_if_needed(&snapshot.role, None, request.admin_risk_acknowledged)
        {
            outcomes.insert(
                item.operator_id,
                rejected_operator_outcome(item.operator_id, error.code),
            );
            continue;
        }
        let Some(assertion) = item.privilege_assertion.clone() else {
            outcomes.insert(
                item.operator_id,
                rejected_operator_outcome(item.operator_id, "privilege_assertion_required"),
            );
            continue;
        };
        prepared.push(prepare_operator_privilege_verification(
            item.operator_id,
            "operator.totp_clear",
            None,
            request.admin_risk_acknowledged,
            assertion,
        )?);
    }
    let approved = verify_operator_privilege_batch(state, prepared, &mut outcomes).await?;
    let repository_outcomes = if approved.is_empty() {
        Vec::new()
    } else {
        state
            .repo
            .clear_operator_totps(&approved, actor)
            .await
            .map_err(ApiError::internal_mapper(
                "operator_totp_clear_failed",
                "The operator authenticators could not be cleared.",
            ))?
    };
    merge_operator_repository_outcomes(&approved, repository_outcomes, &mut outcomes)?;
    ordered_operator_response(&operator_ids, outcomes)
}

pub(crate) async fn list_operator_auth_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<OperatorAuthEventQuery>,
) -> Result<Json<Vec<OperatorAuthEventView>>, ApiError> {
    let _operator = state.require_operator_role(&headers, "admin").await?;
    if let Some(result) = query.result.as_deref() {
        if !matches!(result.trim(), "success" | "failure" | "throttled") {
            return Err(ApiError::bad_request("invalid_operator_auth_event_result"));
        }
    }
    Ok(Json(
        state
            .repo
            .list_operator_auth_events(&query)
            .await
            .map_err(ApiError::internal_mapper(
                "operator_auth_events_unavailable",
                "The operator authentication events could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn list_operator_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<OperatorSessionView>>, ApiError> {
    let operator = state.require_operator_role(&headers, "admin").await?;
    let current_session_id = operator
        .audit_session_id()
        .ok_or_else(|| ApiError::unauthorized("invalid_operator_session"))?;
    Ok(Json(
        state
            .repo
            .list_operator_sessions(query.limit.unwrap_or(50), current_session_id)
            .await
            .map_err(ApiError::internal_mapper(
                "operator_sessions_unavailable",
                "The operator sessions could not be loaded.",
            ))?,
    ))
}

pub(crate) async fn revoke_operator_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<Uuid>,
    Json(request): Json<OperatorSessionRevokeRequest>,
) -> Result<Json<OperatorSessionView>, ApiError> {
    let operator = state.require_operator_role(&headers, "admin").await?;
    let response = mutate_operator_session_revocations(
        &state,
        &operator,
        BulkOperatorSessionRevokeRequest {
            items: vec![BulkOperatorSessionRevokeItem {
                session_id,
                privilege_assertion: request.privilege_assertion,
            }],
            confirmed: request.confirmed,
            admin_risk_acknowledged: request.admin_risk_acknowledged,
        },
    )
    .await?;
    singleton_operator_session_mutation(response)
}

pub(crate) async fn bulk_revoke_operator_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<BulkOperatorSessionRevokeRequest>,
) -> Result<Json<BulkOperatorSessionRevokeResponse>, ApiError> {
    let actor = state.require_operator_role(&headers, "admin").await?;
    Ok(Json(
        mutate_operator_session_revocations(&state, &actor, request).await?,
    ))
}

async fn mutate_operator_session_revocations(
    state: &AppState,
    actor: &AuthContext,
    request: BulkOperatorSessionRevokeRequest,
) -> Result<BulkOperatorSessionRevokeResponse, ApiError> {
    validate_operator_session_batch(request.confirmed, &request.items)?;
    actor
        .audit_session_id()
        .ok_or_else(|| ApiError::unauthorized("invalid_operator_session"))?;
    let session_ids = request
        .items
        .iter()
        .map(|item| item.session_id)
        .collect::<Vec<_>>();
    let snapshots = state
        .repo
        .operator_session_batch_authority_snapshots(&session_ids)
        .await
        .map_err(ApiError::internal_mapper(
            "operator_session_unavailable",
            "The operator sessions could not be loaded.",
        ))?;
    let snapshots = snapshots
        .into_iter()
        .map(|snapshot| (snapshot.session_id, snapshot))
        .collect::<HashMap<_, _>>();
    let mut outcomes = HashMap::new();
    let mut prepared = Vec::new();
    for item in &request.items {
        let Some(snapshot) = snapshots.get(&item.session_id) else {
            outcomes.insert(
                item.session_id,
                rejected_operator_session_outcome(item.session_id, "operator_session_not_found"),
            );
            continue;
        };
        if let Err(error) = require_admin_risk_if_needed(
            &snapshot.operator_role,
            None,
            request.admin_risk_acknowledged,
        ) {
            outcomes.insert(
                item.session_id,
                rejected_operator_session_outcome(item.session_id, error.code),
            );
            continue;
        }
        let Some(assertion) = item.privilege_assertion.clone() else {
            outcomes.insert(
                item.session_id,
                rejected_operator_session_outcome(item.session_id, "privilege_assertion_required"),
            );
            continue;
        };
        prepared.push(prepare_operator_privilege_verification(
            item.session_id,
            "operator_session.revoke",
            None,
            request.admin_risk_acknowledged,
            assertion,
        )?);
    }
    let approved = verify_operator_session_privilege_batch(state, prepared, &mut outcomes).await?;
    let repository_outcomes = if approved.is_empty() {
        Vec::new()
    } else {
        state
            .repo
            .revoke_operator_sessions(&approved, actor)
            .await
            .map_err(ApiError::internal_mapper(
                "operator_session_revoke_failed",
                "The operator sessions could not be revoked.",
            ))?
    };
    merge_operator_session_repository_outcomes(&approved, repository_outcomes, &mut outcomes)?;
    ordered_operator_session_response(&session_ids, outcomes)
}

fn singleton_operator_session_mutation(
    mut response: BulkOperatorSessionRevokeResponse,
) -> Result<Json<OperatorSessionView>, ApiError> {
    let outcome = response.outcomes.pop().ok_or_else(|| {
        ApiError::internal(
            "operator_session_mutation_result_invalid",
            "The operator session change returned no outcome.",
            anyhow::anyhow!("singleton operator session outcome missing"),
        )
    })?;
    if let Some(result) = outcome.result {
        Ok(Json(result))
    } else {
        Err(operator_session_outcome_error(
            outcome
                .error_code
                .as_deref()
                .unwrap_or("operator_session_revoke_failed"),
        ))
    }
}

fn operator_session_outcome_error(code: &str) -> ApiError {
    match code {
        "operator_session_not_found" => ApiError::not_found("operator_session_not_found"),
        "admin_risk_acknowledgement_required" => {
            ApiError::bad_request("admin_risk_acknowledgement_required")
        }
        "privilege_assertion_required" => ApiError::forbidden("privilege_assertion_required"),
        "privilege_verification_failed" => ApiError::forbidden("privilege_verification_failed"),
        _ => ApiError::internal(
            "operator_session_revoke_failed",
            "The operator session could not be revoked.",
            anyhow::anyhow!("unexpected operator session mutation outcome: {code}"),
        ),
    }
}

fn validate_operator_session_batch(
    confirmed: bool,
    items: &[BulkOperatorSessionRevokeItem],
) -> Result<(), ApiError> {
    require_confirmed(confirmed)?;
    if items.is_empty() || items.len() > GATEWAY_CONTROL_BATCH_MAX_ITEMS {
        return Err(ApiError::bad_request(
            "operator_session_batch_targets_invalid",
        ));
    }
    let mut unique = HashSet::with_capacity(items.len());
    if items.iter().any(|item| !unique.insert(item.session_id)) {
        return Err(ApiError::bad_request(
            "operator_session_batch_targets_duplicate",
        ));
    }
    Ok(())
}

fn validate_session_refresh_ttl(value: u64) -> Result<(), ApiError> {
    if (MIN_REFRESH_TOKEN_TTL_SECS..=MAX_REFRESH_TOKEN_TTL_SECS).contains(&value) {
        Ok(())
    } else {
        Err(ApiError::bad_request("invalid_session_refresh_ttl_secs"))
    }
}

fn require_confirmed(confirmed: bool) -> Result<(), ApiError> {
    if confirmed {
        Ok(())
    } else {
        Err(ApiError::bad_request("confirmation_required"))
    }
}

async fn verify_operator_management_privilege(
    state: &AppState,
    action: &str,
    target: &str,
    username: Option<&str>,
    role: Option<&str>,
    scopes: &[String],
    session_refresh_ttl_secs: Option<u64>,
    status: Option<&str>,
    admin_risk_acknowledged: bool,
    assertion: Option<PrivilegeAssertion>,
) -> Result<(), ApiError> {
    let normalized_scopes = normalized_requested_scopes(scopes);
    let payload_hash = operator_db_payload_hash(OperatorDbPayloadInput {
        action,
        target,
        username,
        role,
        scopes: &normalized_scopes,
        session_refresh_ttl_secs,
        status,
        admin_risk_acknowledged,
    })
    .map_err(|error| {
        ApiError::internal(
            "operator_privilege_intent_failed",
            "The operator privilege request could not be prepared.",
            anyhow::Error::from(error),
        )
    })?;
    let targets = vec![target.to_string()];
    let intent = DbPrivilegeIntent::new(action, target, None, &targets, true, Some(&payload_hash));
    verify_privilege_intent(state, &intent, assertion).await
}

fn normalized_requested_scopes(scopes: &[String]) -> Vec<String> {
    let mut scopes = scopes
        .iter()
        .map(|scope| scope.trim())
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    scopes.sort();
    scopes.dedup();
    scopes
}

fn operator_management_error(error: anyhow::Error) -> ApiError {
    if error.to_string().contains("last_active_admin_required") {
        ApiError::conflict("last_active_admin_required")
    } else {
        ApiError::internal(
            "operator_management_failed",
            "The operator account change could not be completed.",
            error,
        )
    }
}

fn require_admin_risk_if_needed(
    current_role: &str,
    requested_role: Option<&str>,
    admin_risk_acknowledged: bool,
) -> Result<(), ApiError> {
    let touches_admin =
        current_role.trim() == "admin" || requested_role.is_some_and(|role| role.trim() == "admin");
    if touches_admin && !admin_risk_acknowledged {
        Err(ApiError::bad_request("admin_risk_acknowledgement_required"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_batch_validators_enforce_the_shared_bound_and_unique_targets() {
        let one = BulkOperatorMutationItem {
            operator_id: Uuid::new_v4(),
            privilege_assertion: None,
        };
        assert!(validate_operator_batch(
            true,
            &[BulkOperatorMutationItem {
                operator_id: one.operator_id,
                privilege_assertion: None,
            }]
        )
        .is_ok());
        assert_eq!(
            validate_operator_batch(false, &[one])
                .expect_err("confirmation must be required")
                .code,
            "confirmation_required"
        );

        let duplicate_id = Uuid::new_v4();
        assert_eq!(
            validate_operator_batch(
                true,
                &[
                    BulkOperatorMutationItem {
                        operator_id: duplicate_id,
                        privilege_assertion: None,
                    },
                    BulkOperatorMutationItem {
                        operator_id: duplicate_id,
                        privilege_assertion: None,
                    },
                ],
            )
            .expect_err("duplicate operators must be rejected")
            .code,
            "operator_batch_targets_duplicate"
        );

        let oversized = (0..=GATEWAY_CONTROL_BATCH_MAX_ITEMS)
            .map(|_| BulkOperatorSessionRevokeItem {
                session_id: Uuid::new_v4(),
                privilege_assertion: None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            validate_operator_session_batch(true, &oversized)
                .expect_err("oversized session batches must be rejected")
                .code,
            "operator_session_batch_targets_invalid"
        );
    }

    #[test]
    fn access_batch_owner_has_one_gateway_verification_call_and_singletons_delegate() {
        let source = include_str!("routes_auth.rs");
        let production = source
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .expect("production route source");
        assert_eq!(production.matches(".verify_privileges(").count(), 1);
        assert!(production.contains("mutate_operator_statuses(\n        &state,"));
        assert!(production.contains("mutate_operator_totp_clears(\n        &state,"));
        assert!(production.contains("mutate_operator_session_revocations(\n        &state,"));
        assert!(
            !production.contains("for item in &request.items {\n        verify_privilege_intent")
        );
    }
}
