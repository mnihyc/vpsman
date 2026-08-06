use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use tokio::{process::Command, time};
use vpsman_common::{
    payload_hash, render_tunnel_endpoint_config, AgentConfig, AgentNetworkConfig, CommandOutput,
    OutputStream, RuntimeTunnelAdapterCommands, RuntimeTunnelManager, TunnelEndpointConfig,
    TunnelEndpointSide, TunnelPlan,
};

use crate::{
    child_process::{run_child_with_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult},
    command_worker::{run_cancelable, CommandCancelToken, CommandCanceled},
    network_runtime::render_runtime_adapter_command,
};

const DEFAULT_PROC_SELF_NETNS_PATH: &str = "/proc/self/ns/net";

pub(crate) struct NetworkStatusInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) runtime_adapter: Option<&'a RuntimeTunnelAdapterCommands>,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) max_timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
}

pub(crate) async fn execute_network_status_command(
    input: NetworkStatusInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let cancel_token = input.cancel_token.clone();
    run_cancelable("network_status", cancel_token, async move {
        time::timeout(
            Duration::from_secs(input.max_timeout_secs.max(1)),
            inspect_network_plan(input),
        )
        .await
        .context("network status timed out")?
    })
    .await
}

async fn inspect_network_plan(input: NetworkStatusInput<'_>) -> Result<Vec<CommandOutput>> {
    let endpoint = render_tunnel_endpoint_config(input.plan, input.side)
        .map_err(|error| anyhow::anyhow!("invalid tunnel endpoint config: {error}"))?;
    if endpoint.local_client_id != input.config.client_id {
        anyhow::bail!(
            "network status side targets {}, but this agent is {}",
            endpoint.local_client_id,
            input.config.client_id
        );
    }

    let runtime = inspect_runtime_status(
        &input.config.network,
        Path::new(&input.config.network.root_dir),
        input.plan,
        input.runtime_adapter,
        &endpoint,
        input.cancel_token,
    )
    .await?;
    let status = serde_json::json!({
        "type": "network_status",
        "plan": input.plan.name,
        "interface": input.plan.interface_name,
        "side": endpoint_side_label(input.side),
        "client_id": input.config.client_id,
        "peer_client_id": endpoint.peer_client_id,
        "scope": "declared_plan_only",
        "runtime": runtime,
    });
    Ok(vec![CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    }])
}

async fn inspect_runtime_status(
    config: &AgentNetworkConfig,
    root: &Path,
    plan: &TunnelPlan,
    runtime_adapter: Option<&RuntimeTunnelAdapterCommands>,
    endpoint: &TunnelEndpointConfig,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    let interface = inspect_interface_sysfs(root, &plan.interface_name).await;
    let desired_interfaces = inspect_desired_interfaces(root, plan).await;
    let declared_stale_interfaces = inspect_declared_stale_interfaces(root, plan).await;
    let kernel_namespace = inspect_kernel_namespace(root).await;
    let kernel = inspect_kernel_status(config, root, plan, endpoint, cancel_token.clone()).await?;
    let adapter =
        inspect_runtime_adapter_status(config, plan, runtime_adapter, endpoint, cancel_token)
            .await?;
    let summary = summarize_runtime_status(
        plan,
        endpoint.local_mtu,
        &interface,
        &desired_interfaces,
        &declared_stale_interfaces,
        &kernel_namespace,
        &kernel,
        &adapter,
    );
    Ok(serde_json::json!({
        "manager": plan.runtime_control.manager,
        "topology_version": &plan.runtime_topology.version,
        "interface": interface,
        "desired_interfaces": desired_interfaces,
        "declared_stale_interfaces": declared_stale_interfaces,
        "kernel_namespace": kernel_namespace,
        "kernel": kernel,
        "adapter": adapter,
        "summary": summary,
    }))
}

