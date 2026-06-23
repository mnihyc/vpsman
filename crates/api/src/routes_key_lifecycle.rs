use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};

use crate::{
    error::ApiError,
    model::{
        AgentIdentityView, ClientKeyRevocationView, CreateClientKeyRevocationRequest, HistoryQuery,
        KeyLifecycleReportView, UpsertAgentIdentityRequest, WsEvent,
    },
    privilege::{verify_privilege_intent, DbPrivilegeIntent},
    state::AppState,
    util::limit_or_default,
};
use tracing::warn;

pub(crate) async fn upsert_agent_identity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpsertAgentIdentityRequest>,
) -> Result<(StatusCode, Json<AgentIdentityView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    validate_agent_identity_request(&request)?;
    let client_id = request
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("client_id_required"))?;
    let targets = vec![client_id.to_string()];
    let action = if request.replace_existing_key {
        "agent_identity.rotate"
    } else {
        "agent_identity.import"
    };
    let intent = DbPrivilegeIntent::new(action, client_id, None, &targets, true, None);
    verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    state
        .repo
        .preflight_agent_identity_upsert(&request)
        .await
        .map_err(agent_identity_mutation_error)?;
    let view = state
        .repo
        .upsert_agent_identity(&request, &operator)
        .await
        .map_err(agent_identity_mutation_error)?;
    if request.replace_existing_key {
        if let Err(error) = state
            .disconnect_gateway_session_for_lifecycle(client_id, "client_key_replaced")
            .await
        {
            warn!(
                ?error,
                client_id, "post-commit gateway disconnect failed after client key replacement"
            );
        }
    }
    state.publish(WsEvent::AgentUpdated {
        client_id: view.client_id.clone(),
        gateway_id: "identity".to_string(),
    });
    state.process_job_terminal_events(500).await?;
    Ok((StatusCode::CREATED, Json(view)))
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
    let targets = vec![client_id.clone()];
    let intent =
        DbPrivilegeIntent::new("client_key.revoke", &client_id, None, &targets, true, None);
    verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    let record = state
        .repo
        .revoke_current_client_key(&client_id, &request, &operator)
        .await?;
    if let Err(error) = state
        .disconnect_gateway_session_for_lifecycle(&client_id, "client_key_revoked")
        .await
    {
        warn!(
            ?error,
            client_id, "post-commit gateway disconnect failed after client key revocation"
        );
    }
    state.publish(WsEvent::AgentUpdated {
        client_id,
        gateway_id: "key_lifecycle".to_string(),
    });
    state.process_job_terminal_events(500).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

fn agent_identity_mutation_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("display_name_already_exists")
        || message.contains("clients_visible_display_name_key_idx")
    {
        ApiError::conflict("display_name_already_exists")
    } else if message.contains("agent_identity_deactivated") {
        ApiError::gone("agent_identity_deactivated")
    } else if message.contains("client_not_found_or_no_key") {
        ApiError::not_found("client_not_found_or_no_key")
    } else if message.contains("client_id_already_registered") {
        ApiError::conflict("client_id_already_registered")
    } else if message.contains("agent_identity_key_revoked") {
        ApiError::conflict("agent_identity_key_revoked")
    } else {
        error.into()
    }
}

pub(crate) async fn key_lifecycle_report(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<KeyLifecycleReportView>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    Ok(Json(state.repo.key_lifecycle_report().await?))
}

fn validate_agent_identity_request(request: &UpsertAgentIdentityRequest) -> Result<(), ApiError> {
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "agent_identity_confirmation_required",
        ));
    }
    let Some(client_id) = request.client_id.as_deref() else {
        return Err(ApiError::bad_request("client_id_required"));
    };
    validate_client_id(client_id)?;
    validate_fixed_hex32(&request.client_public_key_hex, "client_public_key_hex")?;
    if request.replace_existing_key {
        if request.display_name.is_some() {
            return Err(ApiError::bad_request(
                "display_name_not_allowed_during_key_rotation",
            ));
        }
        if !request.tags.is_empty() {
            return Err(ApiError::bad_request(
                "tags_not_allowed_during_key_rotation",
            ));
        }
    } else {
        if let Some(display_name) = request.display_name.as_deref() {
            validate_optional_display_name(display_name)?;
        }
        validate_tags(&request.tags)?;
    }
    Ok(())
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

pub(crate) fn validate_client_id(client_id: &str) -> Result<(), ApiError> {
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
        if tag.starts_with("id:") || tag.starts_with("name:") {
            return Err(ApiError::bad_request("reserved_inner_tag_selector"));
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

fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut normalized = tags
        .iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn validate_optional_display_name(value: &str) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 160
        || value.chars().any(|character| character.is_control())
    {
        return Err(ApiError::bad_request("display_name_invalid"));
    }
    Ok(())
}

fn validate_fixed_hex32(value: &str, _field: &'static str) -> Result<(), ApiError> {
    if value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        Ok(())
    } else {
        Err(ApiError::bad_request("client_public_key_invalid"))
    }
}
