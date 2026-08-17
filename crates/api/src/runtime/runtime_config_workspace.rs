use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Map, Value};
use vpsman_common::{payload_hash, runtime_config_content_hash, AgentRuntimeConfig};

use crate::{
    model::{
        AgentView, ConfigurationSourceView, RuntimeConfigBulkPreviewView,
        RuntimeConfigBulkTargetPreviewView, RuntimeConfigFieldPolicyView,
        RuntimeConfigOverrideCandidate, RuntimeConfigOverridePreviewView,
        RuntimeConfigOverrideView, RuntimeConfigPatchOperationView, RuntimeConfigPathChangeView,
        RuntimeConfigProvenanceView, RuntimeConfigSavedOverrideView, RuntimeConfigWorkspaceView,
        TunnelPlanView,
    },
    runtime_config::{
        clear_runtime_tunnel_credentials, compose_runtime_config_for_agent_with_managed_inputs,
        compose_runtime_config_for_agent_with_read_model_and_override,
        load_runtime_config_managed_inputs, RuntimeConfigManagedInputs,
    },
    state::AppState,
    unix_now, ApiError,
};

const OWNER_DEFAULT: &str = "runtime_override";
const OWNER_PRESET: &str = "configuration_preset";
const OWNER_SERVER: &str = "server";
const OWNER_CONTROL_PLANE: &str = "control_plane";
const MAX_RUNTIME_CONFIG_BULK_CANDIDATE_BYTES: usize = 32 * 1024 * 1024;
const MAX_RUNTIME_CONFIG_BULK_DIFF_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct ParsedOverrideCandidate {
    pub(crate) canonical_toml: Option<String>,
    pub(crate) value: Value,
}

#[derive(Clone, Debug)]
enum PatchKind {
    Set(Value),
    DeleteField,
    DeleteTable,
}

