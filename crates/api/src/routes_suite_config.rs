use std::{collections::BTreeSet, fs};

use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use vpsman_common::{
    redact_suite_config_value, PrivilegeAssertion, SuiteConfig, SuiteConfigValidation,
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
    verify_privilege_intent(
        &state,
        &DbPrivilegeIntent::new("suite_config.update", "suite_config", None, &[], true),
        request.privilege_assertion.clone(),
    )
    .await?;
    let (_exists, old_text) = read_suite_config_text(&state)?;
    let old_redacted = redacted_toml_json(&old_text)?;
    let new_redacted = redacted_toml_json(&request.toml)?;
    let changed_keys = changed_json_paths(&old_redacted, &new_redacted);
    write_suite_config_atomically(&state, &request.toml)?;
    state
        .repo
        .record_suite_config_audit(
            &operator,
            &state.suite_config_path.display().to_string(),
            &changed_keys,
            old_redacted,
            new_redacted,
        )
        .await?;
    Ok(Json(UpdateSuiteConfigResponse {
        path: state.suite_config_path.display().to_string(),
        changed_keys,
        validation: parsed.validation_summary(),
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
    let old_redacted = redacted_toml_json(&old_text)?;
    let redacted = redacted_toml_json(&request.toml)?;
    Ok(Json(ValidateSuiteConfigResponse {
        path: state.suite_config_path.display().to_string(),
        exists,
        changed_keys: changed_json_paths(&old_redacted, &redacted),
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
    if let Some(parent) = state.suite_config_path.parent() {
        fs::create_dir_all(parent).map_err(|_| ApiError::conflict("suite_config_dir_failed"))?;
    }
    let tmp_path = state
        .suite_config_path
        .with_extension(format!("toml.tmp-{}", Uuid::new_v4()));
    fs::write(&tmp_path, text).map_err(|_| ApiError::conflict("suite_config_write_failed"))?;
    fs::rename(&tmp_path, &state.suite_config_path)
        .map_err(|_| ApiError::conflict("suite_config_rename_failed"))?;
    Ok(())
}

fn redacted_toml_json(text: &str) -> Result<Value, ApiError> {
    let value = toml::from_str::<toml::Value>(text)
        .map_err(|_| ApiError::bad_request("suite_config_invalid_toml"))?;
    let json =
        serde_json::to_value(value).map_err(|error| ApiError::from(anyhow::anyhow!(error)))?;
    Ok(redact_suite_config_value(json))
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
                    _ => {
                        changed.insert(path);
                    }
                }
            }
        }
        _ if old != new => {
            changed.insert(prefix.to_string());
        }
        _ => {}
    }
}
