use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use tokio::time::{self, Duration};
use vpsman_common::{
    backend_config_proof_payload, payload_hash, render_backend_config_for_endpoint,
    render_tunnel_endpoint_config, AgentConfig, CommandOutput, OutputStream, TunnelBackendFile,
    TunnelConfigBackend, TunnelEndpointConfig, TunnelEndpointSide, TunnelPlan, MANAGED_BIRD2_FILE,
    MANAGED_IFUPDOWN_FILE, MANAGED_NETPLAN_FILE, MANAGED_SYSTEMD_NETWORKD_NETDEV_FILE,
    MANAGED_SYSTEMD_NETWORKD_NETWORK_FILE,
};

use crate::network_hooks::{
    bird2_reload_hook_specs, bird2_validation_hook_specs, pre_rollback_hook_specs,
    reload_hook_specs, run_network_hooks, validation_hook_specs, NetworkHookContext,
};
use crate::network_runtime::{
    execute_runtime_tunnel_reconcile_report, execute_runtime_tunnel_remove_report,
    NetworkRuntimeReconcileInput, NetworkRuntimeRemoveInput,
};

const MAX_MANAGED_NETWORK_FILE_BYTES: u64 = 256 * 1024;

pub(crate) struct NetworkApplyInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) config_backend: TunnelConfigBackend,
    pub(crate) config_sha256_hex: Option<&'a str>,
    pub(crate) ifupdown_sha256_hex: &'a str,
    pub(crate) bird2_sha256_hex: &'a str,
    pub(crate) timeout_secs: u64,
}

pub(crate) struct NetworkRollbackInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) timeout_secs: u64,
}

pub(crate) struct NetworkOspfCostUpdateInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) current_ospf_cost: u16,
    pub(crate) recommended_ospf_cost: u16,
    pub(crate) bird2_sha256_hex: &'a str,
    pub(crate) timeout_secs: u64,
}

pub(crate) async fn execute_network_apply_command(
    input: NetworkApplyInput<'_>,
) -> Result<Vec<CommandOutput>> {
    time::timeout(
        Duration::from_secs(input.timeout_secs.max(1)),
        apply_network_plan(input),
    )
    .await
    .context("network apply timed out")?
}

pub(crate) async fn execute_network_rollback_command(
    input: NetworkRollbackInput<'_>,
) -> Result<Vec<CommandOutput>> {
    time::timeout(
        Duration::from_secs(input.timeout_secs.max(1)),
        rollback_network_plan(input),
    )
    .await
    .context("network rollback timed out")?
}

pub(crate) async fn execute_network_ospf_cost_update_command(
    input: NetworkOspfCostUpdateInput<'_>,
) -> Result<Vec<CommandOutput>> {
    time::timeout(
        Duration::from_secs(input.timeout_secs.max(1)),
        update_network_ospf_cost(input),
    )
    .await
    .context("network OSPF cost update timed out")?
}