#[derive(Clone, Debug)]
struct PatchOperation {
    path: String,
    segments: Vec<String>,
    kind: PatchKind,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeConfigBulkCandidate {
    pub(crate) client_id: String,
    pub(crate) expected_revision: String,
    pub(crate) canonical_toml: Option<String>,
    pub(crate) no_op: bool,
    pub(crate) storage_only: bool,
}

pub(crate) async fn load_runtime_config_workspace(
    state: &AppState,
    client_id: &str,
) -> Result<RuntimeConfigWorkspaceView, ApiError> {
    let context = load_context(state, client_id).await?;
    let saved = context.saved_override.as_ref();
    let inherited_config = compose_with_override(&context, state, None).await?;
    let parsed_result = saved.map(|record| parse_override_document(&record.toml));
    let parsed = parsed_result
        .as_ref()
        .and_then(|result| result.as_ref().ok());
    let (desired_config, diagnostic) = match (saved, parsed_result.as_ref()) {
        (None, _) => (inherited_config.clone(), None),
        (Some(_), Some(Err(error))) => (inherited_config.clone(), Some(concise_diagnostic(error))),
        (Some(record), Some(Ok(_))) => {
            match compose_with_override(&context, state, Some(&record.toml)).await {
                Ok(desired) => (desired, None),
                Err(error) => (
                    inherited_config.clone(),
                    Some(format!("stored override is invalid: {}", error.code)),
                ),
            }
        }
        _ => (inherited_config.clone(), None),
    };
    let inherited = projected_runtime_config(&inherited_config)?;
    let desired = projected_runtime_config(&desired_config)?;
    let desired_toml = projected_runtime_toml(&desired)?;
    let field_schema = runtime_config_field_policy(&desired);
    let override_value = parsed
        .as_ref()
        .map(|parsed| parsed.value.clone())
        .unwrap_or_else(empty_object);
    let provenance =
        runtime_config_provenance(&field_schema, &override_value, &context.sources, false);
    let apply_state = state
        .repo
        .list_runtime_config_apply_states(Some(client_id))
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .next();
    Ok(RuntimeConfigWorkspaceView {
        client_id: client_id.to_string(),
        inherited,
        desired,
        desired_toml,
        saved_override: RuntimeConfigSavedOverrideView {
            exists: saved.is_some(),
            toml: saved.map(|record| record.toml.clone()).unwrap_or_default(),
            parsed: parsed.map(|parsed| parsed.value.clone()),
            diagnostic,
            reason: saved.map(|record| record.reason.clone()),
            updated_at: saved.map(|record| record.updated_at.clone()),
            updated_by: saved.and_then(|record| record.updated_by),
        },
        apply_state,
        override_revision: runtime_config_override_revision(saved),
        desired_content_hash: runtime_config_content_hash(&desired_config).map_err(|error| {
            ApiError::internal(
                "runtime_config_desired_hash_failed",
                "The desired runtime configuration could not be hashed.",
                error.into(),
            )
        })?,
        desired_hash: desired_basis_hash(&desired_config, saved),
        provenance,
        field_schema,
        generated_at: unix_now().to_string(),
    })
}

pub(crate) async fn preview_runtime_config_override(
    state: &AppState,
    client_id: &str,
    candidate: &RuntimeConfigOverrideCandidate,
) -> Result<RuntimeConfigOverridePreviewView, ApiError> {
    let context = load_context(state, client_id).await?;
    let current = context.saved_override.as_ref();
    let inherited_config = compose_with_override(&context, state, None).await?;
    let (current_config, recovery_sync_required) = match current {
        Some(record) => match compose_with_override(&context, state, Some(&record.toml)).await {
            Ok(config) => (config, false),
            Err(_) => (inherited_config.clone(), true),
        },
        None => (inherited_config.clone(), false),
    };
    let parsed = parse_single_candidate(candidate).map_err(runtime_config_candidate_error)?;
    validate_override_against_policy(
        &parsed.value,
        &runtime_config_field_policy(&projected_runtime_config(&inherited_config)?),
    )
    .map_err(runtime_config_candidate_error)?;
    let candidate_config = compose_with_override(&context, state, parsed.canonical_toml.as_deref())
        .await
        .map_err(normalize_candidate_composition_error)?;
    let current_desired = projected_runtime_config(&current_config)?;
    let desired = projected_runtime_config(&candidate_config)?;
    let desired_toml = projected_runtime_toml(&desired)?;
    let field_schema = runtime_config_field_policy(&desired);
    let provenance =
        runtime_config_provenance(&field_schema, &parsed.value, &context.sources, true);
    let changes = diff_values(&current_desired, &desired);
    let storage_only = !recovery_sync_required
        && changes.is_empty()
        && current.map(|record| record.toml.as_str()) != parsed.canonical_toml.as_deref();
    let override_revision = runtime_config_override_revision(current);
    let desired_hash = desired_basis_hash(&current_config, current);
    let preview_hash = preview_hash(&(
        client_id,
        &override_revision,
        &desired_hash,
        &parsed.canonical_toml,
        &desired,
        recovery_sync_required,
    ))?;
    Ok(RuntimeConfigOverridePreviewView {
        client_id: client_id.to_string(),
        canonical_toml: parsed.canonical_toml,
        candidate_override: parsed.value,
        desired,
        desired_toml,
        provenance,
        changes,
        storage_only,
        recovery_sync_required,
        override_revision,
        desired_hash,
        preview_hash,
    })
}

pub(crate) async fn preview_runtime_config_bulk(
    state: &AppState,
    selector_expression: &str,
    target_client_ids: &[String],
    patch: &str,
) -> Result<
    (
        RuntimeConfigBulkPreviewView,
        Vec<RuntimeConfigBulkCandidate>,
    ),
    ApiError,
> {
    let operations = parse_bulk_patch(patch).map_err(runtime_config_bulk_patch_error)?;
    let contexts = load_bulk_contexts(state, target_client_ids).await?;
    let mut candidates = Vec::with_capacity(target_client_ids.len());
    let mut target_views = Vec::with_capacity(target_client_ids.len());
    let mut aggregate_candidate_bytes = 0usize;
    let mut aggregate_diff_bytes = 0usize;
    for (client_id, context) in target_client_ids.iter().zip(contexts) {
        let managed =
            load_runtime_config_managed_inputs(state, client_id, &context.tunnel_plans).await?;
        let current = context.saved_override.as_ref();
        let current_parsed = match current {
            Some(record) => parse_override_document(&record.toml)
                .map_err(|_| ApiError::conflict("runtime_config_bulk_stored_override_invalid"))?,
            None => ParsedOverrideCandidate {
                canonical_toml: None,
                value: empty_object(),
            },
        };
        // Validate the stored document against the target's real inherited
        // layers before using it as the base of an incremental mutation.
        let current_config = compose_with_override_and_managed(
            &context,
            current_parsed.canonical_toml.as_deref(),
            &managed,
        )
        .map_err(|_| ApiError::conflict("runtime_config_bulk_stored_override_invalid"))?;
        let candidate_value = apply_patch_operations(current_parsed.value.clone(), &operations)
            .map_err(runtime_config_bulk_patch_error)?;
        let candidate =
            canonical_override(candidate_value).map_err(runtime_config_bulk_candidate_error)?;
        validate_override_against_policy(
            &candidate.value,
            &runtime_config_field_policy(&projected_runtime_config(&current_config)?),
        )
        .map_err(runtime_config_bulk_candidate_error)?;
        let candidate_config = compose_with_override_and_managed(
            &context,
            candidate.canonical_toml.as_deref(),
            &managed,
        )
        .map_err(normalize_candidate_composition_error)?;
        let before_desired = projected_runtime_config(&current_config)?;
        let after_desired = projected_runtime_config(&candidate_config)?;
        let changes = diff_values(&before_desired, &after_desired);
        let no_op = current_parsed.value == candidate.value;
        let storage_only = !no_op && changes.is_empty();
        aggregate_candidate_bytes = aggregate_candidate_bytes
            .checked_add(candidate.canonical_toml.as_deref().map_or(0, str::len))
            .ok_or_else(runtime_config_bulk_aggregate_too_large)?;
        if aggregate_candidate_bytes > MAX_RUNTIME_CONFIG_BULK_CANDIDATE_BYTES {
            return Err(runtime_config_bulk_aggregate_too_large());
        }
        aggregate_diff_bytes = aggregate_diff_bytes
            .checked_add(
                serde_json::to_vec(&changes)
                    .map_err(|error| {
                        ApiError::internal(
                            "runtime_config_bulk_diff_projection_failed",
                            "The runtime configuration changes could not be displayed.",
                            error.into(),
                        )
                    })?
                    .len(),
            )
            .ok_or_else(runtime_config_bulk_aggregate_too_large)?;
        if aggregate_diff_bytes > MAX_RUNTIME_CONFIG_BULK_DIFF_BYTES {
            return Err(runtime_config_bulk_aggregate_too_large());
        }
        let override_revision = runtime_config_override_revision(current);
        let candidate_override_hash = candidate_override_hash(candidate.canonical_toml.as_deref());
        let desired_hash = desired_basis_hash(&current_config, current);
        target_views.push(RuntimeConfigBulkTargetPreviewView {
            client_id: client_id.clone(),
            candidate_override_hash,
            override_revision: override_revision.clone(),
            desired_hash,
            changes,
            no_op,
            storage_only,
        });
        candidates.push(RuntimeConfigBulkCandidate {
            client_id: client_id.clone(),
            expected_revision: override_revision,
            canonical_toml: candidate.canonical_toml,
            no_op,
            storage_only,
        });
    }
    let operation_views = operations
        .iter()
        .map(PatchOperation::view)
        .collect::<Vec<_>>();
    let preview_hash = preview_hash(&(
        selector_expression,
        target_client_ids,
        &operation_views,
        &target_views,
    ))?;
    let preview = RuntimeConfigBulkPreviewView {
        selector_expression: selector_expression.to_string(),
        target_client_ids: target_client_ids.to_vec(),
        operations: operation_views,
        changed_target_count: candidates
            .iter()
            .filter(|candidate| !candidate.no_op)
            .count(),
        targets: target_views,
        preview_hash,
    };
    Ok((preview, candidates))
}

fn candidate_override_hash(canonical_toml: Option<&str>) -> String {
    let mut payload = b"runtime-config-override-candidate:v1\0".to_vec();
    payload.extend_from_slice(canonical_toml.unwrap_or_default().as_bytes());
    payload_hash(&payload)
}

fn runtime_config_bulk_aggregate_too_large() -> ApiError {
    ApiError {
        status: axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        code: "runtime_config_bulk_preview_too_large",
        error: anyhow::anyhow!("runtime_config_bulk_preview_too_large"),
        public_message: Some(
            "The combined runtime configuration preview is too large; narrow the targets or patch."
                .to_string(),
        ),
    }
}

pub(crate) fn runtime_config_override_revision(
    record: Option<&RuntimeConfigOverrideView>,
) -> String {
    match record {
        None => payload_hash(b"runtime-config-override:absent:v1"),
        Some(record) => payload_hash(
            format!(
                "runtime-config-override:v1\0{}\0{}\0{}\0{}\0{}",
                record.client_id,
                record.toml,
                record.reason,
                record.updated_at,
                record
                    .updated_by
                    .map(|id| id.to_string())
                    .unwrap_or_default()
            )
            .as_bytes(),
        ),
    }
}

pub(crate) fn parse_single_candidate(
    candidate: &RuntimeConfigOverrideCandidate,
) -> Result<ParsedOverrideCandidate> {
    match candidate {
        RuntimeConfigOverrideCandidate::Reset => Ok(ParsedOverrideCandidate {
            canonical_toml: None,
            value: empty_object(),
        }),
        RuntimeConfigOverrideCandidate::Toml { toml } => parse_override_document(toml),
        RuntimeConfigOverrideCandidate::Structured { value } => canonical_override(value.clone()),
    }
}

pub(crate) fn parse_override_document(document: &str) -> Result<ParsedOverrideCandidate> {
    anyhow::ensure!(
        document.len() <= vpsman_common::MAX_RUNTIME_CONFIG_PATCH_BYTES,
        "runtime_config_override_too_large"
    );
    let parsed: toml::Value =
        toml::from_str(document).context("runtime_config_override_toml_invalid")?;
    let json = serde_json::to_value(parsed).context("runtime_config_override_projection_failed")?;
    canonical_override(json)
}

fn canonical_override(mut value: Value) -> Result<ParsedOverrideCandidate> {
    anyhow::ensure!(value.is_object(), "runtime_config_override_root_invalid");
    reject_identity_and_bootstrap_json(&value)?;
    prune_empty_objects(&mut value);
    validate_override_value_policy(&value)?;
    if value.as_object().is_some_and(Map::is_empty) {
        return Ok(ParsedOverrideCandidate {
            canonical_toml: None,
            value: empty_object(),
        });
    }
    let toml_value =
        toml::Value::try_from(&value).context("runtime_config_override_projection_failed")?;
    let canonical_toml = toml::to_string_pretty(&toml_value)
        .context("runtime_config_override_canonicalization_failed")?;
    anyhow::ensure!(
        canonical_toml.len() <= vpsman_common::MAX_RUNTIME_CONFIG_PATCH_BYTES,
        "runtime_config_override_too_large"
    );
    Ok(ParsedOverrideCandidate {
        canonical_toml: Some(canonical_toml),
        value,
    })
}

fn validate_override_value_policy(value: &Value) -> Result<()> {
    let default = serde_json::to_value(AgentRuntimeConfig::default())
        .context("runtime_config_field_policy_projection_failed")?;
    let schema = runtime_config_field_policy(&default);
    validate_override_against_policy(value, &schema)
}

fn validate_override_against_policy(
    value: &Value,
    schema: &[RuntimeConfigFieldPolicyView],
) -> Result<()> {
    let by_path = schema
        .iter()
        .map(|field| (field.path.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let mut leaves = Vec::new();
    flatten_override_paths(value, &mut Vec::new(), &mut leaves);
    for path in leaves {
        let Some(field) = by_path.get(path.as_str()) else {
            if preset_behavior_for_path(&path).is_some() || is_server_managed_path(&path) {
                anyhow::bail!("runtime_config_override_locked_field:{path}");
            }
            anyhow::bail!("runtime_config_override_unknown_field:{path}");
        };
        anyhow::ensure!(
            field.editable,
            "runtime_config_override_locked_field:{path}"
        );
    }
    Ok(())
}

fn flatten_override_paths(value: &Value, path: &mut Vec<String>, output: &mut Vec<String>) {
    match value {
        Value::Object(object) if !object.is_empty() => {
            for (key, value) in object {
                path.push(key.clone());
                flatten_override_paths(value, path, output);
                path.pop();
            }
        }
        Value::Object(_) if path.is_empty() => {}
        _ => output.push(path.join(".")),
    }
}

fn reject_identity_and_bootstrap_json(value: &Value) -> Result<()> {
    let object = value
        .as_object()
        .context("runtime_config_override_root_invalid")?;
    for forbidden in [
        "client_id",
        "display_name",
        "tags",
        "tcp_endpoints",
        "noise",
        "server_public_key",
        "secret",
        "auth",
        "version",
    ] {
        anyhow::ensure!(
            !object.contains_key(forbidden),
            "runtime_config_override_forbidden_field:{forbidden}"
        );
    }
    Ok(())
}

fn parse_bulk_patch(document: &str) -> Result<Vec<PatchOperation>> {
    anyhow::ensure!(
        !document.trim().is_empty()
            && document.len() <= vpsman_common::MAX_RUNTIME_CONFIG_PATCH_BYTES,
        "runtime_config_bulk_patch_invalid"
    );
    anyhow::ensure!(
        !document.lines().any(|line| {
            let syntax = line.split('#').next().unwrap_or_default();
            syntax.contains("\"\"\"") || syntax.contains("'''")
        }),
        "runtime_config_bulk_patch_multiline_string_forbidden"
    );
    let mut toml_lines = Vec::new();
    let mut deletions = Vec::new();
    let mut current_table = Vec::<String>::new();
    let mut assignment_paths = BTreeSet::new();
    for original_line in document.lines() {
        let trimmed = original_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            toml_lines.push(original_line);
            continue;
        }
        let without_comment = trimmed.split('#').next().unwrap_or_default().trim();
        if let Some(inner) = without_comment
            .strip_prefix("-[")
            .and_then(|value| value.strip_suffix(']'))
        {
            let segments = parse_patch_path(inner)?;
            deletions.push(PatchOperation {
                path: segments.join("."),
                segments,
                kind: PatchKind::DeleteTable,
            });
            continue;
        }
        if let Some(path) = without_comment.strip_prefix('-') {
            if path
                .trim()
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic())
            {
                let segments = parse_patch_path(path.trim())?;
                deletions.push(PatchOperation {
                    path: segments.join("."),
                    segments,
                    kind: PatchKind::DeleteField,
                });
                continue;
            }
        }
        if without_comment.starts_with("[[") {
            anyhow::bail!("runtime_config_bulk_array_table_forbidden");
        }
        if let Some(inner) = without_comment
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        {
            current_table = parse_patch_path(inner)?;
        } else if let Some((key, _)) = without_comment.split_once('=') {
            let mut path = current_table.clone();
            path.extend(parse_patch_path(key.trim())?);
            assignment_paths.insert(path.join("."));
        }
        toml_lines.push(original_line);
    }
    let normal_document = toml_lines.join("\n");
    let normal: toml::Value =
        toml::from_str(&normal_document).context("runtime_config_bulk_patch_toml_invalid")?;
    anyhow::ensure!(normal.is_table(), "runtime_config_bulk_patch_invalid");
    let normal_json = serde_json::to_value(normal).context("runtime_config_bulk_patch_invalid")?;
    reject_identity_and_bootstrap_json(&normal_json)?;
    let mut operations = Vec::new();
    flatten_set_operations(&normal_json, &mut Vec::new(), &mut operations);
    // The syntax scan and parsed leaf set must agree. This rejects dynamic,
    // multiline and empty-table constructs whose operation would be unclear.
    let flattened = operations
        .iter()
        .map(|operation| operation.path.clone())
        .collect::<BTreeSet<_>>();
    anyhow::ensure!(
        assignment_paths == flattened,
        "runtime_config_bulk_patch_assignment_ambiguous"
    );
    operations.extend(deletions);
    validate_patch_operations(&operations)?;
    operations.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(operations)
}

pub(crate) fn validate_runtime_config_bulk_patch(
    document: &str,
) -> Result<(Vec<RuntimeConfigPatchOperationView>, Vec<String>)> {
    let operations = parse_bulk_patch(document)?;
    let views = operations
        .iter()
        .map(PatchOperation::view)
        .collect::<Vec<_>>();
    let sections = operations
        .iter()
        .filter_map(|operation| operation.segments.first().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok((views, sections))
}

fn parse_patch_path(path: &str) -> Result<Vec<String>> {
    anyhow::ensure!(
        !path.is_empty() && !path.contains(['\'', '"', '[', ']']),
        "runtime_config_bulk_patch_path_invalid"
    );
    let segments = path
        .split('.')
        .map(str::trim)
        .map(str::to_string)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !segments.is_empty()
            && segments.iter().all(|segment| {
                let mut chars = segment.chars();
                chars.next().is_some_and(|ch| ch.is_ascii_lowercase())
                    && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
            }),
        "runtime_config_bulk_patch_path_invalid"
    );
    anyhow::ensure!(
        !matches!(
            segments.first().map(String::as_str),
            Some("display_name" | "tags")
        ),
        "runtime_config_override_forbidden_field"
    );
    Ok(segments)
}

fn flatten_set_operations(value: &Value, path: &mut Vec<String>, output: &mut Vec<PatchOperation>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                path.push(key.clone());
                flatten_set_operations(value, path, output);
                path.pop();
            }
        }
        value => output.push(PatchOperation {
            path: path.join("."),
            segments: path.clone(),
            kind: PatchKind::Set(value.clone()),
        }),
    }
}

