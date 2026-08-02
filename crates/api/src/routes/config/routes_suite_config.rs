use std::{collections::BTreeSet, fs};

use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    payload_hash, redact_suite_config_value, write_private_file_atomically, PrivilegeAssertion,
    SuiteConfig, SuiteConfigValidation,
};

use crate::{
    error::ApiError,
    privilege::{verify_privilege_intent, DbPrivilegeIntent},
    state::AppState,
};

#[derive(Debug, Serialize)]
pub(crate) struct SuiteConfigResponse {
    pub(crate) path: String,
    pub(crate) exists: bool,
    pub(crate) effective_require_registered_agent_updates: bool,
    pub(crate) toml: String,
    pub(crate) redacted: Value,
    pub(crate) validation: SuiteConfigValidation,
    pub(crate) hot_reload_note: String,
    pub(crate) restart_required_note: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateSuiteConfigRequest {
    pub(crate) toml: String,
    #[serde(default)]
    pub(crate) confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidateSuiteConfigRequest {
    pub(crate) toml: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ValidateSuiteConfigResponse {
    pub(crate) path: String,
    pub(crate) exists: bool,
    pub(crate) changed_keys: Vec<String>,
    pub(crate) redacted: Value,
    pub(crate) old_redacted: Value,
    pub(crate) validation: SuiteConfigValidation,
}

#[derive(Debug, Serialize)]
pub(crate) struct UpdateSuiteConfigResponse {
    pub(crate) path: String,
    pub(crate) changed_keys: Vec<String>,
    pub(crate) validation: SuiteConfigValidation,
    pub(crate) audit_status: String,
}

pub(crate) async fn get_suite_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SuiteConfigResponse>, ApiError> {
    let _operator = state.require_operator_role(&headers, "admin").await?;
    let (exists, text) = read_suite_config_text(&state)?;
    let config =
        SuiteConfig::parse(&text).map_err(|_| ApiError::bad_request("suite_config_invalid"))?;
    Ok(Json(SuiteConfigResponse {
        path: state.suite_config_path.display().to_string(),
        exists,
        effective_require_registered_agent_updates: state.require_registered_agent_updates(),
        redacted: redacted_toml_json(&text)?,
        toml: text,
        validation: config.validation_summary(),
        hot_reload_note: "API dispatcher limits, gateway-control read timeout, alert thresholds, job-output artifact threshold, update-registration enforcement, gateway runtime timing, and worker tick/schedule/notification/webhook/retention controls are applied by running services after this file changes.".to_string(),
        restart_required_note: "Bind addresses, gateway/API URLs and identities, database URL/migration path/pool sizes, secret refs, object-store clients and local object directories, worker identity/once mode, and connect/write timeout changes require service restart.".to_string(),
    }))
}

pub(crate) async fn update_suite_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateSuiteConfigRequest>,
) -> Result<Json<UpdateSuiteConfigResponse>, ApiError> {
    let operator = state.require_operator_role(&headers, "admin").await?;
    if !request.confirmed {
        return Err(ApiError::conflict(
            "suite_config_update_requires_confirmation",
        ));
    }
    if request.toml.len() > 256 * 1024 {
        return Err(ApiError::bad_request("suite_config_too_large"));
    }
    let parsed = SuiteConfig::parse(&request.toml)
        .map_err(|_| ApiError::bad_request("suite_config_invalid"))?;
    let toml_payload_hash = payload_hash(request.toml.as_bytes());
    verify_privilege_intent(
        &state,
        &DbPrivilegeIntent::new(
            "suite_config.update",
            "suite_config",
            None,
            &[],
            true,
            Some(&toml_payload_hash),
        ),
        request.privilege_assertion.clone(),
    )
    .await?;
    let (_exists, old_text) = read_suite_config_text(&state)?;
    let old_raw = toml_json(&old_text)?;
    let new_raw = toml_json(&request.toml)?;
    let changed_keys = changed_json_paths(&old_raw, &new_raw);
    let old_redacted = redact_suite_config_value(old_raw);
    let new_redacted = redact_suite_config_value(new_raw);
    let request_id = Uuid::new_v4();
    state
        .repo
        .record_suite_config_update_requested(
            &operator,
            &state.suite_config_path.display().to_string(),
            &changed_keys,
            old_redacted.clone(),
            new_redacted.clone(),
            request_id,
            &toml_payload_hash,
        )
        .await?;
    if let Err(write_error) = write_suite_config_atomically(&state, &request.toml) {
        if let Err(audit_error) = state
            .repo
            .record_suite_config_update_failed(
                &operator,
                &state.suite_config_path.display().to_string(),
                &changed_keys,
                old_redacted.clone(),
                new_redacted.clone(),
                request_id,
                &toml_payload_hash,
                write_error.code,
            )
            .await
        {
            warn!(
                error = %audit_error,
                request_id = %request_id,
                "failed to record suite config write failure"
            );
        }
        return Err(write_error);
    }
    let audit_status = match state
        .repo
        .record_suite_config_updated(
            &operator,
            &state.suite_config_path.display().to_string(),
            &changed_keys,
            old_redacted,
            new_redacted,
            request_id,
            &toml_payload_hash,
        )
        .await
    {
        Ok(()) => "applied_recorded".to_string(),
        Err(error) => {
            warn!(
                error = %error,
                request_id = %request_id,
                "failed to record suite config applied audit after successful write"
            );
            "intent_recorded_applied_audit_failed".to_string()
        }
    };
    Ok(Json(UpdateSuiteConfigResponse {
        path: state.suite_config_path.display().to_string(),
        changed_keys,
        validation: parsed.validation_summary(),
        audit_status,
    }))
}

pub(crate) async fn validate_suite_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ValidateSuiteConfigRequest>,
) -> Result<Json<ValidateSuiteConfigResponse>, ApiError> {
    let _operator = state.require_operator_role(&headers, "admin").await?;
    if request.toml.len() > 256 * 1024 {
        return Err(ApiError::bad_request("suite_config_too_large"));
    }
    let parsed = SuiteConfig::parse(&request.toml)
        .map_err(|_| ApiError::bad_request("suite_config_invalid"))?;
    let (exists, old_text) = read_suite_config_text(&state)?;
    let old_raw = toml_json(&old_text)?;
    let new_raw = toml_json(&request.toml)?;
    let changed_keys = changed_json_paths(&old_raw, &new_raw);
    let old_redacted = redact_suite_config_value(old_raw);
    let redacted = redact_suite_config_value(new_raw);
    Ok(Json(ValidateSuiteConfigResponse {
        path: state.suite_config_path.display().to_string(),
        exists,
        changed_keys,
        old_redacted,
        redacted,
        validation: parsed.validation_summary(),
    }))
}

fn read_suite_config_text(state: &AppState) -> Result<(bool, String), ApiError> {
    if !state.suite_config_path.exists() {
        return Ok((false, "version = 1\n".to_string()));
    }
    fs::read_to_string(&state.suite_config_path)
        .map(|text| (true, text))
        .map_err(|_| ApiError::conflict("suite_config_read_failed"))
}

fn write_suite_config_atomically(state: &AppState, text: &str) -> Result<(), ApiError> {
    write_private_file_atomically(&state.suite_config_path, text.as_bytes())
        .map_err(|_| ApiError::conflict("suite_config_write_failed"))
}

fn redacted_toml_json(text: &str) -> Result<Value, ApiError> {
    Ok(redact_suite_config_value(toml_json(text)?))
}

fn toml_json(text: &str) -> Result<Value, ApiError> {
    let value = toml::from_str::<toml::Value>(text)
        .map_err(|_| ApiError::bad_request("suite_config_invalid_toml"))?;
    serde_json::to_value(value).map_err(|error| ApiError::from(anyhow::anyhow!(error)))
}

fn changed_json_paths(old: &Value, new: &Value) -> Vec<String> {
    let mut changed = BTreeSet::new();
    collect_changed_paths("", old, new, &mut changed);
    changed.into_iter().collect()
}

fn collect_changed_paths(prefix: &str, old: &Value, new: &Value, changed: &mut BTreeSet<String>) {
    match (old, new) {
        (Value::Object(left), Value::Object(right)) => {
            let keys = left.keys().chain(right.keys()).collect::<BTreeSet<_>>();
            for key in keys {
                let path = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => collect_changed_paths(&path, left, right, changed),
                    (Some(value), None) | (None, Some(value)) => {
                        collect_leaf_paths(&path, value, changed)
                    }
                    (None, None) => {}
                }
            }
        }
        _ if !json_values_semantically_equal(old, new) => {
            changed.insert(prefix.to_string());
        }
        _ => {}
    }
}

