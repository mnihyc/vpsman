use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    http::StatusCode,
    Json,
};
use std::net::IpAddr;

use crate::{
    error::ApiError,
    model::{
        ClaimEnrollmentRequest, ClaimEnrollmentResponse, ClientKeyRevocationView,
        CreateClientKeyRevocationRequest, CreateEnrollmentTokenRequest,
        CreateEnrollmentTokenResponse, EnrollmentTokenView, HistoryQuery, KeyLifecycleReportView,
        WsEvent,
    },
    repository_enrollment::{
        normalize_enrollment_purpose, normalize_optional_client_id, normalize_tags,
        EnrollmentClaimContext, EnrollmentClaimOutcome, ENROLLMENT_PURPOSE_PROVISION,
        ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT,
    },
    repository_key_lifecycle::KeyLifecycleTrustReport,
    state::AppState,
    util::limit_or_default,
};

pub(crate) async fn list_enrollment_tokens(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<EnrollmentTokenView>>, ApiError> {
    let _operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    Ok(Json(state.repo.list_enrollment_tokens().await?))
}

pub(crate) async fn create_enrollment_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateEnrollmentTokenRequest>,
) -> Result<(StatusCode, Json<CreateEnrollmentTokenResponse>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_create_enrollment_token(&request)?;
    validate_enrollment_token_policy(&state, &request).await?;
    Ok((
        StatusCode::CREATED,
        Json(
            state
                .repo
                .create_enrollment_token_with_update(&request, &operator, &state.enrollment.update)
                .await?,
        ),
    ))
}

pub(crate) async fn list_client_key_revocations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<ClientKeyRevocationView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .list_client_key_revocations(limit_or_default(query.limit))
            .await?,
    ))
}

pub(crate) async fn revoke_current_client_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Json(request): Json<CreateClientKeyRevocationRequest>,
) -> Result<(StatusCode, Json<ClientKeyRevocationView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_client_id(&client_id)?;
    validate_client_key_revocation(&request)?;
    if state
        .repo
        .client_public_key_sha256_hex(&client_id)
        .await?
        .is_none()
    {
        return Err(ApiError::not_found("client_public_key_not_found"));
    }
    let record = state
        .repo
        .revoke_current_client_key(&client_id, &request, &operator)
        .await?;
    state.publish(WsEvent::AgentUpdated {
        client_id,
        gateway_id: "key_lifecycle".to_string(),
    });
    Ok((StatusCode::CREATED, Json(record)))
}

pub(crate) async fn key_lifecycle_report(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<KeyLifecycleReportView>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(
        state
            .repo
            .key_lifecycle_report(KeyLifecycleTrustReport {
                server_ed25519_public_key_configured: state
                    .enrollment
                    .server_ed25519_public_key_hex
                    .is_some(),
                discovery_trusted_server_key_count: state
                    .enrollment
                    .discovery_trusted_server_ed25519_public_keys_hex
                    .len(),
                gateway_server_public_key_configured: state
                    .enrollment
                    .gateway_server_public_key_hex
                    .is_some(),
            })
            .await?,
    ))
}