pub(crate) async fn runtime_tunnel_requires_reconnect_sync(
    config: &AgentConfig,
    telemetry_plan: &vpsman_common::AgentRuntimeStatusTelemetryPlan,
) -> Result<bool> {
    if telemetry_plan.plan.runtime_control.manager == RuntimeTunnelManager::ExternalObserved {
        return Ok(false);
    }
    let endpoint =
        render_tunnel_endpoint_config(&telemetry_plan.plan, telemetry_plan.endpoint_side)
            .map_err(|error| anyhow::anyhow!("invalid tunnel endpoint config: {error}"))?;
    if endpoint.local_client_id != config.client_id {
        anyhow::bail!(
            "runtime tunnel side targets {}, but this agent is {}",
            endpoint.local_client_id,
            config.client_id
        );
    }
    let root = Path::new(&config.network.root_dir);
    let interface = inspect_interface_sysfs(root, &telemetry_plan.plan.interface_name).await;
    let desired_interfaces = inspect_desired_interfaces(root, &telemetry_plan.plan).await;
    let declared_stale_interfaces =
        inspect_declared_stale_interfaces(root, &telemetry_plan.plan).await;
    let adapter = inspect_runtime_adapter_status(
        &config.network,
        &telemetry_plan.plan,
        telemetry_plan.runtime_adapter.as_ref(),
        &endpoint,
        CommandCancelToken::default(),
    )
    .await?;
    Ok(!runtime_reconcile_reasons(
        &telemetry_plan.plan,
        endpoint.local_mtu,
        &interface,
        &desired_interfaces,
        &declared_stale_interfaces,
        &adapter,
    )
    .is_empty())
}

async fn inspect_interface_sysfs(root: &Path, interface_name: &str) -> serde_json::Value {
    let base = root.join("sys/class/net").join(interface_name);
    let Ok(metadata) = tokio::fs::metadata(&base).await else {
        return serde_json::json!({
            "source": "sysfs",
            "interface": interface_name,
            "exists": false,
            "path": base,
        });
    };
    if !metadata.is_dir() {
        return serde_json::json!({
            "source": "sysfs",
            "interface": interface_name,
            "exists": false,
            "path": base,
            "error": "interface_path_is_not_directory",
        });
    }

    serde_json::json!({
        "source": "sysfs",
        "interface": interface_name,
        "exists": true,
        "path": base,
        "operstate": read_sysfs_string(&base.join("operstate")).await,
        "mtu": read_sysfs_u64(&base.join("mtu")).await,
        "address": read_sysfs_string(&base.join("address")).await,
        "type": read_sysfs_i64(&base.join("type")).await,
        "rx_bytes": read_sysfs_u64(&base.join("statistics/rx_bytes")).await,
        "tx_bytes": read_sysfs_u64(&base.join("statistics/tx_bytes")).await,
    })
}

async fn inspect_desired_interfaces(root: &Path, plan: &TunnelPlan) -> Vec<serde_json::Value> {
    let mut names = vec![plan.interface_name.clone()];
    for name in &plan.runtime_topology.desired_interfaces {
        if !names.iter().any(|existing| existing == name) {
            names.push(name.clone());
        }
    }
    let mut reports = Vec::with_capacity(names.len());
    for name in names {
        let report = inspect_interface_sysfs(root, &name).await;
        reports.push(serde_json::json!({
            "interface": name,
            "exists": report["exists"].as_bool().unwrap_or(false),
            "operstate": report.get("operstate").cloned().unwrap_or(serde_json::Value::Null),
        }));
    }
    reports
}

async fn inspect_declared_stale_interfaces(
    root: &Path,
    plan: &TunnelPlan,
) -> Vec<serde_json::Value> {
    let mut reports = Vec::with_capacity(plan.runtime_topology.stale_interfaces.len());
    for name in &plan.runtime_topology.stale_interfaces {
        let report = inspect_interface_sysfs(root, name).await;
        reports.push(serde_json::json!({
            "interface": name,
            "exists": report["exists"].as_bool().unwrap_or(false),
            "operstate": report.get("operstate").cloned().unwrap_or(serde_json::Value::Null),
        }));
    }
    reports
}

async fn read_sysfs_string(path: &Path) -> Option<String> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return None;
    }
    tokio::fs::read_to_string(path)
        .await
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn read_sysfs_u64(path: &Path) -> Option<u64> {
    read_sysfs_string(path).await?.parse().ok()
}

async fn read_sysfs_i64(path: &Path) -> Option<i64> {
    read_sysfs_string(path).await?.parse().ok()
}

