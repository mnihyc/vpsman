use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    runtime_config_content_hash, validate_agent_config_shape, AgentConfig, AgentRuntimeConfig,
    AgentRuntimeStatusTelemetryPlan, AgentRuntimeTrafficSource, JobCommand, TunnelEndpointSide,
};

use crate::{
    error::ApiError,
    model::{AuthContext, CreateJobRequest, CreateJobResponse, OperatorPreferences, OperatorView},
    routes_jobs::create_job_from_internal_operator_mutation,
    state::AppState,
    DEFAULT_REFRESH_TOKEN_TTL_SECS,
};

pub(crate) async fn push_runtime_config_for_clients(
    state: &AppState,
    operator: &AuthContext,
    client_ids: impl IntoIterator<Item = String>,
    reason: &str,
) -> Result<Vec<CreateJobResponse>, ApiError> {
    let clients = client_ids
        .into_iter()
        .filter(|client_id| !client_id.trim().is_empty())
        .collect::<BTreeSet<_>>();
    let known_clients = state
        .repo
        .list_agents()
        .await?
        .into_iter()
        .map(|agent| agent.id)
        .collect::<BTreeSet<_>>();
    let mut responses = Vec::with_capacity(clients.len());
    for client_id in clients {
        if !known_clients.contains(&client_id) {
            warn!(
                client_id,
                reason, "skipping runtime config sync for unknown agent"
            );
            continue;
        }
        let version = runtime_config_version();
        let config = compose_runtime_config(state, &client_id, version).await?;
        let request = CreateJobRequest {
            job_id: Some(Uuid::new_v4()),
            selector_expression: String::new(),
            target_client_ids: vec![client_id],
            destructive: false,
            confirmed: true,
            command: "runtime_config_sync".to_string(),
            argv: Vec::new(),
            operation: Some(JobCommand::RuntimeConfigSync {
                desired_version: version,
                reason: reason.to_string(),
                config: Box::new(config),
            }),
            max_timeout_secs: Some(300),
            force_unprivileged: false,
            privileged: true,
            privilege_assertion: None,
        };
        let (_, response) =
            create_job_from_internal_operator_mutation(state, operator, request).await?;
        responses.push(response.0);
    }
    Ok(responses)
}

pub(crate) async fn request_runtime_config_reload_for_agent(
    state: &AppState,
    client_id: &str,
    current_content_hash: &str,
    reason: &str,
) -> Result<Vec<CreateJobResponse>, ApiError> {
    let version = runtime_config_version();
    let config = compose_runtime_config(state, client_id, version).await?;
    let desired_content_hash = runtime_config_content_hash(&config)
        .map_err(|error| ApiError::from(anyhow::anyhow!("runtime config hash failed: {error}")))?;
    if desired_content_hash.eq_ignore_ascii_case(current_content_hash.trim()) {
        return Ok(Vec::new());
    }
    let operator = runtime_config_system_operator();
    push_runtime_config_job(
        state,
        &operator,
        client_id.to_string(),
        reason,
        version,
        config,
    )
    .await
    .map(|response| vec![response])
}

pub(crate) async fn compose_runtime_config(
    state: &AppState,
    client_id: &str,
    version: u64,
) -> Result<AgentRuntimeConfig, ApiError> {
    let agents = state.repo.list_agents().await?;
    let agent = agents
        .iter()
        .find(|candidate| candidate.id == client_id)
        .with_context(|| format!("runtime_config_client_not_found:{client_id}"))?;
    let mut effective = AgentConfig {
        client_id: agent.id.clone(),
        display_name: agent.display_name.clone(),
        tags: agent.tags.clone(),
        ..AgentConfig::default()
    };

    let rendered = state.repo.render_template_runtime_config(client_id).await?;
    if !rendered.toml.trim().is_empty() {
        merge_runtime_config_toml(&mut effective, &rendered.toml)
            .context("runtime_config_template_merge_failed")?;
    }
    for override_record in state
        .repo
        .list_runtime_config_overrides(Some(client_id))
        .await?
    {
        merge_runtime_config_toml(&mut effective, &override_record.toml)
            .context("runtime_config_override_merge_failed")?;
    }
    apply_enabled_tunnel_plans(state, client_id, &mut effective).await?;

    Ok(AgentRuntimeConfig {
        version,
        display_name: effective.display_name,
        backup: effective.backup,
        update: effective.update,
        execution: effective.execution,
        telemetry: effective.telemetry,
        network: effective.network,
        telemetry_light_secs: effective.telemetry_light_secs,
        telemetry_full_secs: effective.telemetry_full_secs,
        tags: effective.tags,
    })
}

