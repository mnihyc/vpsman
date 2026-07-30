use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    runtime_config_content_hash, runtime_config_reconcile_scope_from_reason,
    validate_agent_config_shape, AgentConfig, AgentRuntimeConfig, AgentRuntimeStatusTelemetryPlan,
    AgentRuntimeTrafficSource, JobCommand, RuntimeConfigReconcileResource,
    RuntimeConfigReconcileScope, RuntimeTunnelManager, TunnelEndpointSide,
};

use crate::{
    error::ApiError,
    internal_operator::system_operator,
    model::{
        AgentView, AuthContext, CreateJobRequest, CreateJobResponse, RuntimeConfigDispatchView,
        TunnelPlanView,
    },
    routes_jobs::create_job_from_internal_operator_mutation,
    state::AppState,
};

static LAST_RUNTIME_CONFIG_VERSION: AtomicU64 = AtomicU64::new(0);
const AUTHORITATIVE_RUNTIME_CONFIG_SYNC_REASON: &str = "agent_reconnect_authoritative_sync";
const AUTHORITATIVE_PORT_FORWARDING_SYNC_REASON: &str =
    "agent_reconnect_authoritative_port_forwarding_sync";
const PORT_FORWARDING_RECONNECT_SYNC_REASON: &str = "agent_reconnect_port_forwarding_sync";
const RUNTIME_TUNNELS_RECONNECT_SYNC_REASON: &str = "agent_reconnect_runtime_tunnels_sync";

pub(crate) async fn dispatch_runtime_config_for_clients(
    state: &AppState,
    operator: &AuthContext,
    client_ids: impl IntoIterator<Item = String>,
    reason: &str,
) -> Vec<RuntimeConfigDispatchView> {
    let clients = normalized_runtime_config_clients(client_ids);
    let known_clients = match state.repo.list_agents().await {
        Ok(agents) => agents
            .into_iter()
            .map(|agent| agent.id)
            .collect::<BTreeSet<_>>(),
        Err(error) => {
            let error = ApiError::from(error);
            let message = operator_dispatch_error(&error, "Runtime apply target lookup");
            return clients
                .into_iter()
                .map(|client_id| RuntimeConfigDispatchView {
                    client_id,
                    status: "queue_failed".to_string(),
                    job_id: None,
                    error: Some(message.clone()),
                })
                .collect();
        }
    };

    let mut outcomes = Vec::with_capacity(clients.len());
    for client_id in clients {
        if !known_clients.contains(&client_id) {
            outcomes.push(RuntimeConfigDispatchView {
                client_id,
                status: "not_queued".to_string(),
                job_id: None,
                error: Some("VPS is no longer available".to_string()),
            });
            continue;
        }
        match push_runtime_config_for_known_client(state, operator, client_id.clone(), reason).await
        {
            Ok(job) => outcomes.push(RuntimeConfigDispatchView {
                client_id,
                status: "queued".to_string(),
                job_id: Some(job.job_id),
                error: None,
            }),
            Err(error) => {
                warn!(
                    ?error,
                    %client_id,
                    %reason,
                    "failed to queue composed runtime configuration"
                );
                outcomes.push(RuntimeConfigDispatchView {
                    client_id,
                    status: "queue_failed".to_string(),
                    job_id: None,
                    error: Some(operator_dispatch_error(&error, "Runtime apply job")),
                });
            }
        }
    }
    outcomes
}

pub(crate) fn operator_dispatch_error(error: &ApiError, operation: &str) -> String {
    if let Some(message) = error.public_message.as_deref() {
        return format!(
            "{operation} could not be queued: {message}. Desired state remains saved; refresh target state and retry"
        );
    }
    if error.status.is_server_error() {
        return format!(
            "{operation} could not be queued because the server failed while creating it. Desired state remains saved; inspect API logs and retry"
        );
    }
    format!(
        "{operation} could not be queued because the server rejected it: {}. Desired state remains saved; refresh target state, correct the reported conflict, and retry",
        error.code.replace('_', " ")
    )
}