async fn apply_network_plan(input: NetworkApplyInput<'_>) -> Result<Vec<CommandOutput>> {
    if !input.config.network.apply_enabled {
        anyhow::bail!("network apply is disabled in agent config");
    }
    let endpoint = render_tunnel_endpoint_config(input.plan, input.side)
        .map_err(|error| anyhow::anyhow!("invalid tunnel endpoint config: {error}"))?;
    if endpoint.local_client_id != input.config.client_id {
        anyhow::bail!(
            "network apply side targets {}, but this agent is {}",
            endpoint.local_client_id,
            input.config.client_id
        );
    }
    if input.config_backend != input.config.network.backend {
        anyhow::bail!(
            "network apply backend {} does not match agent backend {}",
            input.config_backend.as_str(),
            input.config.network.backend.as_str()
        );
    }
    let backend_config =
        render_backend_config_for_endpoint(input.plan, &endpoint, input.config_backend)
            .map_err(|error| anyhow::anyhow!("invalid backend tunnel config: {error}"))?;
    if input.config_backend == TunnelConfigBackend::Ifupdown {
        verify_expected_hash(
            endpoint.ifupdown_snippet.as_bytes(),
            input.ifupdown_sha256_hex,
            "network apply",
            "ifupdown",
        )?;
    }
    if input.config_backend != TunnelConfigBackend::Ifupdown && input.config_sha256_hex.is_none() {
        anyhow::bail!("network apply backend config hash is required");
    }
    if let Some(config_sha256_hex) = input.config_sha256_hex {
        verify_expected_hash(
            &backend_config_proof_payload(&backend_config),
            config_sha256_hex,
            "network apply",
            "backend_config",
        )?;
    }
    verify_expected_hash(
        endpoint.bird2_interface_snippet.as_bytes(),
        input.bird2_sha256_hex,
        "network apply",
        "bird2",
    )?;
    let runtime_reconcile = if input.config.network.runtime_reconcile_enabled {
        let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
            config: input.config,
            plan: input.plan,
            side: input.side,
            timeout_secs: input.timeout_secs,
            #[cfg(test)]
            effective_uid_override: None,
        })
        .await?;
        if report["status"].as_str() == Some("failed") {
            anyhow::bail!("runtime tunnel reconcile failed before managed-file apply");
        }
        Some(report)
    } else {
        None
    };
    let routing_gate =
        runtime_routing_gate_from_reconcile(&input.config.network, runtime_reconcile.as_ref())?;

    let root = Path::new(&input.config.network.root_dir);
    let bird2_path = managed_destination(root, MANAGED_BIRD2_FILE)?;
    let mut planned = Vec::new();
    for file in &backend_config.files {
        planned.push(prepare_backend_file_update(root, input.plan, &endpoint, file).await?);
    }
    planned.push(
        prepare_file_update(
            &bird2_path,
            MANAGED_BIRD2_FILE,
            &managed_block(
                input.plan,
                &endpoint,
                "bird2",
                &endpoint.bird2_interface_snippet,
            ),
        )
        .await?,
    );

    let applied = apply_updates_with_rollback(&planned).await?;

    let hook_context = NetworkHookContext {
        plan: input.plan,
        endpoint: &endpoint,
    };
    let validation_specs = validation_hook_specs(&input.config.network, hook_context);
    let validation =
        match run_network_hooks(&validation_specs, input.config.network.hook_timeout_secs).await {
            Ok(reports) => reports,
            Err(error) => {
                rollback_updates(&applied).await;
                return Err(error);
            }
        };
    let hook_context = NetworkHookContext {
        plan: input.plan,
        endpoint: &endpoint,
    };
    let reload_specs = reload_hook_specs(&input.config.network, hook_context);
    let reload =
        match run_network_hooks(&reload_specs, input.config.network.hook_timeout_secs).await {
            Ok(reports) => reports,
            Err(error) => {
                rollback_updates(&applied).await;
                return Err(error);
            }
        };

    let files = applied
        .iter()
        .map(|update| {
            serde_json::json!({
                "path": update.managed_path,
                "destination": update.path,
                "sha256_hex": payload_hash(&update.next_contents),
                "backup_path": update.backup_path,
                "changed": update.changed,
            })
        })
        .collect::<Vec<_>>();
    let status = serde_json::json!({
        "type": "network_apply",
        "plan": input.plan.name,
        "interface": input.plan.interface_name,
        "side": match input.side {
            TunnelEndpointSide::Left => "left",
            TunnelEndpointSide::Right => "right",
        },
        "client_id": input.config.client_id,
        "peer_client_id": endpoint.peer_client_id,
        "config_backend": input.config_backend.as_str(),
        "applied_files": files,
        "validation": validation,
        "reload": reload,
        "runtime_reconcile": runtime_reconcile,
        "routing_gate": routing_gate,
        "rollback_available": true,
    });
    Ok(vec![CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    }])
}