pub(crate) async fn claim_enrollment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ClaimEnrollmentRequest>,
) -> Result<(StatusCode, Json<ClaimEnrollmentResponse>), ApiError> {
    validate_claim_enrollment(&request)?;
    let mut enrollment = state.enrollment.clone();
    if let Some(country_tag) = country_tag_from_headers(&headers) {
        enrollment.default_country_tag = Some(country_tag);
    }
    let context = EnrollmentClaimContext {
        fallback_display_name: default_alias_from_headers(&headers)
            .unwrap_or_else(|| request.client_id.clone()),
    };
    match state
        .repo
        .claim_enrollment_with_context(&enrollment, &context, &request)
        .await?
    {
        EnrollmentClaimOutcome::Accepted(response) => {
            state.publish(WsEvent::AgentUpdated {
                client_id: response.client_id.clone(),
                gateway_id: "enrollment".to_string(),
            });
            Ok((StatusCode::CREATED, Json(*response)))
        }
        EnrollmentClaimOutcome::InvalidToken => {
            Err(ApiError::unauthorized("invalid_enrollment_token"))
        }
        EnrollmentClaimOutcome::ExpiredToken => {
            Err(ApiError::unauthorized("expired_enrollment_token"))
        }
        EnrollmentClaimOutcome::UsedToken => {
            Err(ApiError::conflict("enrollment_token_already_used"))
        }
        EnrollmentClaimOutcome::TokenClientMismatch => {
            Err(ApiError::conflict("enrollment_token_client_mismatch"))
        }
        EnrollmentClaimOutcome::ExistingClientRequiresReenrollmentToken => Err(ApiError::conflict(
            "existing_client_requires_reenrollment_token",
        )),
        EnrollmentClaimOutcome::ReenrollmentClientMissing => {
            Err(ApiError::conflict("reenrollment_client_missing"))
        }
        EnrollmentClaimOutcome::ReenrollmentClientKeyChanged => {
            Err(ApiError::conflict("reenrollment_client_key_changed"))
        }
    }
}

fn country_tag_from_headers(headers: &HeaderMap) -> Option<String> {
    let country = headers
        .get("cf-ipcountry")
        .or_else(|| headers.get("x-vercel-ip-country"))
        .or_else(|| headers.get("x-country-code"))?
        .to_str()
        .ok()?
        .trim();
    if country.len() < 2 || country.len() > 32 {
        return None;
    }
    if !country
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
    {
        return None;
    }
    Some(format!("country:{}", country.to_ascii_uppercase()))
}

fn default_alias_from_headers(headers: &HeaderMap) -> Option<String> {
    let ip = headers
        .get("cf-connecting-ip")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|value| value.to_str().ok())
        .and_then(first_valid_ip)
        .or_else(|| {
            headers
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').find_map(first_valid_ip))
        })?;
    Some(ip.to_string())
}

fn first_valid_ip(value: &str) -> Option<IpAddr> {
    value.trim().parse::<IpAddr>().ok()
}

fn validate_create_enrollment_token(
    request: &CreateEnrollmentTokenRequest,
) -> Result<(), ApiError> {
    if request
        .ttl_secs
        .is_some_and(|ttl_secs| !(60..=86_400).contains(&ttl_secs))
    {
        return Err(ApiError::bad_request("enrollment_ttl_out_of_range"));
    }
    let purpose = request
        .purpose
        .as_deref()
        .unwrap_or(ENROLLMENT_PURPOSE_PROVISION)
        .trim();
    if !matches!(
        purpose,
        ENROLLMENT_PURPOSE_PROVISION | ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT
    ) {
        return Err(ApiError::bad_request("enrollment_purpose_invalid"));
    }
    if let Some(client_id) = normalize_optional_client_id(request.allowed_client_id.as_deref()) {
        validate_client_id(&client_id)?;
    }
    validate_tags(&request.default_tags)?;
    if let Some(default_display_name) = request.default_display_name.as_deref() {
        validate_optional_display_name(default_display_name)?;
    }
    if let Some(default_pool_name) = request.default_pool_name.as_deref() {
        validate_pool_name(default_pool_name)?;
    }
    if let Some(version_url) = request.unmanaged_update_version_url.as_deref() {
        validate_update_version_url(version_url)?;
    }
    if request
        .unmanaged_update_interval_secs
        .is_some_and(|value| !(300..=604_800).contains(&value))
    {
        return Err(ApiError::bad_request(
            "unmanaged_update_interval_secs_out_of_range",
        ));
    }
    if request
        .unmanaged_update_jitter_secs
        .is_some_and(|value| !(0..=604_800).contains(&value))
    {
        return Err(ApiError::bad_request(
            "unmanaged_update_jitter_secs_out_of_range",
        ));
    }
    Ok(())
}