fn normalized_runtime_config_clients(
    client_ids: impl IntoIterator<Item = String>,
) -> BTreeSet<String> {
    client_ids
        .into_iter()
        .filter(|client_id| !client_id.trim().is_empty())
        .collect()
}

async fn push_runtime_config_for_known_client(
    state: &AppState,
    operator: &AuthContext,
    client_id: String,
    reason: &str,
) -> Result<CreateJobResponse, ApiError> {
    let version = next_runtime_config_version(state, &client_id).await?;
    let config = compose_runtime_config(state, &client_id, version).await?;
    push_runtime_config_job(state, operator, client_id, reason, version, config).await
}

pub(crate) async fn request_runtime_config_reload_for_agent(
    state: &AppState,
    client_id: &str,
    current_content_hash: &str,
    reason: &str,
    reconcile_scope: RuntimeConfigReconcileScope,
) -> Result<Vec<CreateJobResponse>, ApiError> {
    let mut config = compose_runtime_config(state, client_id, 1).await?;
    let desired_content_hash = runtime_config_content_hash(&config)
        .map_err(|error| ApiError::from(anyhow::anyhow!("runtime config hash failed: {error}")))?;
    if !reconcile_scope.requires_reconcile()
        && desired_content_hash.eq_ignore_ascii_case(current_content_hash.trim())
    {
        state
            .repo
            .promote_runtime_config_apply_from_agent_hash(client_id, &desired_content_hash)
            .await?;
        return Ok(Vec::new());
    }
    if let Some(pending) = state
        .repo
        .runtime_config_pending_state_for_client(client_id)
        .await?
    {
        let same_queued_content = pending.pending_status.as_deref() == Some("queued")
            && pending
                .pending_content_hash
                .as_deref()
                .is_some_and(|hash| hash.eq_ignore_ascii_case(&desired_content_hash));
        let pending_scope = pending
            .pending_reason
            .as_deref()
            .map(runtime_config_reconcile_scope_from_reason)
            .unwrap_or_default();
        if same_queued_content
            && (!reconcile_scope.requires_reconcile() || pending_scope.covers(&reconcile_scope))
        {
            return Ok(Vec::new());
        }
    }
    let version = next_runtime_config_version(state, client_id).await?;
    config.version = version;
    let operator = system_operator("runtime-config-agent-request");
    let sync_reason = runtime_config_reload_reason(&reconcile_scope, reason);
    push_runtime_config_job(
        state,
        &operator,
        client_id.to_string(),
        sync_reason,
        version,
        config,
    )
    .await
    .map(|response| vec![response])
}

fn runtime_config_reload_reason<'a>(
    scope: &RuntimeConfigReconcileScope,
    fallback: &'a str,
) -> &'a str {
    if (scope.authoritative || scope.resources.len() > 1)
        && scope
            .resources
            .contains(&RuntimeConfigReconcileResource::PortForwarding)
    {
        AUTHORITATIVE_PORT_FORWARDING_SYNC_REASON
    } else if scope.authoritative || scope.resources.len() > 1 {
        // The legacy command wire has no scope field. A full reconcile is the safe
        // superset when an older agent receives a multi-resource repair request.
        AUTHORITATIVE_RUNTIME_CONFIG_SYNC_REASON
    } else if scope
        .resources
        .contains(&RuntimeConfigReconcileResource::PortForwarding)
    {
        PORT_FORWARDING_RECONNECT_SYNC_REASON
    } else if scope
        .resources
        .contains(&RuntimeConfigReconcileResource::RuntimeTunnels)
    {
        RUNTIME_TUNNELS_RECONNECT_SYNC_REASON
    } else {
        fallback
    }
}

pub(crate) async fn compose_runtime_config(
    state: &AppState,
    client_id: &str,
    version: u64,
) -> Result<AgentRuntimeConfig, ApiError> {
    let agents = state
        .repo
        .list_agents_for_client_ids(&[client_id.to_string()])
        .await?;
    let agent = agents
        .first()
        .ok_or_else(|| ApiError::not_found("runtime_config_client_not_found"))?;
    compose_runtime_config_for_agent(state, agent, version).await
}