fn validate_patch_operations(operations: &[PatchOperation]) -> Result<()> {
    anyhow::ensure!(!operations.is_empty(), "runtime_config_bulk_patch_empty");
    let mut paths = BTreeSet::new();
    for operation in operations {
        anyhow::ensure!(
            paths.insert(operation.path.clone()),
            "runtime_config_bulk_patch_duplicate"
        );
    }
    for (index, left) in operations.iter().enumerate() {
        for right in operations.iter().skip(index + 1) {
            if path_is_prefix(&left.segments, &right.segments)
                || path_is_prefix(&right.segments, &left.segments)
            {
                anyhow::bail!("runtime_config_bulk_patch_overlap");
            }
        }
    }
    let mut default_value = serde_json::to_value(AgentRuntimeConfig::default())
        .context("runtime_config_field_policy_projection_failed")?;
    default_value
        .as_object_mut()
        .map(|object| object.remove("version"));
    let schema = runtime_config_field_policy(&default_value);
    let containers = schema
        .iter()
        .filter(|field| field.value_type == "object")
        .map(|field| field.path.as_str())
        .collect::<BTreeSet<_>>();
    let editable = schema
        .iter()
        .filter(|field| field.editable)
        .map(|field| field.path.as_str())
        .collect::<BTreeSet<_>>();
    let known = schema
        .iter()
        .map(|field| field.path.as_str())
        .collect::<BTreeSet<_>>();
    for operation in operations {
        let exact_known = known.contains(operation.path.as_str());
        let exact_editable = editable.contains(operation.path.as_str());
        let descendant_prefix = format!("{}.", operation.path);
        let has_known_descendant = known
            .iter()
            .any(|path| path.starts_with(&descendant_prefix));
        let has_editable_descendant = editable
            .iter()
            .any(|path| path.starts_with(&descendant_prefix));
        let statically_locked = preset_behavior_for_path(&operation.path).is_some()
            || is_server_managed_path(&operation.path);
        match operation.kind {
            PatchKind::DeleteTable => {
                anyhow::ensure!(
                    exact_known || has_known_descendant,
                    "runtime_config_bulk_patch_path_unknown"
                );
                anyhow::ensure!(
                    has_editable_descendant && !statically_locked,
                    "runtime_config_bulk_patch_field_forbidden"
                );
            }
            PatchKind::Set(_) => {
                if !exact_known && !statically_locked {
                    anyhow::bail!("runtime_config_bulk_patch_path_unknown");
                }
                anyhow::ensure!(
                    exact_editable && !statically_locked,
                    "runtime_config_bulk_patch_field_forbidden"
                );
            }
            PatchKind::DeleteField => {
                if !exact_known && !statically_locked {
                    anyhow::bail!("runtime_config_bulk_patch_path_unknown");
                }
                anyhow::ensure!(
                    exact_editable
                        && !containers.contains(operation.path.as_str())
                        && !statically_locked,
                    "runtime_config_bulk_patch_field_forbidden"
                );
            }
        }
    }
    Ok(())
}