async fn update_network_ospf_cost(
    input: NetworkOspfCostUpdateInput<'_>,
) -> Result<Vec<CommandOutput>> {
    if !input.config.network.apply_enabled {
        anyhow::bail!("network OSPF cost update is disabled in agent config");
    }
    if input.current_ospf_cost == input.recommended_ospf_cost {
        anyhow::bail!("network OSPF cost update is a no-op");
    }
    if input.plan.recommended_ospf_cost != input.recommended_ospf_cost {
        anyhow::bail!("network OSPF cost update plan cost does not match requested cost");
    }

    let endpoint = render_tunnel_endpoint_config(input.plan, input.side)
        .map_err(|error| anyhow::anyhow!("invalid tunnel endpoint config: {error}"))?;
    if endpoint.local_client_id != input.config.client_id {
        anyhow::bail!(
            "network OSPF cost update side targets {}, but this agent is {}",
            endpoint.local_client_id,
            input.config.client_id
        );
    }
    verify_expected_hash(
        endpoint.bird2_interface_snippet.as_bytes(),
        input.bird2_sha256_hex,
        "network OSPF cost update",
        "bird2",
    )?;
    let routing_gate = runtime_routing_gate_from_sysfs(&input.config.network, input.plan).await?;

    let mut previous_plan = input.plan.clone();
    previous_plan.recommended_ospf_cost = input.current_ospf_cost;
    let previous_endpoint = render_tunnel_endpoint_config(&previous_plan, input.side)
        .map_err(|error| anyhow::anyhow!("invalid current tunnel endpoint config: {error}"))?;

    let root = Path::new(&input.config.network.root_dir);
    let bird2_path = managed_destination(root, MANAGED_BIRD2_FILE)?;
    let previous_block = managed_block(
        &previous_plan,
        &previous_endpoint,
        "bird2",
        &previous_endpoint.bird2_interface_snippet,
    );
    let next_block = managed_block(
        input.plan,
        &endpoint,
        "bird2",
        &endpoint.bird2_interface_snippet,
    );
    let planned = prepare_ospf_cost_update(
        &bird2_path,
        &previous_block,
        &next_block,
        input.current_ospf_cost,
    )
    .await?;
    let applied = apply_updates_with_rollback(std::slice::from_ref(&planned)).await?;

    let hook_context = NetworkHookContext {
        plan: input.plan,
        endpoint: &endpoint,
    };
    let validation_specs = bird2_validation_hook_specs(&input.config.network, hook_context);
    let validation =
        match run_network_hooks(&validation_specs, input.config.network.hook_timeout_secs).await {
            Ok(reports) => reports,
            Err(error) => {
                rollback_updates(&applied).await;
                return Err(error);
            }
        };
    let hook_context = NetworkHookContext {
        plan: input.plan,
        endpoint: &endpoint,
    };
    let reload_specs = bird2_reload_hook_specs(&input.config.network, hook_context);
    let reload =
        match run_network_hooks(&reload_specs, input.config.network.hook_timeout_secs).await {
            Ok(reports) => reports,
            Err(error) => {
                rollback_updates(&applied).await;
                return Err(error);
            }
        };

    let files = applied
        .iter()
        .map(|update| {
            serde_json::json!({
                "path": update.managed_path,
                "destination": update.path,
                "sha256_hex": payload_hash(&update.next_contents),
                "backup_path": update.backup_path,
                "changed": update.changed,
            })
        })
        .collect::<Vec<_>>();
    let status = serde_json::json!({
        "type": "network_ospf_cost_update",
        "plan": input.plan.name,
        "interface": input.plan.interface_name,
        "side": match input.side {
            TunnelEndpointSide::Left => "left",
            TunnelEndpointSide::Right => "right",
        },
        "client_id": input.config.client_id,
        "peer_client_id": endpoint.peer_client_id,
        "current_ospf_cost": input.current_ospf_cost,
        "recommended_ospf_cost": input.recommended_ospf_cost,
        "applied_files": files,
        "validation": validation,
        "reload": reload,
        "routing_gate": routing_gate,
        "rollback_mode": "apply_previous_cost",
    });
    Ok(vec![CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    }])
}