pub(crate) async fn compose_runtime_config_for_agent(
    state: &AppState,
    agent: &AgentView,
    version: u64,
) -> Result<AgentRuntimeConfig, ApiError> {
    let preset_toml = state
        .repo
        .render_configuration_preset_patch_toml(&agent.id)
        .await?;
    let tunnel_plans = state.repo.list_tunnel_plans().await?;
    compose_runtime_config_for_agent_with_read_model(
        state,
        agent,
        version,
        &preset_toml,
        &tunnel_plans,
    )
    .await
}

pub(crate) async fn compose_runtime_config_for_agent_with_read_model(
    state: &AppState,
    agent: &AgentView,
    version: u64,
    preset_toml: &str,
    tunnel_plans: &[TunnelPlanView],
) -> Result<AgentRuntimeConfig, ApiError> {
    let mut effective = AgentConfig {
        client_id: agent.id.clone(),
        display_name: agent.display_name.clone(),
        tags: agent.tags.clone(),
        ..AgentConfig::default()
    };

    if !preset_toml.trim().is_empty() {
        merge_configuration_preset_toml(&mut effective, preset_toml)
            .context("runtime_config_preset_merge_failed")?;
    }
    for override_record in state
        .repo
        .list_runtime_config_overrides(Some(&agent.id))
        .await?
    {
        merge_runtime_config_toml(&mut effective, &override_record.toml)
            .context("runtime_config_override_merge_failed")?;
    }
    apply_enabled_tunnel_plans(state, &agent.id, tunnel_plans, &mut effective).await?;
    effective.network.port_forwarding = state
        .repo
        .port_forwarding_config_for_client(&agent.id)
        .await?;
    validate_agent_config_shape(&effective)
        .map_err(|error| anyhow::anyhow!("composed_runtime_config_invalid:{error}"))?;

    Ok(AgentRuntimeConfig {
        version,
        display_name: effective.display_name,
        backup: effective.backup,
        update: effective.update,
        execution: effective.execution,
        telemetry: effective.telemetry,
        network: effective.network,
        telemetry_interval_secs: effective.telemetry_interval_secs,
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
    let job_id = Uuid::new_v4();
    let request = CreateJobRequest {
        job_id: Some(job_id),
        selector_expression: String::new(),
        target_client_ids: vec![client_id.clone()],
        destructive: false,
        confirmed: true,
        command: "runtime_config_sync".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::RuntimeConfigSync {
            desired_version: version,
            reason: reason.to_string(),
            config: Box::new(config.clone()),
        }),
        max_timeout_secs: Some(300),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
        rollout: None,
    };
    let (status, response) =
        create_job_from_internal_operator_mutation(state, operator, request).await?;
    let response = response.0;
    if !status.is_success() {
        return Err(ApiError {
            status,
            code: "runtime_config_job_not_queued",
            error: anyhow::anyhow!(
                "runtime config apply job was not queued (status={})",
                response.status
            ),
            public_message: Some(format!(
                "Runtime configuration was saved, but its apply job was not queued ({})",
                response.status
            )),
        });
    }
    Ok(response)
}

async fn apply_enabled_tunnel_plans(
    state: &AppState,
    client_id: &str,
    plans: &[TunnelPlanView],
    effective: &mut AgentConfig,
) -> Result<(), ApiError> {
    let mut has_mutating_plan = false;
    for plan in plans
        .iter()
        .filter(|plan| plan.enabled)
        .filter(|plan| plan.left_client_id == client_id || plan.right_client_id == client_id)
    {
        has_mutating_plan |=
            plan.plan.runtime_control.manager != RuntimeTunnelManager::ExternalObserved;
        let endpoint_side = if plan.left_client_id == client_id {
            TunnelEndpointSide::Left
        } else {
            TunnelEndpointSide::Right
        };
        let runtime_adapter =
            if plan.plan.runtime_control.manager == RuntimeTunnelManager::ExternalManagedAdapter {
                let definition_id = match endpoint_side {
                    TunnelEndpointSide::Left => plan
                        .plan
                        .runtime_control
                        .left_adapter_definition_id
                        .as_deref(),
                    TunnelEndpointSide::Right => plan
                        .plan
                        .runtime_control
                        .right_adapter_definition_id
                        .as_deref(),
                }
                .ok_or_else(|| ApiError::conflict("runtime_tunnel_adapter_definition_required"))?;
                Some(
                    state
                        .repo
                        .resolve_runtime_tunnel_adapter(definition_id)
                        .await
                        .map_err(ApiError::from)?,
                )
            } else {
                None
            };
        effective
            .network
            .runtime_status_telemetry_plans
            .push(AgentRuntimeStatusTelemetryPlan {
                plan_id: Some(plan.id.to_string()),
                endpoint_side,
                plan: plan.plan.clone(),
                runtime_adapter,
                traffic_source: if effective.network.runtime_vnstat_argv.is_empty() {
                    AgentRuntimeTrafficSource::InterfaceCounters
                } else {
                    AgentRuntimeTrafficSource::Vnstat
                },
                traffic_command: None,
                latency_monitoring_enabled: effective.network.latency_monitoring_enabled,
            });
    }
    if !effective.network.runtime_status_telemetry_plans.is_empty() {
        effective.network.apply_enabled |= has_mutating_plan;
        effective.network.runtime_reconcile_enabled = true;
        effective.network.runtime_status_telemetry_enabled = true;
    }
    Ok(())
}

fn merge_runtime_config_toml(config: &mut AgentConfig, toml_document: &str) -> Result<()> {
    let patch: toml::Value =
        toml::from_str(toml_document).context("failed to parse runtime config patch TOML")?;
    reject_server_managed_runtime_config_keys(&patch)?;
    reject_configuration_preset_owned_runtime_config_keys(&patch)?;
    merge_runtime_config_value(config, patch)
}

fn merge_configuration_preset_toml(config: &mut AgentConfig, toml_document: &str) -> Result<()> {
    let patch: toml::Value =
        toml::from_str(toml_document).context("failed to parse configuration preset TOML")?;
    reject_server_managed_runtime_config_keys(&patch)?;
    merge_runtime_config_value(config, patch)
}

fn merge_runtime_config_value(config: &mut AgentConfig, patch: toml::Value) -> Result<()> {
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
    reject_configuration_preset_owned_runtime_config_keys(&patch)?;
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
    if table
        .get("network")
        .and_then(toml::Value::as_table)
        .is_some_and(|network| network.contains_key("port_forwarding"))
    {
        anyhow::bail!("runtime_config_patch_managed_port_forwarding_forbidden");
    }
    Ok(())
}

fn reject_configuration_preset_owned_runtime_config_keys(patch: &toml::Value) -> Result<()> {
    let Some(table) = patch.as_table() else {
        anyhow::bail!("runtime_config_patch_toml_invalid");
    };
    const TELEMETRY_KEYS: &[&str] = &[
        "source",
        "proc_root",
        "sys_class_net_dir",
        "hostname_file",
        "os_release_file",
        "custom_metrics_command",
    ];
    const EXECUTION_KEYS: &[&str] = &[
        "shell_script_argv",
        "working_directory",
        "environment_policy",
        "environment_keep",
        "environment_set",
        "pty_policy",
        "process_cleanup",
        "user_sessions_source",
        "user_sessions_command",
        "process_inventory_source",
        "process_proc_root",
        "process_inventory_command",
    ];
    const NETWORK_KEYS: &[&str] = &[
        "probe_ping_argv",
        "runtime_vnstat_argv",
        "ospf_status_command",
        "ospf_update_command",
    ];
    for (section, keys) in [
        ("telemetry", TELEMETRY_KEYS),
        ("execution", EXECUTION_KEYS),
        ("network", NETWORK_KEYS),
    ] {
        if table
            .get(section)
            .and_then(toml::Value::as_table)
            .is_some_and(|values| keys.iter().any(|key| values.contains_key(*key)))
        {
            anyhow::bail!("runtime_config_patch_configuration_preset_field_forbidden");
        }
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

async fn next_runtime_config_version(state: &AppState, client_id: &str) -> Result<u64, ApiError> {
    let floor = state
        .repo
        .list_runtime_config_apply_states(Some(client_id))
        .await?
        .into_iter()
        .flat_map(|record| [record.applied_version, record.pending_version])
        .flatten()
        .max()
        .unwrap_or(0);
    runtime_config_version_after(floor).map_err(ApiError::from)
}

fn runtime_config_version_after(floor: u64) -> Result<u64> {
    const MAX_PERSISTED_VERSION: u64 = i64::MAX as u64;
    anyhow::ensure!(
        floor < MAX_PERSISTED_VERSION,
        "runtime config version space exhausted"
    );
    let wall_clock = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros().min(u128::from(MAX_PERSISTED_VERSION)) as u64)
        .unwrap_or(1)
        .max(1);
    let minimum = wall_clock.max(floor + 1);
    loop {
        let previous = LAST_RUNTIME_CONFIG_VERSION.load(Ordering::Relaxed);
        let candidate = minimum.max(previous.saturating_add(1));
        anyhow::ensure!(
            candidate <= MAX_PERSISTED_VERSION,
            "runtime config version space exhausted"
        );
        if LAST_RUNTIME_CONFIG_VERSION
            .compare_exchange(previous, candidate, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
        {
            return Ok(candidate);
        }
    }
}

#[cfg(test)]
mod version_tests {
    use super::{
        operator_dispatch_error, runtime_config_reload_reason, runtime_config_version_after,
    };
    use crate::error::ApiError;
    use vpsman_common::{RuntimeConfigReconcileResource, RuntimeConfigReconcileScope};

    fn scope(
        authoritative: bool,
        resources: &[RuntimeConfigReconcileResource],
    ) -> RuntimeConfigReconcileScope {
        RuntimeConfigReconcileScope {
            authoritative,
            resources: resources.iter().copied().collect(),
        }
    }

    #[test]
    fn runtime_config_versions_are_strictly_monotonic_above_the_persisted_floor() {
        let first = runtime_config_version_after(100).unwrap();
        let second = runtime_config_version_after(first).unwrap();
        assert!(first > 100);
        assert!(second > first);
    }

    #[test]
    fn reconnect_reason_projects_typed_scope_onto_the_rolling_update_wire() {
        assert_eq!(
            runtime_config_reload_reason(
                &scope(true, &[RuntimeConfigReconcileResource::PortForwarding],),
                "fallback",
            ),
            "agent_reconnect_authoritative_port_forwarding_sync"
        );
        assert_eq!(
            runtime_config_reload_reason(&scope(true, &[]), "fallback"),
            "agent_reconnect_authoritative_sync"
        );
        assert_eq!(
            runtime_config_reload_reason(
                &scope(false, &[RuntimeConfigReconcileResource::PortForwarding],),
                "fallback",
            ),
            "agent_reconnect_port_forwarding_sync"
        );
        assert_eq!(
            runtime_config_reload_reason(
                &scope(false, &[RuntimeConfigReconcileResource::RuntimeTunnels],),
                "fallback",
            ),
            "agent_reconnect_runtime_tunnels_sync"
        );
        assert_eq!(
            runtime_config_reload_reason(&scope(false, &[]), "fallback"),
            "fallback"
        );
    }

    #[test]
    fn dispatch_errors_explain_impact_and_recovery_without_leaking_internal_details() {
        let internal = ApiError::from(anyhow::anyhow!("private database detail"));
        let internal_message = operator_dispatch_error(&internal, "Runtime apply job");
        assert!(internal_message.contains("Desired state remains saved"));
        assert!(internal_message.contains("inspect API logs and retry"));
        assert!(!internal_message.contains("private database detail"));

        let conflict = ApiError::conflict("agent_command_queue_full");
        let conflict_message = operator_dispatch_error(&conflict, "Runtime apply job");
        assert!(conflict_message.contains("agent command queue full"));
        assert!(conflict_message.contains("refresh target state"));

        let public = ApiError::bad_request_with_message(
            "runtime_config_invalid",
            "The rendered config is invalid for this VPS",
        );
        let public_message = operator_dispatch_error(&public, "Runtime apply job");
        assert!(public_message.contains("The rendered config is invalid for this VPS"));
        assert!(public_message.contains("Desired state remains saved"));
    }
}
