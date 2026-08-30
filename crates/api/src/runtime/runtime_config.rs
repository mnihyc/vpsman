use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use anyhow::{Context, Result};
use sqlx::postgres::PgListener;
use tokio::sync::oneshot;
use tracing::warn;
use uuid::Uuid;
use vpsman_common::{
    runtime_config_content_hash, runtime_config_reconcile_scope_from_reason,
    tunnel_runtime_evidence_identity_hash, tunnel_topology_identity_hash,
    validate_agent_config_shape, AgentConfig, AgentNetworkConfig, AgentPingTarget,
    AgentPortForwardingConfig, AgentRuntimeConfig, AgentRuntimeStatusTelemetryPlan, JobCommand,
    RuntimeConfigReconcileResource, RuntimeConfigReconcileScope, RuntimeTunnelAdapterCommands,
    RuntimeTunnelManager, TunnelEndpointSide, TunnelKind,
};

use crate::{
    error::ApiError,
    internal_operator::system_operator,
    model::{
        AgentView, AuthContext, CreateJobRequest, CreateJobResponse, RuntimeConfigDispatchView,
        TunnelPlanView,
    },
    repository::Repository,
    repository_runtime_config::ClaimedRuntimeConfigReconciliation,
    routes_jobs::create_job_from_runtime_config_reconciliation,
    state::AppState,
};

const AUTHORITATIVE_RUNTIME_CONFIG_SYNC_REASON: &str = "agent_reconnect_authoritative_sync";
const AUTHORITATIVE_PORT_FORWARDING_SYNC_REASON: &str =
    "agent_reconnect_authoritative_port_forwarding_sync";
const PORT_FORWARDING_RECONNECT_SYNC_REASON: &str = "agent_reconnect_port_forwarding_sync";
const RUNTIME_TUNNELS_RECONNECT_SYNC_REASON: &str = "agent_reconnect_runtime_tunnels_sync";
// The lease fences ownership; it never bounds composition or job creation.
// Renewal at one third leaves two missed-heartbeat margins before takeover.
const RUNTIME_CONFIG_RECONCILE_LEASE_SECS: i32 = 30;
const RUNTIME_CONFIG_RECONCILE_RENEW_SECS: u64 = 10;
// NOTIFY is the normal wake path. This interval is only missed-notification or
// listener-reconnect recovery and does not limit a non-empty drain.
const RUNTIME_CONFIG_RECONCILE_RECOVERY_POLL_SECS: u64 = 5;
// A failed source document remains durable. A short retry avoids a hot error
// loop; any new source mutation resets it to immediately due.
const RUNTIME_CONFIG_RECONCILE_ERROR_RETRY_SECS: i32 = 5;

enum RuntimeConfigReconcileOutcome {
    Job(Box<CreateJobResponse>),
    Unchanged,
}

struct AbortTaskOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortTaskOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) async fn dispatch_runtime_config_for_clients(
    state: &AppState,
    operator: &AuthContext,
    client_ids: impl IntoIterator<Item = String>,
    reason: &str,
) -> Vec<RuntimeConfigDispatchView> {
    let clients = normalized_runtime_config_clients(client_ids);
    let client_values = clients.iter().cloned().collect::<Vec<_>>();
    let known_clients = match state.repo.list_agents_for_client_ids(&client_values).await {
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

    let known = clients
        .iter()
        .filter(|client_id| known_clients.contains(*client_id))
        .cloned()
        .collect::<Vec<_>>();
    if let Err(error) = state
        .repo
        .ensure_runtime_config_reconciliations(&known, reason, Some(operator.operator.id))
        .await
    {
        let error = ApiError::from(error);
        let message = operator_dispatch_error(&error, "Runtime reconciliation");
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

    clients
        .into_iter()
        .map(|client_id| {
            if known_clients.contains(&client_id) {
                // The source mutation and its trigger own the durable revision.
                // This response path only reports the queued handoff; the sole
                // reconciler claims, composes, and creates any runtime job.
                RuntimeConfigDispatchView {
                    client_id,
                    status: "queued".to_string(),
                    job_id: None,
                    error: None,
                }
            } else {
                RuntimeConfigDispatchView {
                    client_id,
                    status: "not_queued".to_string(),
                    job_id: None,
                    error: Some("VPS is no longer available".to_string()),
                }
            }
        })
        .collect()
}

pub(crate) fn spawn_runtime_config_reconciler(state: AppState) -> tokio::task::JoinHandle<()> {
    let pool = match &state.repo {
        Repository::Postgres(pool) => pool.clone(),
    };
    tokio::spawn(async move {
        loop {
            let mut listener = match PgListener::connect_with(&pool).await {
                Ok(listener) => listener,
                Err(error) => {
                    warn!(%error, "runtime config reconcile listener connection failed");
                    tokio::time::sleep(Duration::from_secs(
                        RUNTIME_CONFIG_RECONCILE_RECOVERY_POLL_SECS,
                    ))
                    .await;
                    continue;
                }
            };
            if let Err(error) = listener.listen("runtime_config_reconcile").await {
                warn!(%error, "runtime config reconcile listener registration failed");
                tokio::time::sleep(Duration::from_secs(
                    RUNTIME_CONFIG_RECONCILE_RECOVERY_POLL_SECS,
                ))
                .await;
                continue;
            }
            loop {
                match drain_runtime_config_reconciliations(&state).await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        warn!(?error, "runtime config reconciliation drain failed");
                    }
                }
                match tokio::time::timeout(
                    Duration::from_secs(RUNTIME_CONFIG_RECONCILE_RECOVERY_POLL_SECS),
                    listener.recv(),
                )
                .await
                {
                    Ok(Ok(_)) | Err(_) => {}
                    Ok(Err(error)) => {
                        warn!(%error, "runtime config reconcile listener disconnected");
                        break;
                    }
                }
            }
        }
    })
}