async fn push_runtime_config_job(
    state: &AppState,
    operator: &AuthContext,
    client_id: String,
    reason: &str,
    version: u64,
    config: AgentRuntimeConfig,
) -> Result<CreateJobResponse, ApiError> {
    let request = CreateJobRequest {
        job_id: Some(Uuid::new_v4()),
        selector_expression: String::new(),
        target_client_ids: vec![client_id],
        destructive: false,
        confirmed: true,
        command: "runtime_config_sync".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::RuntimeConfigSync {
            desired_version: version,
            reason: reason.to_string(),
            config: Box::new(config),
        }),
        max_timeout_secs: Some(300),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };
    let (_, response) =
        create_job_from_internal_operator_mutation(state, operator, request).await?;
    Ok(response.0)
}

fn runtime_config_system_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "runtime-config-agent-request".to_string(),
            role: "system".to_string(),
            scopes: vec!["*".to_string()],
            preferences: OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Uuid::nil(),
    }
}

async fn apply_enabled_tunnel_plans(
    state: &AppState,
    client_id: &str,
    effective: &mut AgentConfig,
) -> Result<(), ApiError> {
    let plans = state.repo.list_tunnel_plans().await?;
    for plan in plans
        .into_iter()
        .filter(|plan| plan.enabled)
        .filter(|plan| plan.left_client_id == client_id || plan.right_client_id == client_id)
    {
        let endpoint_side = if plan.left_client_id == client_id {
            TunnelEndpointSide::Left
        } else {
            TunnelEndpointSide::Right
        };
        effective
            .network
            .runtime_status_telemetry_plans
            .push(AgentRuntimeStatusTelemetryPlan {
                plan_id: Some(plan.id.to_string()),
                endpoint_side,
                plan: plan.plan,
                traffic_source: AgentRuntimeTrafficSource::InterfaceCounters,
                traffic_command: None,
                latency_monitoring_enabled: effective.network.latency_monitoring_enabled,
                auto_ospf_enabled: effective.network.auto_ospf_enabled,
                auto_ospf_updater: effective.network.auto_ospf_updater.clone(),
            });
    }
    if !effective.network.runtime_status_telemetry_plans.is_empty() {
        effective.network.apply_enabled = true;
        effective.network.runtime_reconcile_enabled = true;
        effective.network.runtime_status_telemetry_enabled = true;
    }
    Ok(())
}

fn merge_runtime_config_toml(config: &mut AgentConfig, toml_document: &str) -> Result<()> {
    let patch: toml::Value =
        toml::from_str(toml_document).context("failed to parse runtime config template TOML")?;
    reject_server_managed_runtime_config_keys(&patch)?;
    let mut merged =
        toml::Value::try_from(&*config).context("failed to serialize base runtime config")?;
    merge_toml_value(&mut merged, patch)?;
    *config = merged
        .try_into()
        .context("failed to deserialize merged runtime config")?;
    validate_agent_config_shape(config)
        .map_err(|error| anyhow::anyhow!("failed to validate merged runtime config: {error}"))?;
    Ok(())
}

pub(crate) fn validate_runtime_config_patch_toml(toml_document: &str) -> Result<()> {
    let patch: toml::Value =
        toml::from_str(toml_document).context("failed to parse runtime config patch TOML")?;
    if !patch.is_table() {
        anyhow::bail!("runtime_config_patch_toml_invalid");
    }
    reject_server_managed_runtime_config_keys(&patch)?;
    let mut merged = toml::Value::try_from(AgentConfig::default())
        .context("failed to serialize base runtime config")?;
    merge_toml_value(&mut merged, patch)?;
    let config: AgentConfig = merged
        .try_into()
        .context("failed to deserialize runtime config patch")?;
    validate_agent_config_shape(&config)
        .map_err(|error| anyhow::anyhow!("failed to validate runtime config patch: {error}"))?;
    Ok(())
}

fn reject_server_managed_runtime_config_keys(patch: &toml::Value) -> Result<()> {
    let Some(table) = patch.as_table() else {
        anyhow::bail!("runtime_config_patch_toml_invalid");
    };
    const IMMUTABLE_TOP_LEVEL_KEYS: &[&str] = &[
        "client_id",
        "tcp_endpoints",
        "noise",
        "server_public_key",
        "secret",
        "auth",
    ];
    for key in IMMUTABLE_TOP_LEVEL_KEYS {
        if table.contains_key(*key) {
            anyhow::bail!("runtime_config_patch_bootstrap_field_forbidden");
        }
    }
    if table
        .get("network")
        .and_then(toml::Value::as_table)
        .is_some_and(|network| network.contains_key("runtime_status_telemetry_plans"))
    {
        anyhow::bail!("runtime_config_patch_managed_tunnel_plans_forbidden");
    }
    Ok(())
}

fn merge_toml_value(target: &mut toml::Value, patch: toml::Value) -> Result<()> {
    match (target, patch) {
        (toml::Value::Table(target), toml::Value::Table(patch)) => {
            for (key, value) in patch {
                if let Some(existing) = target.get_mut(&key) {
                    merge_toml_value(existing, value)?;
                } else {
                    target.insert(key, value);
                }
            }
            Ok(())
        }
        (target, patch) => {
            *target = patch;
            Ok(())
        }
    }
}

fn runtime_config_version() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(1)
        .max(1)
}