fn runtime_routing_gate_from_reconcile(
    config: &vpsman_common::AgentNetworkConfig,
    report: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>> {
    if !config.runtime_reconcile_enabled {
        return Ok(None);
    }
    let Some(report) = report else {
        anyhow::bail!("runtime routing gate requires reconcile evidence");
    };
    let runtime_status = report["status"].as_str().unwrap_or("unknown");
    let link_existed_before = report["link_existed_before"].as_bool();
    let ready = match runtime_status {
        "converged" => true,
        "observed_only" => link_existed_before == Some(true),
        _ => false,
    };
    if ready {
        return Ok(Some(serde_json::json!({
            "type": "runtime_routing_gate",
            "status": "ready",
            "source": "runtime_reconcile",
            "runtime_status": runtime_status,
            "link_existed_before": link_existed_before,
        })));
    }
    if config.allow_routing_without_runtime_ready {
        return Ok(Some(serde_json::json!({
            "type": "runtime_routing_gate",
            "status": "degraded_allowed",
            "source": "runtime_reconcile",
            "runtime_status": runtime_status,
            "link_existed_before": link_existed_before,
            "reason": "allow_routing_without_runtime_ready",
        })));
    }
    anyhow::bail!("runtime tunnel is not ready for Bird2 routing update: {runtime_status}");
}

async fn runtime_routing_gate_from_sysfs(
    config: &vpsman_common::AgentNetworkConfig,
    plan: &TunnelPlan,
) -> Result<Option<serde_json::Value>> {
    if !config.runtime_reconcile_enabled {
        return Ok(None);
    }
    let link_exists =
        runtime_link_exists_for_routing(Path::new(&config.root_dir), &plan.interface_name).await;
    if link_exists {
        return Ok(Some(serde_json::json!({
            "type": "runtime_routing_gate",
            "status": "ready",
            "source": "sysfs",
            "interface": plan.interface_name,
            "link_exists": true,
        })));
    }
    if config.allow_routing_without_runtime_ready {
        return Ok(Some(serde_json::json!({
            "type": "runtime_routing_gate",
            "status": "degraded_allowed",
            "source": "sysfs",
            "interface": plan.interface_name,
            "link_exists": false,
            "reason": "allow_routing_without_runtime_ready",
        })));
    }
    anyhow::bail!(
        "runtime tunnel interface {} is not present before Bird2 OSPF cost update",
        plan.interface_name
    );
}

async fn runtime_link_exists_for_routing(root: &Path, interface_name: &str) -> bool {
    tokio::fs::metadata(root.join("sys/class/net").join(interface_name))
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

async fn rollback_network_plan(input: NetworkRollbackInput<'_>) -> Result<Vec<CommandOutput>> {
    if !input.config.network.apply_enabled {
        anyhow::bail!("network rollback is disabled in agent config");
    }
    let endpoint = render_tunnel_endpoint_config(input.plan, input.side)
        .map_err(|error| anyhow::anyhow!("invalid tunnel endpoint config: {error}"))?;
    if endpoint.local_client_id != input.config.client_id {
        anyhow::bail!(
            "network rollback side targets {}, but this agent is {}",
            endpoint.local_client_id,
            input.config.client_id
        );
    }

    let root = Path::new(&input.config.network.root_dir);
    let runtime_remove = if input.config.network.runtime_reconcile_enabled {
        let report = execute_runtime_tunnel_remove_report(NetworkRuntimeRemoveInput {
            config: input.config,
            plan: input.plan,
            side: input.side,
            timeout_secs: input.timeout_secs,
            #[cfg(test)]
            effective_uid_override: None,
        })
        .await?;
        if report["status"].as_str() == Some("failed") {
            anyhow::bail!("runtime tunnel remove failed before managed-file rollback");
        }
        Some(report)
    } else {
        None
    };
    let bird2_path = managed_destination(root, MANAGED_BIRD2_FILE)?;
    let backend_config =
        render_backend_config_for_endpoint(input.plan, &endpoint, input.config.network.backend)
            .map_err(|error| anyhow::anyhow!("invalid backend tunnel config: {error}"))?;
    let mut planned = Vec::new();
    for file in &backend_config.files {
        planned.push(prepare_backend_file_removal(root, input.plan, &endpoint, file).await?);
    }
    planned.push(
        prepare_file_removal(
            &bird2_path,
            MANAGED_BIRD2_FILE,
            &managed_block(
                input.plan,
                &endpoint,
                "bird2",
                &endpoint.bird2_interface_snippet,
            ),
        )
        .await?,
    );
    let changed = planned.iter().any(|update| update.changed);
    let pre_rollback = if changed {
        let hook_context = NetworkHookContext {
            plan: input.plan,
            endpoint: &endpoint,
        };
        let pre_rollback_specs = pre_rollback_hook_specs(&input.config.network, hook_context);
        run_network_hooks(&pre_rollback_specs, input.config.network.hook_timeout_secs).await?
    } else {
        Vec::new()
    };
    let applied = apply_updates_with_rollback(&planned).await?;

    let (validation, reload) = if changed {
        let hook_context = NetworkHookContext {
            plan: input.plan,
            endpoint: &endpoint,
        };
        let validation_specs = validation_hook_specs(&input.config.network, hook_context);
        let validation = match run_network_hooks(
            &validation_specs,
            input.config.network.hook_timeout_secs,
        )
        .await
        {
            Ok(reports) => reports,
            Err(error) => {
                rollback_updates(&applied).await;
                return Err(error);
            }
        };
        let hook_context = NetworkHookContext {
            plan: input.plan,
            endpoint: &endpoint,
        };
        let reload_specs = reload_hook_specs(&input.config.network, hook_context);
        let reload =
            match run_network_hooks(&reload_specs, input.config.network.hook_timeout_secs).await {
                Ok(reports) => reports,
                Err(error) => {
                    rollback_updates(&applied).await;
                    return Err(error);
                }
            };
        (validation, reload)
    } else {
        (Vec::new(), Vec::new())
    };

    let files = planned
        .iter()
        .map(|update| {
            serde_json::json!({
                "path": update.managed_path,
                "destination": update.path,
                "sha256_hex": payload_hash(&update.next_contents),
                "backup_path": update.backup_path,
                "changed": update.changed,
            })
        })
        .collect::<Vec<_>>();
    let status = serde_json::json!({
        "type": "network_rollback",
        "plan": input.plan.name,
        "interface": input.plan.interface_name,
        "side": match input.side {
            TunnelEndpointSide::Left => "left",
            TunnelEndpointSide::Right => "right",
        },
        "client_id": input.config.client_id,
        "peer_client_id": endpoint.peer_client_id,
        "config_backend": input.config.network.backend.as_str(),
        "removed_files": files,
        "pre_rollback": pre_rollback,
        "validation": validation,
        "reload": reload,
        "changed": changed,
        "runtime_remove": runtime_remove,
    });
    Ok(vec![CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    }])
}

async fn apply_updates_with_rollback(
    planned: &[PlannedFileUpdate],
) -> Result<Vec<PlannedFileUpdate>> {
    let mut applied = Vec::new();
    for update in planned {
        if update.changed {
            if let Err(error) = write_file_atomic(&update.path, &update.next_contents).await {
                rollback_updates(&applied).await;
                return Err(error);
            }
        }
        applied.push(update.clone());
    }
    Ok(applied)
}

fn verify_expected_hash(
    data: &[u8],
    expected: &str,
    action_label: &str,
    label: &str,
) -> Result<()> {
    let expected = expected.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        anyhow::bail!("{action_label} {label} hash is invalid");
    }
    let observed = payload_hash(data);
    if observed != expected {
        anyhow::bail!("{action_label} {label} hash mismatch");
    }
    Ok(())
}