async fn inspect_kernel_namespace(root: &Path) -> serde_json::Value {
    let real_kernel_namespace = root == Path::new("/");
    let netns_link = if real_kernel_namespace {
        tokio::fs::read_link(DEFAULT_PROC_SELF_NETNS_PATH)
            .await
            .ok()
            .map(|path| path.to_string_lossy().to_string())
    } else {
        None
    };
    serde_json::json!({
        "real_kernel_namespace": real_kernel_namespace,
        "netns_link": netns_link,
        "configured_root": root,
        "probe_policy": if real_kernel_namespace {
            "declared_interface_read_only"
        } else {
            "rooted_sysfs_only"
        },
    })
}

async fn inspect_kernel_status(
    config: &AgentNetworkConfig,
    root: &Path,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    if root != Path::new("/") {
        return Ok(serde_json::json!({
            "configured": false,
            "skipped": true,
            "reason": "kernel_probes_require_real_root_namespace",
            "probe_scope": "declared_interface_read_only",
        }));
    }
    if config.runtime_ip_argv.is_empty() {
        return Ok(serde_json::json!({
            "configured": false,
            "skipped": true,
            "reason": "runtime_ip_argv_unconfigured",
            "probe_scope": "declared_interface_read_only",
        }));
    }

    let link = run_kernel_ip_probe(
        config,
        "kernel_link",
        plan,
        endpoint,
        &["-j", "-s", "link", "show", "dev", "{interface}"],
        cancel_token.clone(),
    )
    .await?;
    let neighbors = run_kernel_ip_probe(
        config,
        "kernel_neighbors",
        plan,
        endpoint,
        &["-j", "neigh", "show", "dev", "{interface}"],
        cancel_token.clone(),
    )
    .await?;
    let routes = run_kernel_ip_probe(
        config,
        "kernel_routes",
        plan,
        endpoint,
        &["-j", "route", "show", "dev", "{interface}"],
        cancel_token,
    )
    .await?;
    Ok(serde_json::json!({
        "configured": true,
        "probe_scope": "declared_interface_read_only",
        "link": link,
        "neighbors": neighbors,
        "routes": routes,
    }))
}

async fn run_kernel_ip_probe(
    config: &AgentNetworkConfig,
    label: &str,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    args: &[&str],
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    let mut argv = config.runtime_ip_argv.clone();
    argv.extend(args.iter().map(|part| part.to_string()));
    let argv = render_probe_argv(&argv, plan, endpoint);
    match run_status_probe(
        label,
        &argv,
        config.status_probe_timeout_secs,
        config.status_probe_max_output_bytes as usize,
        cancel_token,
    )
    .await
    {
        Ok(report) => Ok(report),
        Err(error) if error.downcast_ref::<CommandCanceled>().is_some() => Err(error),
        Err(error) => Ok(serde_json::json!({
            "configured": true,
            "label": label,
            "argv": argv,
            "success": false,
            "error": error.to_string(),
        })),
    }
}

async fn inspect_runtime_adapter_status(
    config: &AgentNetworkConfig,
    plan: &TunnelPlan,
    runtime_adapter: Option<&RuntimeTunnelAdapterCommands>,
    endpoint: &TunnelEndpointConfig,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    match plan.runtime_control.manager {
        RuntimeTunnelManager::AgentBuiltin => Ok(serde_json::json!({
            "configured": false,
            "skipped": true,
            "reason": "agent_builtin",
        })),
        RuntimeTunnelManager::ExternalObserved => Ok(serde_json::json!({
            "configured": false,
            "skipped": true,
            "reason": "external_observed",
        })),
        RuntimeTunnelManager::CustomAdapter => {
            let Some(adapter) = runtime_adapter else {
                return Ok(serde_json::json!({
                    "configured": false,
                    "skipped": true,
                    "reason": "adapter_snapshot_unconfigured",
                }));
            };
            let command = &adapter.status;
            let argv = match render_runtime_adapter_command(command, plan, endpoint) {
                Ok(argv) => argv,
                Err(error) => {
                    return Ok(serde_json::json!({
                        "configured": true,
                        "label": "runtime_adapter_status",
                        "success": false,
                        "error": error.to_string(),
                    }));
                }
            };
            match run_status_probe(
                "runtime_adapter_status",
                &argv,
                command
                    .max_timeout_secs
                    .min(config.runtime_command_timeout_secs)
                    .max(1),
                usize::try_from(
                    command
                        .max_output_bytes
                        .min(config.runtime_command_max_output_bytes),
                )
                .unwrap_or(config.runtime_command_max_output_bytes as usize),
                cancel_token,
            )
            .await
            {
                Ok(report) => Ok(report),
                Err(error) if error.downcast_ref::<CommandCanceled>().is_some() => Err(error),
                Err(error) => Ok(serde_json::json!({
                    "configured": true,
                    "label": "runtime_adapter_status",
                    "argv": argv,
                    "success": false,
                    "error": error.to_string(),
                })),
            }
        }
    }
}