async fn drain_runtime_config_reconciliations(state: &AppState) -> Result<bool, ApiError> {
    let Some(claim) = state
        .repo
        .claim_runtime_config_reconciliation(None, RUNTIME_CONFIG_RECONCILE_LEASE_SECS)
        .await?
    else {
        return Ok(false);
    };
    let operator = system_operator("runtime-config-reconciler");
    if let Err(error) = reconcile_runtime_config_claim(state, &operator, claim).await {
        warn!(
            ?error,
            "durable runtime configuration reconciliation failed"
        );
    }
    Ok(true)
}

async fn reconcile_runtime_config_claim(
    state: &AppState,
    operator: &AuthContext,
    claim: ClaimedRuntimeConfigReconciliation,
) -> Result<RuntimeConfigReconcileOutcome, ApiError> {
    let heartbeat_claim = claim.clone();
    let (stop_tx, mut stop_rx) = oneshot::channel();
    let heartbeat = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(RUNTIME_CONFIG_RECONCILE_RENEW_SECS)) => {
                    if !heartbeat_claim
                        .renew(RUNTIME_CONFIG_RECONCILE_LEASE_SECS)
                        .await
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
                _ = &mut stop_rx => break,
            }
        }
    });
    let task_state = state.clone();
    let task_operator = operator.clone();
    let task_client_id = claim.client_id.clone();
    let task_reason = claim.reason.clone();
    let version = claim.apply_version;
    let task_claim = claim.clone();
    let mut push_task = AbortTaskOnDrop(tokio::spawn(async move {
        let config = compose_runtime_config(&task_state, &task_client_id, version).await?;
        let content_hash = runtime_config_content_hash(&config).map_err(|error| {
            ApiError::from(anyhow::anyhow!("runtime config hash failed: {error}"))
        })?;
        if task_claim
            .acknowledge_if_content_current(&content_hash)
            .await?
            .is_some()
        {
            return Ok(RuntimeConfigReconcileOutcome::Unchanged);
        }
        push_runtime_config_job(
            &task_state,
            &task_operator,
            task_client_id,
            &task_reason,
            task_claim.desired_revision,
            version,
            config,
            task_claim.claim_token,
        )
        .await
        .map(|response| RuntimeConfigReconcileOutcome::Job(Box::new(response)))
    }));
    let result = match (&mut push_task.0).await {
        Ok(result) => result,
        Err(error) => Err(ApiError::internal(
            "runtime_config_dispatch_task_failed",
            "The runtime configuration could not be queued.",
            error.into(),
        )),
    };
    let _ = stop_tx.send(());
    let _ = heartbeat.await;
    if let Err(error) = &result {
        let _ = claim
            .defer(
                &error.code.replace('_', " "),
                RUNTIME_CONFIG_RECONCILE_ERROR_RETRY_SECS,
            )
            .await;
    }
    result
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

pub(crate) fn redact_runtime_tunnel_credentials(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            fields.remove("builtin_credentials");
            for value in fields.values_mut() {
                redact_runtime_tunnel_credentials(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_runtime_tunnel_credentials(value);
            }
        }
        _ => {}
    }
}