pub(crate) fn managed_destination(root: &Path, managed_path: &str) -> Result<PathBuf> {
    if !root.is_absolute() {
        anyhow::bail!("network root must be absolute");
    }
    if !is_supported_managed_path(managed_path) {
        anyhow::bail!("unsupported network managed path");
    }
    Ok(root.join(managed_path.trim_start_matches('/')))
}

fn is_supported_managed_path(managed_path: &str) -> bool {
    matches!(
        managed_path,
        MANAGED_IFUPDOWN_FILE
            | MANAGED_BIRD2_FILE
            | MANAGED_NETPLAN_FILE
            | MANAGED_SYSTEMD_NETWORKD_NETDEV_FILE
            | MANAGED_SYSTEMD_NETWORKD_NETWORK_FILE
    )
}

pub(crate) fn managed_block(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    kind: &str,
    snippet: &str,
) -> String {
    let marker = format!(
        "{} {} {} {}",
        endpoint.local_client_id, endpoint.peer_client_id, plan.name, plan.interface_name
    );
    format!(
        "# vpsman-managed {kind} begin {marker}\n{}\n# vpsman-managed {kind} end {marker}\n",
        snippet.trim(),
    )
}

#[derive(Clone)]
struct PlannedFileUpdate {
    managed_path: &'static str,
    path: PathBuf,
    previous_contents: Option<Vec<u8>>,
    next_contents: Vec<u8>,
    backup_path: Option<String>,
    changed: bool,
}