fn summarize_runtime_status(
    plan: &TunnelPlan,
    desired_mtu: Option<u16>,
    interface: &serde_json::Value,
    desired_interfaces: &[serde_json::Value],
    declared_stale_interfaces: &[serde_json::Value],
    kernel_namespace: &serde_json::Value,
    kernel: &serde_json::Value,
    adapter: &serde_json::Value,
) -> serde_json::Value {
    let interface_exists = interface["exists"].as_bool().unwrap_or(false);
    let interface_operstate = interface["operstate"].as_str();
    let desired_missing_count = desired_interfaces
        .iter()
        .filter(|report| report["exists"].as_bool() != Some(true))
        .count();
    let stale_present_count = declared_stale_interfaces
        .iter()
        .filter(|report| report["exists"].as_bool() == Some(true))
        .count();
    let reasons = runtime_reconcile_reasons(
        plan,
        desired_mtu,
        interface,
        desired_interfaces,
        declared_stale_interfaces,
        adapter,
    );
    let adapter_state = match plan.runtime_control.manager {
        RuntimeTunnelManager::AgentBuiltin => "not_applicable",
        RuntimeTunnelManager::ExternalObserved => "observed_only",
        RuntimeTunnelManager::CustomAdapter => {
            if adapter["success"].as_bool() == Some(true) {
                "healthy"
            } else if adapter["configured"].as_bool() == Some(false) {
                "unknown"
            } else {
                "unhealthy"
            }
        }
    };

    let healthy = reasons.is_empty();
    let status =
        if plan.runtime_control.manager == RuntimeTunnelManager::ExternalObserved && healthy {
            "observed"
        } else if healthy {
            "healthy"
        } else if reasons.contains(&"runtime_interface_missing")
            || reasons.contains(&"runtime_plan_mtu_unconfigured")
            || reasons.contains(&"runtime_interface_mtu_mismatch")
            || reasons.contains(&"runtime_interface_mtu_unavailable")
            || reasons.contains(&"desired_interface_missing")
            || reasons.contains(&"stale_interface_present")
        {
            "drift"
        } else if reasons.contains(&"adapter_status_failed") {
            "adapter_unhealthy"
        } else {
            "degraded"
        };

    serde_json::json!({
        "manager": plan.runtime_control.manager,
        "scope": "declared_plan_only",
        "status": status,
        "healthy": healthy,
        "drift": status == "drift",
        "reasons": reasons,
        "interface_exists": interface_exists,
        "interface_operstate": interface_operstate,
        "interface_mtu": interface["mtu"],
        "desired_mtu": desired_mtu,
        "desired_missing_count": desired_missing_count,
        "stale_present_count": stale_present_count,
        "adapter_state": adapter_state,
        "real_kernel_namespace_covered": kernel_namespace["real_kernel_namespace"]
            .as_bool()
            .unwrap_or(false),
        "kernel_link_probe_state": probe_state(&kernel["link"]),
        "neighbor_probe_state": probe_state(&kernel["neighbors"]),
        "route_probe_state": probe_state(&kernel["routes"]),
        "topology_version": &plan.runtime_topology.version,
    })
}