fn apply_patch_operations(mut base: Value, operations: &[PatchOperation]) -> Result<Value> {
    anyhow::ensure!(base.is_object(), "runtime_config_override_root_invalid");
    for operation in operations {
        match &operation.kind {
            PatchKind::Set(value) => set_json_path(&mut base, &operation.segments, value.clone())?,
            PatchKind::DeleteField | PatchKind::DeleteTable => {
                delete_json_path(&mut base, &operation.segments)?;
            }
        }
    }
    prune_empty_objects(&mut base);
    Ok(base)
}

fn set_json_path(target: &mut Value, path: &[String], value: Value) -> Result<()> {
    let (leaf, parents) = path
        .split_last()
        .context("runtime_config_bulk_patch_path_invalid")?;
    let mut cursor = target;
    for segment in parents {
        let object = cursor
            .as_object_mut()
            .context("runtime_config_bulk_patch_path_conflict")?;
        cursor = object
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    cursor
        .as_object_mut()
        .context("runtime_config_bulk_patch_path_conflict")?
        .insert(leaf.clone(), value);
    Ok(())
}

fn delete_json_path(target: &mut Value, path: &[String]) -> Result<()> {
    let (leaf, parents) = path
        .split_last()
        .context("runtime_config_bulk_patch_path_invalid")?;
    let mut cursor = target;
    for segment in parents {
        let Some(next) = cursor
            .as_object_mut()
            .and_then(|object| object.get_mut(segment))
        else {
            return Ok(());
        };
        cursor = next;
    }
    if let Some(object) = cursor.as_object_mut() {
        object.remove(leaf);
    }
    Ok(())
}

fn path_is_prefix(left: &[String], right: &[String]) -> bool {
    left.len() <= right.len() && left.iter().zip(right).all(|(left, right)| left == right)
}

impl PatchOperation {
    fn view(&self) -> RuntimeConfigPatchOperationView {
        let (operation, value) = match &self.kind {
            PatchKind::Set(value) => ("set", Some(value.clone())),
            PatchKind::DeleteField => ("delete_field", None),
            PatchKind::DeleteTable => ("delete_table", None),
        };
        RuntimeConfigPatchOperationView {
            operation: operation.to_string(),
            path: self.path.clone(),
            value,
        }
    }
}

struct WorkspaceContext {
    agent: AgentView,
    preset_toml: String,
    tunnel_plans: Arc<[TunnelPlanView]>,
    sources: Vec<ConfigurationSourceView>,
    saved_override: Option<RuntimeConfigOverrideView>,
}

async fn load_context(state: &AppState, client_id: &str) -> Result<WorkspaceContext, ApiError> {
    let agent = state
        .repo
        .list_agents_for_client_ids(&[client_id.to_string()])
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found("runtime_config_client_not_found"))?;
    let preset_toml = state
        .repo
        .render_configuration_preset_patch_toml(client_id)
        .await
        .map_err(ApiError::from)?;
    let tunnel_plans = state
        .repo
        .list_tunnel_plans()
        .await
        .map_err(ApiError::from)?
        .into();
    let sources = state
        .repo
        .list_configuration_sources(Some(client_id), None)
        .await
        .map_err(ApiError::from)?;
    let saved_override = state
        .repo
        .list_runtime_config_overrides(Some(client_id))
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .next();
    Ok(WorkspaceContext {
        agent,
        preset_toml,
        tunnel_plans,
        sources,
        saved_override,
    })
}