async fn prepare_backend_file_update(
    root: &Path,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    file: &TunnelBackendFile,
) -> Result<PlannedFileUpdate> {
    let path = managed_destination(root, file.managed_path)?;
    let block = managed_block(plan, endpoint, file.block_kind, &file.contents);
    prepare_file_update(&path, file.managed_path, &block).await
}

async fn prepare_backend_file_removal(
    root: &Path,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    file: &TunnelBackendFile,
) -> Result<PlannedFileUpdate> {
    let path = managed_destination(root, file.managed_path)?;
    let block = managed_block(plan, endpoint, file.block_kind, &file.contents);
    prepare_file_removal(&path, file.managed_path, &block).await
}

async fn prepare_file_update(
    path: &Path,
    managed_path: &'static str,
    block: &str,
) -> Result<PlannedFileUpdate> {
    prepare_file_transform(path, managed_path, |previous_text| {
        upsert_managed_block(previous_text, block)
    })
    .await
}

async fn prepare_file_removal(
    path: &Path,
    managed_path: &'static str,
    block: &str,
) -> Result<PlannedFileUpdate> {
    prepare_file_transform(path, managed_path, |previous_text| {
        remove_managed_block(previous_text, block)
    })
    .await
}

async fn prepare_ospf_cost_update(
    path: &Path,
    previous_block: &str,
    next_block: &str,
    current_ospf_cost: u16,
) -> Result<PlannedFileUpdate> {
    prepare_file_transform(path, MANAGED_BIRD2_FILE, |previous_text| {
        let Some((start, end)) = managed_block_bounds(previous_text, previous_block)? else {
            anyhow::bail!("network OSPF cost update requires an existing managed Bird2 block");
        };
        let current_block = &previous_text[start..end];
        let expected_cost = format!("  cost {current_ospf_cost};");
        if !current_block
            .lines()
            .any(|line| line.trim() == expected_cost.trim())
        {
            anyhow::bail!("network OSPF cost update current cost mismatch");
        }
        upsert_managed_block(previous_text, next_block)
    })
    .await
}

async fn prepare_file_transform<F>(
    path: &Path,
    managed_path: &'static str,
    transform: F,
) -> Result<PlannedFileUpdate>
where
    F: FnOnce(&str) -> Result<String>,
{
    let previous_contents = read_existing_regular_file(path).await?;
    let previous_text = previous_contents
        .as_ref()
        .map(|contents| {
            std::str::from_utf8(contents)
                .map(str::to_owned)
                .context("managed network file must be UTF-8")
        })
        .transpose()?
        .unwrap_or_default();
    let next_contents = transform(&previous_text)?.into_bytes();
    let changed = previous_contents.as_deref().unwrap_or_default() != next_contents.as_slice();
    let backup_path = if changed {
        if let Some(contents) = &previous_contents {
            Some(write_backup(path, contents).await?)
        } else {
            None
        }
    } else {
        None
    };
    Ok(PlannedFileUpdate {
        managed_path,
        path: path.to_path_buf(),
        previous_contents,
        next_contents,
        backup_path,
        changed,
    })
}