pub(crate) fn clear_runtime_tunnel_credentials(config: &mut AgentNetworkConfig) {
    for plan in &mut config.runtime_status_telemetry_plans {
        plan.builtin_credentials = None;
    }
}

fn normalized_runtime_config_clients(
    client_ids: impl IntoIterator<Item = String>,
) -> BTreeSet<String> {
    client_ids
        .into_iter()
        .filter(|client_id| !client_id.trim().is_empty())
        .collect()
}

pub(crate) async fn request_runtime_config_reload_for_agent(
    state: &AppState,
    client_id: &str,
    current_content_hash: &str,
    reason: &str,
    reconcile_scope: RuntimeConfigReconcileScope,
) -> Result<Vec<CreateJobResponse>, ApiError> {
    let config = compose_runtime_config(state, client_id, 1).await?;
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
    let operator = system_operator("runtime-config-agent-request");
    let sync_reason = runtime_config_reload_reason(&reconcile_scope, reason);
    state
        .repo
        .enqueue_runtime_config_reconciliations(&[client_id.to_string()], sync_reason, None)
        .await?;
    let claim = state
        .repo
        .claim_runtime_config_reconciliation(Some(client_id), RUNTIME_CONFIG_RECONCILE_LEASE_SECS)
        .await?;
    match claim {
        Some(claim) => reconcile_runtime_config_claim(state, &operator, claim)
            .await
            .map(|outcome| match outcome {
                RuntimeConfigReconcileOutcome::Job(response) => vec![*response],
                RuntimeConfigReconcileOutcome::Unchanged => Vec::new(),
            }),
        None => Ok(Vec::new()),
    }
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
        // The persisted job reason carries the requested reconciliation scope
        // through dispatch and acknowledgement.
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
    let override_toml = state
        .repo
        .list_runtime_config_overrides(Some(&agent.id))
        .await?
        .into_iter()
        .next()
        .map(|record| record.toml);
    compose_runtime_config_for_agent_with_read_model_and_override(
        state,
        agent,
        version,
        preset_toml,
        tunnel_plans,
        override_toml.as_deref(),
    )
    .await
}

/// Composes the authoritative runtime document using an explicit per-VPS
/// override. `None` intentionally means "inherit everything" and does not
/// consult the stored override row. The configuration workspace uses this to
/// preview replacements and to recover from malformed stored override text.
pub(crate) async fn compose_runtime_config_for_agent_with_read_model_and_override(
    state: &AppState,
    agent: &AgentView,
    version: u64,
    preset_toml: &str,
    tunnel_plans: &[TunnelPlanView],
    override_toml: Option<&str>,
) -> Result<AgentRuntimeConfig, ApiError> {
    let managed = load_runtime_config_managed_inputs(state, &agent.id, tunnel_plans).await?;
    compose_runtime_config_for_agent_with_managed_inputs(
        agent,
        version,
        preset_toml,
        tunnel_plans,
        override_toml,
        &managed,
    )
}

pub(crate) struct RuntimeConfigManagedInputs {
    port_forwarding: AgentPortForwardingConfig,
    ping_targets: Vec<AgentPingTarget>,
    runtime_adapters: BTreeMap<String, RuntimeTunnelAdapterCommands>,
}

pub(crate) async fn load_runtime_config_managed_inputs(
    state: &AppState,
    client_id: &str,
    tunnel_plans: &[TunnelPlanView],
) -> Result<RuntimeConfigManagedInputs, ApiError> {
    let mut definition_ids = BTreeSet::new();
    for plan in tunnel_plans
        .iter()
        .filter(|plan| plan.enabled)
        .filter(|plan| plan.left_client_id == client_id || plan.right_client_id == client_id)
        .filter(|plan| plan.plan.runtime_control.manager == RuntimeTunnelManager::CustomAdapter)
    {
        let definition_id = if plan.left_client_id == client_id {
            plan.plan
                .runtime_control
                .left_adapter_definition_id
                .as_deref()
        } else {
            plan.plan
                .runtime_control
                .right_adapter_definition_id
                .as_deref()
        }
        .ok_or_else(|| ApiError::conflict("runtime_tunnel_adapter_definition_required"))?;
        definition_ids.insert(definition_id.to_string());
    }
    let mut runtime_adapters = BTreeMap::new();
    for definition_id in definition_ids {
        runtime_adapters.insert(
            definition_id.clone(),
            state
                .repo
                .resolve_runtime_tunnel_adapter(&definition_id)
                .await
                .map_err(ApiError::from)?,
        );
    }
    Ok(RuntimeConfigManagedInputs {
        port_forwarding: state
            .repo
            .port_forwarding_config_for_client(client_id)
            .await?,
        ping_targets: state.repo.ping_targets_for_client(client_id).await?,
        runtime_adapters,
    })
}