async fn load_bulk_contexts(
    state: &AppState,
    client_ids: &[String],
) -> Result<Vec<WorkspaceContext>, ApiError> {
    let agents = state
        .repo
        .list_agents_for_client_ids(client_ids)
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .map(|agent| (agent.id.clone(), agent))
        .collect::<BTreeMap<_, _>>();
    let tunnel_plans: Arc<[TunnelPlanView]> = state
        .repo
        .list_tunnel_plans()
        .await
        .map_err(ApiError::from)?
        .into();
    let mut overrides = state
        .repo
        .list_runtime_config_overrides_for_clients(client_ids)
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .map(|record| (record.client_id.clone(), record))
        .collect::<BTreeMap<_, _>>();
    let mut preset_tomls = state
        .repo
        .render_configuration_preset_patches_for_clients(client_ids)
        .await
        .map_err(ApiError::from)?;
    let mut contexts = Vec::with_capacity(client_ids.len());
    for client_id in client_ids {
        let agent = agents
            .get(client_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found("runtime_config_client_not_found"))?;
        let preset_toml = preset_tomls
            .remove(client_id)
            .ok_or_else(|| ApiError::not_found("runtime_config_client_not_found"))?;
        contexts.push(WorkspaceContext {
            agent,
            preset_toml,
            tunnel_plans: tunnel_plans.clone(),
            // Bulk preview never renders per-field provenance, so loading the
            // same configuration-source presentation rows would be wasted.
            sources: Vec::new(),
            saved_override: overrides.remove(client_id),
        });
    }
    Ok(contexts)
}

async fn compose_with_override(
    context: &WorkspaceContext,
    state: &AppState,
    override_toml: Option<&str>,
) -> Result<AgentRuntimeConfig, ApiError> {
    compose_runtime_config_for_agent_with_read_model_and_override(
        state,
        &context.agent,
        1,
        &context.preset_toml,
        &context.tunnel_plans,
        override_toml,
    )
    .await
}

fn compose_with_override_and_managed(
    context: &WorkspaceContext,
    override_toml: Option<&str>,
    managed: &RuntimeConfigManagedInputs,
) -> Result<AgentRuntimeConfig, ApiError> {
    compose_runtime_config_for_agent_with_managed_inputs(
        &context.agent,
        1,
        &context.preset_toml,
        &context.tunnel_plans,
        override_toml,
        managed,
    )
}

fn projected_runtime_config(config: &AgentRuntimeConfig) -> Result<Value, ApiError> {
    let mut redacted = config.clone();
    clear_runtime_tunnel_credentials(&mut redacted.network);
    let mut value = serde_json::to_value(redacted).map_err(|error| {
        ApiError::internal(
            "runtime_config_projection_failed",
            "The runtime configuration could not be displayed.",
            error.into(),
        )
    })?;
    value.as_object_mut().map(|object| object.remove("version"));
    Ok(value)
}

fn projected_runtime_toml(value: &Value) -> Result<String, ApiError> {
    toml::to_string_pretty(value).map_err(|error| {
        ApiError::internal(
            "runtime_config_projection_failed",
            "The runtime configuration could not be displayed.",
            error.into(),
        )
    })
}

fn desired_basis_hash(
    config: &AgentRuntimeConfig,
    saved: Option<&RuntimeConfigOverrideView>,
) -> String {
    let content = runtime_config_content_hash(config).unwrap_or_else(|_| "invalid".to_string());
    payload_hash(
        format!(
            "runtime-config-basis:v1\0{content}\0{}",
            runtime_config_override_revision(saved)
        )
        .as_bytes(),
    )
}

fn preview_hash(value: &impl Serialize) -> Result<String, ApiError> {
    serde_json::to_vec(value)
        .map(|payload| payload_hash(&payload))
        .map_err(|error| {
            ApiError::internal(
                "runtime_config_preview_hash_failed",
                "The runtime configuration preview could not be generated.",
                error.into(),
            )
        })
}