pub(crate) async fn read_existing_regular_file(path: &Path) -> Result<Option<Vec<u8>>> {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return Ok(None);
    };
    if !metadata.is_file() {
        anyhow::bail!(
            "managed network path is not a regular file: {}",
            path.display()
        );
    }
    if metadata.len() > MAX_MANAGED_NETWORK_FILE_BYTES {
        anyhow::bail!(
            "managed network file exceeds size limit: {}",
            path.display()
        );
    }
    Ok(Some(tokio::fs::read(path).await?))
}

async fn write_backup(path: &Path, contents: &[u8]) -> Result<String> {
    let parent = path.parent().context("managed file has no parent")?;
    tokio::fs::create_dir_all(parent).await?;
    let file_name = path
        .file_name()
        .context("managed file has no file name")?
        .to_string_lossy();
    let backup_path = parent.join(format!(
        "{file_name}.vpsman-backup-{}",
        uuid::Uuid::new_v4()
    ));
    tokio::fs::write(&backup_path, contents).await?;
    tokio::fs::set_permissions(&backup_path, std::fs::Permissions::from_mode(0o600)).await?;
    Ok(backup_path.to_string_lossy().to_string())
}

fn upsert_managed_block(existing: &str, block: &str) -> Result<String> {
    if let Some((start, end)) = managed_block_bounds(existing, block)? {
        let mut next = String::new();
        next.push_str(&existing[..start]);
        next.push_str(block);
        next.push_str(&existing[end..]);
        return Ok(next);
    }
    let mut next = existing.to_string();
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    if !next.is_empty() {
        next.push('\n');
    }
    next.push_str(block);
    Ok(next)
}

fn remove_managed_block(existing: &str, block: &str) -> Result<String> {
    let Some((start, end)) = managed_block_bounds(existing, block)? else {
        return Ok(existing.to_string());
    };
    let mut next = String::new();
    next.push_str(&existing[..start]);
    next.push_str(&existing[end..]);
    while next.contains("\n\n\n") {
        next = next.replace("\n\n\n", "\n\n");
    }
    Ok(next)
}

pub(crate) fn managed_block_bounds(existing: &str, block: &str) -> Result<Option<(usize, usize)>> {
    let first_line = block
        .lines()
        .next()
        .context("managed block is empty")?
        .to_string();
    let last_line = block
        .lines()
        .last()
        .context("managed block is empty")?
        .to_string();
    let Some(start) = existing.find(&first_line) else {
        return Ok(None);
    };
    let after_start = start + first_line.len();
    let relative_end = existing[after_start..]
        .find(&last_line)
        .context("existing managed block is missing end marker")?;
    let mut end = after_start + relative_end + last_line.len();
    if existing[end..].starts_with('\n') {
        end += 1;
    }
    Ok(Some((start, end)))
}

async fn write_file_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    if contents.len() as u64 > MAX_MANAGED_NETWORK_FILE_BYTES {
        anyhow::bail!(
            "managed network file exceeds size limit: {}",
            path.display()
        );
    }
    let parent = path.parent().context("managed file has no parent")?;
    tokio::fs::create_dir_all(parent).await?;
    let file_name = path
        .file_name()
        .context("managed file has no file name")?
        .to_string_lossy();
    let temp_path = parent.join(format!(
        ".vpsman-network-{file_name}-{}",
        uuid::Uuid::new_v4()
    ));
    tokio::fs::write(&temp_path, contents).await?;
    tokio::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o644)).await?;
    if let Err(error) = tokio::fs::rename(&temp_path, path).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
    Ok(())
}

async fn rollback_updates(updates: &[PlannedFileUpdate]) {
    for update in updates.iter().rev() {
        if let Some(contents) = &update.previous_contents {
            let _ = tokio::fs::write(&update.path, contents).await;
        } else {
            let _ = tokio::fs::remove_file(&update.path).await;
        }
    }
}

#[cfg(test)]
mod tests;