fn json_values_semantically_equal(left: &Value, right: &Value) -> bool {
    let (Value::Number(left), Value::Number(right)) = (left, right) else {
        return left == right;
    };
    let left_integer = json_integer(left);
    let right_integer = json_integer(right);
    match (left_integer, right_integer) {
        (Some(left), Some(right)) => left == right,
        (Some(integer), None) => integer_equals_json_float(integer, right),
        (None, Some(integer)) => integer_equals_json_float(integer, left),
        (None, None) => left.as_f64() == right.as_f64(),
    }
}

fn json_integer(value: &serde_json::Number) -> Option<i128> {
    value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))
}

fn integer_equals_json_float(integer: i128, value: &serde_json::Number) -> bool {
    const MAX_SAFE_JSON_FLOAT_INTEGER: i128 = 9_007_199_254_740_991;
    if !(-MAX_SAFE_JSON_FLOAT_INTEGER..=MAX_SAFE_JSON_FLOAT_INTEGER).contains(&integer) {
        return false;
    }
    value
        .as_f64()
        .is_some_and(|float| float.is_finite() && float == integer as f64)
}

fn collect_leaf_paths(prefix: &str, value: &Value, changed: &mut BTreeSet<String>) {
    if let Value::Object(object) = value {
        if object.is_empty() {
            changed.insert(prefix.to_string());
            return;
        }
        for (key, child) in object {
            collect_leaf_paths(&format!("{prefix}.{key}"), child, changed);
        }
        return;
    }
    changed.insert(prefix.to_string());
}

#[cfg(test)]
#[path = "tests_routes_suite_config.rs"]
mod tests;