pub(crate) fn compose_runtime_config_for_agent_with_managed_inputs(
    agent: &AgentView,
    version: u64,
    preset_toml: &str,
    tunnel_plans: &[TunnelPlanView],
    override_toml: Option<&str>,
    managed: &RuntimeConfigManagedInputs,
) -> Result<AgentRuntimeConfig, ApiError> {
    let mut effective = AgentConfig {
        client_id: agent.id.clone(),
        ..AgentConfig::default()
    };

    if !preset_toml.trim().is_empty() {
        merge_configuration_preset_toml(&mut effective, preset_toml)
            .context("runtime_config_preset_merge_failed")?;
    }
    if let Some(override_toml) = override_toml {
        merge_runtime_config_toml(&mut effective, override_toml)
            .context("runtime_config_override_merge_failed")?;
    }
    apply_enabled_tunnel_plans(
        &agent.id,
        tunnel_plans,
        &managed.runtime_adapters,
        &mut effective,
    )?;
    effective.network.port_forwarding = managed.port_forwarding.clone();
    effective.network.ping_targets = managed.ping_targets.clone();
    validate_agent_config_shape(&effective)
        .map_err(|error| anyhow::anyhow!("composed_runtime_config_invalid:{error}"))?;

    Ok(AgentRuntimeConfig {
        version,
        backup: effective.backup,
        update: effective.update,
        execution: effective.execution,
        telemetry: effective.telemetry,
        network: effective.network,
        telemetry_interval_secs: effective.telemetry_interval_secs,
    })
}

async fn push_runtime_config_job(
    state: &AppState,
    operator: &AuthContext,
    client_id: String,
    reason: &str,
    desired_revision: i64,
    version: u64,
    config: AgentRuntimeConfig,
    claim_token: Uuid,
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
    let (status, response) = create_job_from_runtime_config_reconciliation(
        state,
        operator,
        request,
        desired_revision,
        claim_token,
    )
    .await?;
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

fn apply_enabled_tunnel_plans(
    client_id: &str,
    plans: &[TunnelPlanView],
    runtime_adapters: &BTreeMap<String, RuntimeTunnelAdapterCommands>,
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
            if plan.plan.runtime_control.manager == RuntimeTunnelManager::CustomAdapter {
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
                    runtime_adapters
                        .get(definition_id)
                        .cloned()
                        .ok_or_else(|| {
                            ApiError::conflict("runtime_tunnel_adapter_definition_required")
                        })?,
                )
            } else {
                None
            };
        let builtin_credential_generation = plan
            .builtin_credentials
            .as_ref()
            .map(vpsman_common::TunnelBuiltinCredentials::generation);
        let builtin_credentials = if plan.plan.runtime_control.manager
            == RuntimeTunnelManager::AgentBuiltin
            && matches!(plan.plan.kind, TunnelKind::Wireguard | TunnelKind::Openvpn)
        {
            Some(
                plan.builtin_credentials
                    .as_ref()
                    .ok_or_else(|| {
                        ApiError::conflict("runtime_tunnel_builtin_credentials_required")
                    })?
                    .endpoint(endpoint_side),
            )
        } else {
            None
        };
        effective
            .network
            .runtime_status_telemetry_plans
            .push(AgentRuntimeStatusTelemetryPlan {
                plan_id: Some(plan.id.to_string()),
                topology_identity_hash: tunnel_topology_identity_hash(plan.id, &plan.plan),
                runtime_evidence_identity_hash: tunnel_runtime_evidence_identity_hash(
                    plan.id,
                    &plan.plan,
                    builtin_credential_generation,
                ),
                endpoint_side,
                plan: plan.plan.clone(),
                builtin_credentials,
                runtime_adapter,
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

fn reject_server_managed_runtime_config_keys(patch: &toml::Value) -> Result<()> {
    let Some(table) = patch.as_table() else {
        anyhow::bail!("runtime_config_patch_toml_invalid");
    };
    const IMMUTABLE_TOP_LEVEL_KEYS: &[&str] = &[
        "client_id",
        "display_name",
        "tags",
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
    if table
        .get("network")
        .and_then(toml::Value::as_table)
        .is_some_and(|network| network.contains_key("ping_targets"))
    {
        anyhow::bail!("runtime_config_patch_managed_ping_targets_forbidden");
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

#[cfg(test)]
#[path = "tests_runtime_config.rs"]
mod version_tests;