pub(crate) fn runtime_config_field_policy(desired: &Value) -> Vec<RuntimeConfigFieldPolicyView> {
    let shape = runtime_config_policy_shape(desired);
    let network_forced = desired
        .pointer("/network/runtime_status_telemetry_plans")
        .and_then(Value::as_array)
        .is_some_and(|plans| !plans.is_empty());
    let mut fields = Vec::new();
    collect_field_policy(&shape, &mut Vec::new(), &mut fields, network_forced);
    fields
}

fn runtime_config_policy_shape(desired: &Value) -> Value {
    let mut shape = serde_json::to_value(AgentRuntimeConfig::default())
        .expect("default runtime config must serialize");
    shape.as_object_mut().map(|object| object.remove("version"));
    merge_json_shape(&mut shape, desired);
    for (path, placeholder) in [
        ("execution.working_directory", Value::String(String::new())),
        ("execution.environment_keep", Value::Array(Vec::new())),
        ("execution.environment_set", Value::Object(Map::new())),
        ("execution.user_sessions_command", Value::Object(Map::new())),
        (
            "execution.process_inventory_command",
            Value::Object(Map::new()),
        ),
        (
            "telemetry.custom_metrics_command",
            Value::Object(Map::new()),
        ),
        ("network.ospf_status_command", Value::Object(Map::new())),
        ("network.ospf_update_command", Value::Object(Map::new())),
        (
            "network.runtime_status_telemetry_plans",
            Value::Array(Vec::new()),
        ),
        ("network.ping_targets", Value::Array(Vec::new())),
    ] {
        ensure_json_path(&mut shape, path, placeholder);
    }
    shape
}

fn merge_json_shape(target: &mut Value, source: &Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                if let Some(existing) = target.get_mut(key) {
                    merge_json_shape(existing, value);
                } else {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
        (target, source) => *target = source.clone(),
    }
}

fn ensure_json_path(target: &mut Value, path: &str, placeholder: Value) {
    let segments = path.split('.').collect::<Vec<_>>();
    let Some((leaf, parents)) = segments.split_last() else {
        return;
    };
    let mut cursor = target;
    for segment in parents {
        let Some(next) = cursor
            .as_object_mut()
            .and_then(|object| object.get_mut(*segment))
        else {
            return;
        };
        cursor = next;
    }
    if let Some(object) = cursor.as_object_mut() {
        object.entry((*leaf).to_string()).or_insert(placeholder);
    }
}

fn collect_field_policy(
    value: &Value,
    path: &mut Vec<String>,
    fields: &mut Vec<RuntimeConfigFieldPolicyView>,
    network_forced: bool,
) {
    if !path.is_empty() {
        fields.push(field_policy(path, value, network_forced));
    }
    if let Value::Object(object) = value {
        for (key, value) in object {
            path.push(key.clone());
            collect_field_policy(value, path, fields, network_forced);
            path.pop();
        }
    }
}

fn field_policy(
    path: &[String],
    value: &Value,
    network_forced: bool,
) -> RuntimeConfigFieldPolicyView {
    let dotted = path.join(".");
    let pointer = format!("/{}", path.join("/"));
    let preset_owned = preset_behavior_for_path(&dotted).is_some();
    let server_managed = is_server_managed_path(&dotted);
    let control_plane = network_forced
        && (dotted == "network.apply_enabled"
            || dotted == "network.runtime_reconcile_enabled"
            || dotted == "network.runtime_status_telemetry_enabled");
    let (owner, owner_link) = if preset_owned {
        (OWNER_PRESET, Some("/config/presets".to_string()))
    } else if server_managed {
        (OWNER_SERVER, Some("/network".to_string()))
    } else if control_plane {
        (OWNER_CONTROL_PLANE, Some("/network".to_string()))
    } else {
        (OWNER_DEFAULT, None)
    };
    let is_container = value.is_object();
    let editable = owner == OWNER_DEFAULT && !is_container;
    let value_type = json_type(value);
    let control = if dotted == "execution.environment_set" {
        "map"
    } else {
        match value {
            Value::Object(_) => "section",
            Value::Bool(_) => "toggle",
            Value::Number(_) => "number",
            Value::Array(values) if values.iter().all(Value::is_string) => "text_list",
            Value::Array(_) => "object_list",
            _ => "text",
        }
    };
    let allowed_operations = if !editable {
        Vec::new()
    } else if value.is_array() {
        ["set", "inherit", "append", "remove_item", "reorder"]
            .into_iter()
            .map(str::to_string)
            .collect()
    } else {
        ["set", "inherit"].into_iter().map(str::to_string).collect()
    };
    RuntimeConfigFieldPolicyView {
        pointer,
        path: dotted.clone(),
        label: path
            .last()
            .map(|value| title_case(value))
            .unwrap_or_default(),
        value_type: value_type.to_string(),
        control: control.to_string(),
        editable,
        collection: value.is_array() || dotted == "execution.environment_set",
        owner: owner.to_string(),
        owner_link,
        allowed_operations,
        enum_values: enum_values_for_path(&dotted),
        unit: unit_for_path(&dotted).map(str::to_string),
    }
}