fn runtime_reconcile_reasons(
    plan: &TunnelPlan,
    desired_mtu: Option<u16>,
    interface: &serde_json::Value,
    desired_interfaces: &[serde_json::Value],
    declared_stale_interfaces: &[serde_json::Value],
    adapter: &serde_json::Value,
) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    if interface["exists"].as_bool() != Some(true) {
        reasons.push("runtime_interface_missing");
    } else if interface["operstate"].as_str() == Some("down") {
        reasons.push("runtime_interface_down");
    }
    if plan.runtime_control.manager == RuntimeTunnelManager::AgentBuiltin
        && interface["exists"].as_bool() == Some(true)
    {
        match (interface["mtu"].as_u64(), desired_mtu) {
            (_, None) => reasons.push("runtime_plan_mtu_unconfigured"),
            (Some(actual), Some(desired)) if actual != u64::from(desired) => {
                reasons.push("runtime_interface_mtu_mismatch");
            }
            (None, Some(_)) => reasons.push("runtime_interface_mtu_unavailable"),
            (Some(_), Some(_)) => {}
        }
    }
    if desired_interfaces
        .iter()
        .any(|report| report["exists"].as_bool() != Some(true))
    {
        reasons.push("desired_interface_missing");
    }
    if declared_stale_interfaces
        .iter()
        .any(|report| report["exists"].as_bool() == Some(true))
    {
        reasons.push("stale_interface_present");
    }
    if plan.runtime_control.manager == RuntimeTunnelManager::CustomAdapter {
        if adapter["configured"].as_bool() == Some(false) {
            reasons.push("adapter_status_unconfigured");
        } else if adapter["success"].as_bool() != Some(true) {
            reasons.push("adapter_status_failed");
        }
    }
    reasons
}

fn probe_state(report: &serde_json::Value) -> &'static str {
    if report.is_null() {
        "skipped"
    } else if report["success"].as_bool() == Some(true) {
        "success"
    } else if report["configured"].as_bool() == Some(false)
        || report["skipped"].as_bool() == Some(true)
    {
        "skipped"
    } else if report["configured"].as_bool() == Some(true) {
        "failed"
    } else {
        "unknown"
    }
}

fn render_probe_argv(
    argv: &[String],
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Vec<String> {
    argv.iter()
        .map(|part| {
            part.replace("{interface}", &plan.interface_name)
                .replace("{plan}", &plan.name)
                .replace("{local_client_id}", &endpoint.local_client_id)
                .replace("{peer_client_id}", &endpoint.peer_client_id)
        })
        .collect()
}

async fn run_status_probe(
    label: &str,
    argv: &[String],
    max_timeout_secs: u64,
    max_output_bytes: usize,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    if argv.is_empty() || !argv[0].starts_with('/') {
        anyhow::bail!("network status probe {label} requires an absolute executable");
    }
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    let result = run_child_with_bounded_output_cancelable(
        command,
        max_timeout_secs.clamp(1, 30),
        max_output_bytes,
        ChildCleanupPolicy::ProcessGroup,
        cancel_token,
    )
    .await
    .with_context(|| format!("failed to run network status probe {label}"))?;
    match result {
        ChildRunResult::Completed(output) => Ok(probe_report(ProbeReportInput {
            label,
            argv,
            exit_code: output.exit_code,
            timed_out: false,
            output_limited: output.stdout_truncated || output.stderr_truncated,
            max_output_bytes,
            stdout: output.stdout,
            stderr: output.stderr,
        })),
        ChildRunResult::TimedOut(_) => Ok(probe_report(ProbeReportInput {
            label,
            argv,
            exit_code: None,
            timed_out: true,
            output_limited: false,
            max_output_bytes,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })),
        ChildRunResult::Canceled { reason, .. } => {
            Err(CommandCanceled::new("network_status", reason).into())
        }
    }
}

struct ProbeReportInput<'a> {
    label: &'a str,
    argv: &'a [String],
    exit_code: Option<i32>,
    timed_out: bool,
    output_limited: bool,
    max_output_bytes: usize,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn probe_report(input: ProbeReportInput<'_>) -> serde_json::Value {
    let success = input.exit_code == Some(0) && !input.timed_out && !input.output_limited;
    serde_json::json!({
        "configured": true,
        "label": input.label,
        "argv": input.argv,
        "success": success,
        "exit_code": input.exit_code,
        "timed_out": input.timed_out,
        "output_limited": input.output_limited,
        "max_output_bytes": input.max_output_bytes,
        "stdout": output_json(&input.stdout),
        "stderr": output_json(&input.stderr),
    })
}

fn output_json(output: &[u8]) -> serde_json::Value {
    let utf8 = std::str::from_utf8(output).ok();
    serde_json::json!({
        "size_bytes": output.len(),
        "sha256_hex": payload_hash(output),
        "utf8": utf8.is_some(),
        "text": utf8.map(str::to_string),
    })
}

fn endpoint_side_label(side: TunnelEndpointSide) -> &'static str {
    match side {
        TunnelEndpointSide::Left => "left",
        TunnelEndpointSide::Right => "right",
    }
}

#[cfg(test)]
#[path = "tests_network_status.rs"]
mod tests;