async fn validate_enrollment_token_policy(
    state: &AppState,
    request: &CreateEnrollmentTokenRequest,
) -> Result<(), ApiError> {
    let purpose = normalize_enrollment_purpose(request.purpose.as_deref());
    let allowed_client_id = normalize_optional_client_id(request.allowed_client_id.as_deref());
    if let Some(default_pool_name) = request.default_pool_name.as_deref() {
        state.repo.pool_by_name(default_pool_name.trim()).await?;
    }
    if purpose == ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT {
        let Some(client_id) = allowed_client_id.as_deref() else {
            return Err(ApiError::bad_request("reenrollment_client_id_required"));
        };
        if !request.confirmed_reenrollment {
            return Err(ApiError::bad_request("reenrollment_confirmation_required"));
        }
        if state
            .repo
            .client_public_key_sha256_hex(client_id)
            .await?
            .is_none()
        {
            return Err(ApiError::not_found("reenrollment_client_not_found"));
        }
    }
    Ok(())
}

fn validate_claim_enrollment(request: &ClaimEnrollmentRequest) -> Result<(), ApiError> {
    if request.token.trim().is_empty() {
        return Err(ApiError::bad_request("enrollment_token_required"));
    }
    validate_client_id(&request.client_id)?;
    if request.client_public_key_hex.len() != 64
        || !request
            .client_public_key_hex
            .as_bytes()
            .iter()
            .all(u8::is_ascii_hexdigit)
    {
        return Err(ApiError::bad_request("client_public_key_invalid"));
    }
    Ok(())
}

fn validate_optional_display_name(value: &str) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 160
        || value.chars().any(|character| character.is_control())
    {
        return Err(ApiError::bad_request("default_display_name_invalid"));
    }
    Ok(())
}

fn validate_pool_name(value: &str) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| character.is_control())
    {
        return Err(ApiError::bad_request("default_pool_name_invalid"));
    }
    Ok(())
}

fn validate_update_version_url(value: &str) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 2048 || value.as_bytes().contains(&0) {
        return Err(ApiError::bad_request(
            "unmanaged_update_version_url_invalid",
        ));
    }
    if value.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = value.strip_prefix("http://") {
        if is_local_http_authority(rest) {
            return Ok(());
        }
        return Err(ApiError::bad_request(
            "unmanaged_update_version_url_http_must_be_localhost",
        ));
    }
    if let Some(path) = value.strip_prefix("file://") {
        if path.starts_with('/') {
            return Ok(());
        }
        return Err(ApiError::bad_request(
            "unmanaged_update_version_url_file_must_be_absolute",
        ));
    }
    Err(ApiError::bad_request(
        "unmanaged_update_version_url_must_be_https",
    ))
}

fn is_local_http_authority(rest: &str) -> bool {
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host = if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, _suffix)) = rest.split_once(']') else {
            return false;
        };
        host
    } else {
        match authority.rsplit_once(':') {
            Some((host, _port)) if !host.contains(':') => host,
            _ => authority,
        }
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn validate_client_key_revocation(
    request: &CreateClientKeyRevocationRequest,
) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "client_key_revocation_confirmation_required",
        ));
    }
    if request
        .reason
        .as_deref()
        .is_some_and(|reason| reason.trim().len() > 1024)
    {
        return Err(ApiError::bad_request(
            "client_key_revocation_reason_too_long",
        ));
    }
    Ok(())
}

fn validate_client_id(client_id: &str) -> Result<(), ApiError> {
    if client_id.trim().is_empty() || client_id.len() > 120 {
        return Err(ApiError::bad_request("client_id_invalid"));
    }
    if !client_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(ApiError::bad_request("client_id_invalid"));
    }
    Ok(())
}

fn validate_tags(tags: &[String]) -> Result<(), ApiError> {
    if tags.len() > 32 {
        return Err(ApiError::bad_request("too_many_tags"));
    }
    for tag in normalize_tags(tags) {
        if tag.len() > 64 {
            return Err(ApiError::bad_request("tag_too_long"));
        }
        if !tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
        {
            return Err(ApiError::bad_request("tag_invalid"));
        }
    }
    Ok(())
}