fn runtime_config_provenance(
    fields: &[RuntimeConfigFieldPolicyView],
    override_value: &Value,
    sources: &[ConfigurationSourceView],
    draft: bool,
) -> Vec<RuntimeConfigProvenanceView> {
    let preset_names = sources
        .iter()
        .map(|source| {
            (
                source.behavior.as_str(),
                source.effective_preset_name.as_str(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    fields
        .iter()
        .map(|field| {
            let overridden = json_path_exists(override_value, &field.path);
            let preset = preset_behavior_for_path(&field.path)
                .and_then(|behavior| preset_names.get(behavior).copied());
            let source = if field.owner == OWNER_SERVER {
                "Server managed".to_string()
            } else if field.owner == OWNER_CONTROL_PLANE {
                "Control plane".to_string()
            } else if let Some(preset) = preset {
                format!("Preset: {preset}")
            } else if overridden && draft {
                "Draft override".to_string()
            } else if overridden {
                "VPS override".to_string()
            } else {
                "Default".to_string()
            };
            let mut chain = vec!["Default".to_string()];
            if let Some(preset) = preset {
                chain.push(format!("Preset: {preset}"));
            }
            if overridden {
                chain.push(if draft {
                    "Draft override".to_string()
                } else {
                    "VPS override".to_string()
                });
            }
            if matches!(field.owner.as_str(), OWNER_SERVER | OWNER_CONTROL_PLANE) {
                chain.push(source.clone());
            }
            RuntimeConfigProvenanceView {
                pointer: field.pointer.clone(),
                path: field.path.clone(),
                source,
                source_chain: chain,
                locked: field.owner != OWNER_DEFAULT,
                owner: field.owner.clone(),
                owner_link: field.owner_link.clone(),
                shadowed_override: overridden
                    && matches!(field.owner.as_str(), OWNER_SERVER | OWNER_CONTROL_PLANE),
            }
        })
        .collect()
}

fn preset_behavior_for_path(path: &str) -> Option<&'static str> {
    if path.starts_with("telemetry.") {
        Some("host_metrics")
    } else if path == "network.probe_ping_argv" {
        Some("latency_probe")
    } else if path_is_exact_or_descendant(path, "network.ospf_status_command")
        || path_is_exact_or_descendant(path, "network.ospf_update_command")
    {
        Some("ospf_update_command")
    } else if matches!(
        path,
        "execution.shell_script_argv"
            | "execution.working_directory"
            | "execution.environment_policy"
            | "execution.environment_keep"
            | "execution.pty_policy"
            | "execution.process_cleanup"
    ) || path_is_exact_or_descendant(path, "execution.environment_set")
    {
        Some("command_execution")
    } else if path.starts_with("execution.process_") {
        Some("process_inventory")
    } else if path.starts_with("execution.user_sessions_") {
        Some("user_sessions")
    } else {
        None
    }
}

fn path_is_exact_or_descendant(path: &str, owner_path: &str) -> bool {
    path == owner_path
        || path
            .strip_prefix(owner_path)
            .is_some_and(|suffix| suffix.starts_with('.'))
}

fn is_server_managed_path(path: &str) -> bool {
    path == "network.runtime_status_telemetry_plans"
        || path.starts_with("network.runtime_status_telemetry_plans.")
        || path == "network.port_forwarding"
        || path.starts_with("network.port_forwarding.")
        || path == "network.ping_targets"
        || path.starts_with("network.ping_targets.")
}

fn json_path_exists(value: &Value, path: &str) -> bool {
    let mut cursor = value;
    for segment in path.split('.') {
        let Some(next) = cursor.as_object().and_then(|object| object.get(segment)) else {
            return false;
        };
        cursor = next;
    }
    true
}

fn diff_values(before: &Value, after: &Value) -> Vec<RuntimeConfigPathChangeView> {
    let mut changes = Vec::new();
    collect_diff(before, after, &mut Vec::new(), &mut changes);
    changes
}

fn collect_diff(
    before: &Value,
    after: &Value,
    path: &mut Vec<String>,
    changes: &mut Vec<RuntimeConfigPathChangeView>,
) {
    if before == after {
        return;
    }
    match (before, after) {
        (Value::Object(before), Value::Object(after)) => {
            let keys = before.keys().chain(after.keys()).collect::<BTreeSet<_>>();
            for key in keys {
                path.push(key.clone());
                match (before.get(key), after.get(key)) {
                    (Some(before), Some(after)) => collect_diff(before, after, path, changes),
                    (before, after) => {
                        changes.push(path_change(path, before.cloned(), after.cloned()))
                    }
                }
                path.pop();
            }
        }
        _ => changes.push(path_change(path, Some(before.clone()), Some(after.clone()))),
    }
}

fn path_change(
    path: &[String],
    before: Option<Value>,
    after: Option<Value>,
) -> RuntimeConfigPathChangeView {
    let kind = match (&before, &after) {
        (None, Some(_)) => "added",
        (Some(_), None) => "removed",
        _ => "changed",
    };
    RuntimeConfigPathChangeView {
        pointer: format!("/{}", path.join("/")),
        path: path.join("."),
        before,
        after,
        kind: kind.to_string(),
    }
}

fn prune_empty_objects(value: &mut Value) {
    if let Value::Object(object) = value {
        for value in object.values_mut() {
            prune_empty_objects(value);
        }
        object.retain(|_, value| !value.as_object().is_some_and(Map::is_empty));
    }
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn title_case(value: &str) -> String {
    let mut output = String::new();
    for (index, word) in value.split('_').enumerate() {
        if index > 0 {
            output.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            output.extend(first.to_uppercase());
            output.extend(chars);
        }
    }
    output
}

fn unit_for_path(path: &str) -> Option<&'static str> {
    if path.ends_with("_secs") {
        Some("seconds")
    } else if path.ends_with("_bytes") {
        Some("bytes")
    } else if path.ends_with("_millis") || path.ends_with("_ms") {
        Some("milliseconds")
    } else {
        None
    }
}

fn enum_values_for_path(path: &str) -> Vec<String> {
    let values: &[&str] = match path {
        "execution.environment_policy" => &["inherit", "clean", "minimal_path"],
        "execution.pty_policy" => &["native_pty", "disabled"],
        "execution.process_cleanup" => &["process_group", "direct_child"],
        "execution.user_sessions_source" => &["linux_w_who_preset", "custom_command"],
        "execution.process_inventory_source" => &["linux_procfs", "custom_command"],
        "telemetry.source" => &["linux_procfs", "custom_command"],
        "network.runtime_unprivileged_mutation_policy" => {
            &["skip", "try_custom_adapters", "try_all"]
        }
        _ => &[],
    };
    values.iter().map(|value| (*value).to_string()).collect()
}

fn concise_diagnostic(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(ToString::to_string)
        .take(3)
        .collect::<Vec<_>>()
        .join(": ")
}

fn normalize_candidate_composition_error(error: ApiError) -> ApiError {
    if error.code.starts_with("runtime_config_")
        || error
            .error
            .to_string()
            .contains("runtime_config_override_merge_failed")
    {
        ApiError::bad_request("runtime_config_override_invalid")
    } else {
        error
    }
}

fn runtime_config_candidate_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("forbidden_field") {
        ApiError::bad_request("runtime_config_override_field_forbidden")
    } else if message.contains("locked_field") {
        ApiError::bad_request("runtime_config_override_field_locked")
    } else if message.contains("unknown_field") {
        ApiError::bad_request("runtime_config_override_field_unknown")
    } else if message.contains("too_large") {
        ApiError::bad_request("runtime_config_override_too_large")
    } else {
        ApiError::bad_request("runtime_config_override_toml_invalid")
    }
}

fn runtime_config_bulk_patch_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    let code = if message.contains("path_unknown") {
        "runtime_config_bulk_patch_path_unknown"
    } else if message.contains("overlap") {
        "runtime_config_bulk_patch_overlap"
    } else if message.contains("duplicate") {
        "runtime_config_bulk_patch_duplicate"
    } else if message.contains("path_invalid") {
        "runtime_config_bulk_patch_path_invalid"
    } else if message.contains("forbidden") {
        "runtime_config_bulk_patch_field_forbidden"
    } else if message.contains("stored_override") {
        "runtime_config_bulk_stored_override_invalid"
    } else {
        "runtime_config_bulk_patch_invalid"
    };
    ApiError::bad_request(code)
}

fn runtime_config_bulk_candidate_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("locked_field") || message.contains("forbidden_field") {
        ApiError::bad_request("runtime_config_bulk_patch_field_forbidden")
    } else if message.contains("unknown_field") {
        ApiError::bad_request("runtime_config_bulk_patch_path_unknown")
    } else if message.contains("too_large") {
        ApiError::bad_request("runtime_config_override_too_large")
    } else {
        ApiError::bad_request("runtime_config_bulk_patch_invalid")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_patch_supports_set_and_absolute_deletions() {
        let operations = parse_bulk_patch(
            "[backup]\nmax_archive_bytes = 42\n\n-network.latency_monitoring_enabled\n-[update] # inherit update\n",
        )
        .unwrap();
        assert_eq!(operations.len(), 3);
        assert!(operations.iter().any(|operation| {
            operation.path == "backup.max_archive_bytes"
                && matches!(operation.kind, PatchKind::Set(_))
        }));
        assert!(operations.iter().any(|operation| {
            operation.path == "network.latency_monitoring_enabled"
                && matches!(operation.kind, PatchKind::DeleteField)
        }));
        assert!(operations.iter().any(|operation| {
            operation.path == "update" && matches!(operation.kind, PatchKind::DeleteTable)
        }));
    }

    #[test]
    fn bulk_patch_rejects_overlapping_operations() {
        let error = parse_bulk_patch("[backup]\nmax_archive_bytes = 42\n-[backup]").unwrap_err();
        assert!(error.to_string().contains("overlap"));
    }

    #[test]
    fn bulk_patch_rejects_directive_text_inside_multiline_strings() {
        let error = parse_bulk_patch(
            "[update]\nunmanaged_version_url = \"\"\"https://example.com/\n-[backup]\n\"\"\"",
        )
        .unwrap_err();
        assert!(error.to_string().contains("multiline_string_forbidden"));
    }

    #[test]
    fn structured_override_rejects_an_oversized_canonical_document() {
        let error = canonical_override(serde_json::json!({
            "network": {
                "runtime_ip_argv": vec![
                    "x".repeat(vpsman_common::MAX_RUNTIME_CONFIG_FIELD_BYTES);
                    (vpsman_common::MAX_RUNTIME_CONFIG_PATCH_BYTES
                        / vpsman_common::MAX_RUNTIME_CONFIG_FIELD_BYTES)
                        + 1
                ]
            }
        }))
        .unwrap_err();
        assert!(error.to_string().contains("too_large"));
    }

    #[test]
    fn bulk_patch_missing_delete_is_a_no_op() {
        let operations = parse_bulk_patch("-backup.max_archive_bytes").unwrap();
        let base = serde_json::json!({"update": {"enabled": true}});
        assert_eq!(
            apply_patch_operations(base.clone(), &operations).unwrap(),
            base
        );
    }

    #[test]
    fn identity_fields_are_never_runtime_overrides() {
        let error = parse_override_document("display_name = 'wrong'").unwrap_err();
        assert!(error.to_string().contains("forbidden_field"));
        let error = parse_bulk_patch("tags = ['wrong']").unwrap_err();
        assert!(error.to_string().contains("forbidden_field"));
    }

    #[test]
    fn process_cleanup_keeps_command_execution_provenance() {
        assert_eq!(
            preset_behavior_for_path("execution.process_cleanup"),
            Some("command_execution")
        );
        assert_eq!(
            preset_behavior_for_path("execution.process_inventory_source"),
            Some("process_inventory")
        );
        assert_eq!(
            preset_behavior_for_path("execution.environment_set.PATH"),
            Some("command_execution")
        );
        assert_eq!(
            preset_behavior_for_path("network.ospf_status_command.argv"),
            Some("ospf_update_command")
        );
    }

    #[test]
    fn dynamic_preset_descendants_are_locked_for_single_replacements() {
        let error = canonical_override(serde_json::json!({
            "execution": {"environment_set": {"new_key": "value"}}
        }))
        .unwrap_err();
        assert!(error.to_string().contains("locked_field"));
    }

    #[test]
    fn bulk_patch_distinguishes_locked_fields_from_unknown_paths() {
        let locked = parse_bulk_patch("[telemetry]\nproc_root = '/host/proc'").unwrap_err();
        assert!(locked.to_string().contains("field_forbidden"));

        let locked_dynamic =
            parse_bulk_patch("[execution.environment_set]\npath = '/bin'").unwrap_err();
        assert!(locked_dynamic.to_string().contains("field_forbidden"));

        let unknown = parse_bulk_patch("[unknown]\nvalue = true").unwrap_err();
        assert!(unknown.to_string().contains("path_unknown"));

        assert_eq!(
            runtime_config_bulk_candidate_error(anyhow::anyhow!(
                "runtime_config_override_locked_field:network.apply_enabled"
            ))
            .code,
            "runtime_config_bulk_patch_field_forbidden"
        );
    }

    #[test]
    fn policy_registry_covers_every_projected_runtime_node() {
        let desired = projected_runtime_config(&AgentRuntimeConfig::default()).unwrap();
        let fields = runtime_config_field_policy(&desired);
        let shape = runtime_config_policy_shape(&desired);
        let mut expected = Vec::new();
        fn collect(value: &Value, path: &mut Vec<String>, output: &mut Vec<String>) {
            if !path.is_empty() {
                output.push(path.join("."));
            }
            if let Value::Object(object) = value {
                for (key, value) in object {
                    path.push(key.clone());
                    collect(value, path, output);
                    path.pop();
                }
            }
        }
        collect(&shape, &mut Vec::new(), &mut expected);
        assert_eq!(
            fields
                .iter()
                .map(|field| field.path.clone())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(fields.iter().all(|field| field.path != "version"));
    }
}
