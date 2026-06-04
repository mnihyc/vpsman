use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    task::JoinHandle,
    time,
};
use vpsman_common::{
    payload_hash, render_backend_config_for_endpoint, render_tunnel_endpoint_config, AgentConfig,
    AgentNetworkConfig, CommandOutput, OutputStream, RuntimeTunnelManager, TunnelEndpointConfig,
    TunnelEndpointSide, TunnelPlan, MANAGED_BIRD2_FILE,
};

use crate::network_apply::{
    managed_block, managed_block_bounds, managed_destination, read_existing_regular_file,
};
use crate::network_runtime::render_runtime_adapter_command;

const DEFAULT_PROC_SELF_NETNS_PATH: &str = "/proc/self/ns/net";

pub(crate) struct NetworkStatusInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) timeout_secs: u64,
}

pub(crate) async fn execute_network_status_command(
    input: NetworkStatusInput<'_>,
) -> Result<Vec<CommandOutput>> {
    time::timeout(
        Duration::from_secs(input.timeout_secs.max(1)),
        inspect_network_plan(input),
    )
    .await
    .context("network status timed out")?
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
    let root = Path::new(&input.config.network.root_dir);
    let bird2_path = managed_destination(root, MANAGED_BIRD2_FILE)?;
    let backend_config =
        render_backend_config_for_endpoint(input.plan, &endpoint, input.config.network.backend)
            .map_err(|error| anyhow::anyhow!("invalid backend tunnel config: {error}"))?;
    let mut files = Vec::new();
    for file in &backend_config.files {
        let path = managed_destination(root, file.managed_path)?;
        files.push(
            inspect_managed_file(
                &path,
                file.managed_path,
                &managed_block(input.plan, &endpoint, file.block_kind, &file.contents),
            )
            .await?,
        );
    }
    files.push(
        inspect_managed_file(
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
    let applied = files
        .iter()
        .all(|file| file["expected_block_matches"].as_bool() == Some(true));
    let malformed = files
        .iter()
        .any(|file| file["managed_block_malformed"].as_bool() == Some(true));
    let runtime = inspect_runtime_status(&input.config.network, root, input.plan, &endpoint).await;
    let status = serde_json::json!({
        "type": "network_status",
        "plan": input.plan.name,
        "interface": input.plan.interface_name,
        "side": match input.side {
            TunnelEndpointSide::Left => "left",
            TunnelEndpointSide::Right => "right",
        },
        "client_id": input.config.client_id,
        "peer_client_id": endpoint.peer_client_id,
        "config_backend": input.config.network.backend.as_str(),
        "applied": applied,
        "malformed": malformed,
        "files": files,
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

async fn inspect_managed_file(
    path: &Path,
    managed_path: &'static str,
    expected_block: &str,
) -> Result<serde_json::Value> {
    let expected_hash = payload_hash(expected_block.as_bytes());
    let Some(contents) = read_existing_regular_file(path).await? else {
        return Ok(serde_json::json!({
            "path": managed_path,
            "destination": path,
            "exists": false,
            "utf8": true,
            "file_size_bytes": 0,
            "file_sha256_hex": null,
            "managed_block_present": false,
            "managed_block_malformed": false,
            "managed_block_sha256_hex": null,
            "expected_block_sha256_hex": expected_hash,
            "expected_block_matches": false,
        }));
    };
    let file_hash = payload_hash(&contents);
    let file_size = contents.len();
    let Ok(text) = std::str::from_utf8(&contents) else {
        return Ok(serde_json::json!({
            "path": managed_path,
            "destination": path,
            "exists": true,
            "utf8": false,
            "file_size_bytes": file_size,
            "file_sha256_hex": file_hash,
            "managed_block_present": false,
            "managed_block_malformed": false,
            "managed_block_sha256_hex": null,
            "expected_block_sha256_hex": expected_hash,
            "expected_block_matches": false,
        }));
    };
    let block_result = managed_block_bounds(text, expected_block);
    let (present, malformed, observed_hash, matches_expected) = match block_result {
        Ok(Some((start, end))) => {
            let observed = &text[start..end];
            let observed_hash = payload_hash(observed.as_bytes());
            (
                true,
                false,
                Some(observed_hash.clone()),
                observed_hash == expected_hash && observed == expected_block,
            )
        }
        Ok(None) => (false, false, None, false),
        Err(_) => (true, true, None, false),
    };
    Ok(serde_json::json!({
        "path": managed_path,
        "destination": path,
        "exists": true,
        "utf8": true,
        "file_size_bytes": file_size,
        "file_sha256_hex": file_hash,
        "managed_block_present": present,
        "managed_block_malformed": malformed,
        "managed_block_sha256_hex": observed_hash,
        "expected_block_sha256_hex": expected_hash,
        "expected_block_matches": matches_expected,
    }))
}

async fn inspect_runtime_status(
    config: &AgentNetworkConfig,
    root: &Path,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> serde_json::Value {
    let interface = inspect_interface_sysfs(root, &plan.interface_name).await;
    let desired_interfaces = inspect_desired_interfaces(root, plan).await;
    let declared_stale_interfaces = inspect_declared_stale_interfaces(root, plan).await;
    let observed_tunnels = discover_observed_tunnels(root, plan).await;
    let kernel_namespace = inspect_kernel_namespace(root).await;
    let kernel = inspect_kernel_status(config, root, plan, endpoint).await;
    let adapter = inspect_runtime_adapter_status(config, plan, endpoint).await;
    let bird2 = inspect_bird2_status(config, plan, endpoint).await;
    let summary = summarize_runtime_status(RuntimeStatusSummaryInput {
        plan,
        interface: &interface,
        desired_interfaces: &desired_interfaces,
        declared_stale_interfaces: &declared_stale_interfaces,
        observed_tunnels: &observed_tunnels,
        kernel_namespace: &kernel_namespace,
        kernel: &kernel,
        adapter: &adapter,
        bird2: &bird2,
    });
    serde_json::json!({
        "manager": plan.runtime_control.manager,
        "topology_version": &plan.runtime_topology.version,
        "desired_interfaces": desired_interfaces,
        "declared_stale_interfaces": declared_stale_interfaces,
        "observed_tunnels": observed_tunnels,
        "kernel_namespace": kernel_namespace,
        "kernel": kernel,
        "interface": interface,
        "adapter": adapter,
        "bird2": bird2,
        "summary": summary,
    })
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

async fn discover_observed_tunnels(root: &Path, plan: &TunnelPlan) -> Vec<serde_json::Value> {
    const MAX_OBSERVED_TUNNELS: usize = 64;
    let mut desired = BTreeSet::from([plan.interface_name.clone()]);
    desired.extend(plan.runtime_topology.desired_interfaces.iter().cloned());
    let declared_stale = plan
        .runtime_topology
        .stale_interfaces
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let Ok(mut entries) = tokio::fs::read_dir(root.join("sys/class/net")).await else {
        return Vec::new();
    };
    let mut tunnels = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let interface = entry.file_name().to_string_lossy().to_string();
        if interface.is_empty() || interface.len() > 64 {
            continue;
        }
        let interface_path = entry.path();
        let link_type = read_sysfs_i64(&interface_path.join("type")).await;
        let Some(kind) = classify_runtime_tunnel(&interface, link_type) else {
            continue;
        };
        let is_desired = desired.contains(&interface);
        let is_declared_stale = declared_stale.contains(&interface);
        let promotion_required = !is_desired && !is_declared_stale;
        tunnels.push(serde_json::json!({
            "interface": interface,
            "kind": kind,
            "source": "sysfs",
            "desired": is_desired,
            "declared_stale": is_declared_stale,
            "import_candidate": promotion_required,
            "mutation_policy": observed_tunnel_mutation_policy(is_desired, is_declared_stale),
            "promotion_required": promotion_required,
            "promotion_hint": if promotion_required {
                Some("promote_to_external_observed_or_adapter_plan_before_mutation")
            } else {
                None
            },
            "operstate": read_sysfs_string(&interface_path.join("operstate")).await,
            "mtu": read_sysfs_u64(&interface_path.join("mtu")).await,
            "link_type": link_type,
        }));
        if tunnels.len() >= MAX_OBSERVED_TUNNELS {
            break;
        }
    }
    tunnels.sort_by(|left, right| {
        left["interface"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["interface"].as_str().unwrap_or_default())
    });
    tunnels
}

fn observed_tunnel_mutation_policy(is_desired: bool, is_declared_stale: bool) -> &'static str {
    if is_desired {
        "managed_desired"
    } else if is_declared_stale {
        "delete_allowed_when_declared_stale"
    } else {
        "observe_only_import_candidate"
    }
}

fn classify_runtime_tunnel(interface: &str, link_type: Option<i64>) -> Option<&'static str> {
    let lower = interface.to_ascii_lowercase();
    match link_type {
        Some(778) => Some("gre"),
        Some(776) => Some("sit"),
        Some(768) => Some("ipip"),
        Some(65534) if lower.starts_with("tun") || lower.starts_with("tap") => Some("tun_tap"),
        _ if lower.starts_with("wg") => Some("wireguard"),
        _ if lower.starts_with("tun") || lower.starts_with("tap") => Some("tun_tap"),
        _ if lower.starts_with("gre") => Some("gre"),
        _ if lower.starts_with("ipip") => Some("ipip"),
        _ if lower.starts_with("sit") => Some("sit"),
        _ if lower.contains("openvpn") || lower.starts_with("ovpn") => Some("openvpn"),
        _ if lower.starts_with("vpn") || lower.starts_with("vps") => Some("custom"),
        _ => None,
    }
}

async fn read_sysfs_string(path: &Path) -> Option<String> {
    read_small_text(path)
        .await
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn read_sysfs_u64(path: &Path) -> Option<u64> {
    read_sysfs_string(path).await?.parse().ok()
}

async fn read_sysfs_i64(path: &Path) -> Option<i64> {
    read_sysfs_string(path).await?.parse().ok()
}

async fn read_small_text(path: &Path) -> Result<Option<String>> {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return Ok(None);
    };
    if !metadata.is_file() || metadata.len() > 4096 {
        return Ok(None);
    }
    Ok(Some(tokio::fs::read_to_string(path).await?))
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
            "real_kernel_readonly_ip_json"
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
) -> serde_json::Value {
    if root != Path::new("/") {
        return serde_json::json!({
            "configured": false,
            "skipped": true,
            "reason": "kernel_probes_require_real_root_namespace",
            "probe_scope": "read_only",
        });
    }
    if config.runtime_ip_argv.is_empty() {
        return serde_json::json!({
            "configured": false,
            "skipped": true,
            "reason": "runtime_ip_argv_unconfigured",
            "probe_scope": "read_only",
        });
    }
    let link = run_kernel_ip_probe(
        config,
        "kernel_link",
        plan,
        endpoint,
        &["-j", "-s", "link", "show", "dev", "{interface}"],
    )
    .await;
    let neighbors = run_kernel_ip_probe(
        config,
        "kernel_neighbors",
        plan,
        endpoint,
        &["-j", "neigh", "show", "dev", "{interface}"],
    )
    .await;
    let routes = run_kernel_ip_probe(
        config,
        "kernel_routes",
        plan,
        endpoint,
        &["-j", "route", "show", "dev", "{interface}"],
    )
    .await;
    serde_json::json!({
        "configured": true,
        "probe_scope": "read_only",
        "link": link,
        "neighbors": neighbors,
        "routes": routes,
    })
}

async fn run_kernel_ip_probe(
    config: &AgentNetworkConfig,
    label: &str,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    args: &[&str],
) -> serde_json::Value {
    let mut argv = config.runtime_ip_argv.clone();
    argv.extend(args.iter().map(|part| part.to_string()));
    let argv = render_probe_argv(&argv, plan, endpoint);
    match run_status_probe(
        label,
        &argv,
        config.status_probe_timeout_secs,
        config.status_probe_max_output_bytes as usize,
    )
    .await
    {
        Ok(report) => report,
        Err(error) => serde_json::json!({
            "configured": true,
            "label": label,
            "argv": argv,
            "success": false,
            "error": error.to_string(),
        }),
    }
}

async fn inspect_bird2_status(
    config: &AgentNetworkConfig,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> serde_json::Value {
    if config.bird2_status_argv.is_empty() {
        return serde_json::json!({
            "configured": false,
            "skipped": true,
        });
    }
    let argv = render_probe_argv(&config.bird2_status_argv, plan, endpoint);
    match run_status_probe(
        "bird2_status",
        &argv,
        config.status_probe_timeout_secs,
        config.status_probe_max_output_bytes as usize,
    )
    .await
    {
        Ok(mut report) => {
            let parsed = report["stdout"]["text"]
                .as_str()
                .map(|text| parse_bird2_ospf_status(text, &plan.interface_name))
                .unwrap_or_else(|| {
                    serde_json::json!({
                        "parser": "bird2_ospf_status_v1",
                        "parsed": false,
                        "reason": "stdout_not_utf8",
                    })
                });
            if let Some(object) = report.as_object_mut() {
                object.insert(
                    "healthy".to_string(),
                    parsed
                        .get("healthy")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                );
                object.insert("parsed_ospf".to_string(), parsed);
            }
            report
        }
        Err(error) => serde_json::json!({
            "configured": true,
            "label": "bird2_status",
            "argv": argv,
            "success": false,
            "error": error.to_string(),
        }),
    }
}

async fn inspect_runtime_adapter_status(
    config: &AgentNetworkConfig,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> serde_json::Value {
    match plan.runtime_control.manager {
        RuntimeTunnelManager::AgentIproute2Managed => serde_json::json!({
            "configured": false,
            "skipped": true,
            "reason": "agent_iproute2_managed",
        }),
        RuntimeTunnelManager::ExternalObserved => serde_json::json!({
            "configured": false,
            "skipped": true,
            "reason": "external_observed",
        }),
        RuntimeTunnelManager::ExternalManagedAdapter => {
            let Some(command) = &plan.runtime_control.status else {
                return serde_json::json!({
                    "configured": false,
                    "skipped": true,
                    "reason": "adapter_status_unconfigured",
                });
            };
            let argv = match render_runtime_adapter_command(command, plan, endpoint) {
                Ok(argv) => argv,
                Err(error) => {
                    return serde_json::json!({
                        "configured": true,
                        "label": "runtime_adapter_status",
                        "success": false,
                        "error": error.to_string(),
                    });
                }
            };
            match run_status_probe(
                "runtime_adapter_status",
                &argv,
                command
                    .timeout_secs
                    .min(config.runtime_command_timeout_secs)
                    .max(1),
                usize::try_from(
                    command
                        .max_output_bytes
                        .min(config.runtime_command_max_output_bytes),
                )
                .unwrap_or(config.runtime_command_max_output_bytes as usize),
            )
            .await
            {
                Ok(report) => report,
                Err(error) => serde_json::json!({
                    "configured": true,
                    "label": "runtime_adapter_status",
                    "argv": argv,
                    "success": false,
                    "error": error.to_string(),
                }),
            }
        }
    }
}

struct RuntimeStatusSummaryInput<'a> {
    plan: &'a TunnelPlan,
    interface: &'a serde_json::Value,
    desired_interfaces: &'a [serde_json::Value],
    declared_stale_interfaces: &'a [serde_json::Value],
    observed_tunnels: &'a [serde_json::Value],
    kernel_namespace: &'a serde_json::Value,
    kernel: &'a serde_json::Value,
    adapter: &'a serde_json::Value,
    bird2: &'a serde_json::Value,
}

fn summarize_runtime_status(input: RuntimeStatusSummaryInput<'_>) -> serde_json::Value {
    let RuntimeStatusSummaryInput {
        plan,
        interface,
        desired_interfaces,
        declared_stale_interfaces,
        observed_tunnels,
        kernel_namespace,
        kernel,
        adapter,
        bird2,
    } = input;
    let mut reasons = Vec::new();
    let interface_exists = interface["exists"].as_bool().unwrap_or(false);
    let interface_operstate = interface["operstate"].as_str();
    if !interface_exists {
        reasons.push("runtime_interface_missing");
    } else if matches!(interface_operstate, Some("down")) {
        reasons.push("runtime_interface_down");
    }

    let desired_missing_count = desired_interfaces
        .iter()
        .filter(|report| report["exists"].as_bool() != Some(true))
        .count();
    if desired_missing_count > 0 {
        reasons.push("desired_interface_missing");
    }
    let stale_present_count = declared_stale_interfaces
        .iter()
        .filter(|report| report["exists"].as_bool() == Some(true))
        .count();
    if stale_present_count > 0 {
        reasons.push("stale_interface_present");
    }
    let external_import_candidate_count = observed_tunnels
        .iter()
        .filter(|report| report["import_candidate"].as_bool() == Some(true))
        .count();

    let adapter_state = match plan.runtime_control.manager {
        RuntimeTunnelManager::AgentIproute2Managed => "not_applicable",
        RuntimeTunnelManager::ExternalObserved => "observed_only",
        RuntimeTunnelManager::ExternalManagedAdapter => {
            if adapter["success"].as_bool() == Some(true) {
                "healthy"
            } else if adapter["configured"].as_bool() == Some(false) {
                reasons.push("adapter_status_unconfigured");
                "unknown"
            } else {
                reasons.push("adapter_status_failed");
                "unhealthy"
            }
        }
    };

    let bird2_state = if bird2["configured"].as_bool() == Some(false) {
        "not_configured"
    } else if bird2["healthy"].as_bool() == Some(true) {
        "healthy"
    } else if bird2["healthy"].as_bool() == Some(false) {
        reasons.push("bird2_not_full");
        "unhealthy"
    } else {
        "unknown"
    };
    let real_kernel_namespace_covered = kernel_namespace["real_kernel_namespace"]
        .as_bool()
        .unwrap_or(false);
    let kernel_link_probe_state = probe_state(&kernel["link"]);
    let neighbor_probe_state = probe_state(&kernel["neighbors"]);
    let route_probe_state = probe_state(&kernel["routes"]);

    let healthy = reasons.is_empty();
    let status =
        if plan.runtime_control.manager == RuntimeTunnelManager::ExternalObserved && healthy {
            "observed"
        } else if healthy {
            "healthy"
        } else if reasons.contains(&"runtime_interface_missing")
            || reasons.contains(&"desired_interface_missing")
            || reasons.contains(&"stale_interface_present")
        {
            "drift"
        } else if reasons.contains(&"adapter_status_failed") {
            "adapter_unhealthy"
        } else if reasons.contains(&"bird2_not_full") {
            "routing_unhealthy"
        } else {
            "degraded"
        };

    serde_json::json!({
        "manager": plan.runtime_control.manager,
        "status": status,
        "healthy": healthy,
        "drift": matches!(status, "drift"),
        "reasons": reasons,
        "interface_exists": interface_exists,
        "interface_operstate": interface_operstate,
        "desired_missing_count": desired_missing_count,
        "stale_present_count": stale_present_count,
        "external_import_candidate_count": external_import_candidate_count,
        "external_import_candidate_policy": if external_import_candidate_count > 0 {
            "observe_only_requires_plan_promotion"
        } else {
            "none"
        },
        "adapter_state": adapter_state,
        "bird2_state": bird2_state,
        "real_kernel_namespace_covered": real_kernel_namespace_covered,
        "kernel_link_probe_state": kernel_link_probe_state,
        "neighbor_probe_state": neighbor_probe_state,
        "route_probe_state": route_probe_state,
        "topology_version": &plan.runtime_topology.version,
    })
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

fn parse_bird2_ospf_status(output: &str, interface_name: &str) -> serde_json::Value {
    let mut interface_seen = false;
    let mut full_neighbor_seen = false;
    let mut neighbor_state_count = 0_u64;
    let mut state_counts = BTreeMap::<String, u64>::new();
    let mut interface_states = Vec::new();

    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let references_interface = line_references_interface(line, interface_name);
        interface_seen |= references_interface;
        let Some(state) = extract_ospf_neighbor_state(line) else {
            continue;
        };
        neighbor_state_count += 1;
        *state_counts.entry(state.to_string()).or_default() += 1;
        if references_interface {
            interface_states.push(serde_json::json!({
                "state": state,
            }));
        }
        if references_interface && state == "full" {
            full_neighbor_seen = true;
        }
    }

    serde_json::json!({
        "parser": "bird2_ospf_status_v1",
        "parsed": true,
        "interface": interface_name,
        "interface_seen": interface_seen,
        "full_neighbor_seen": full_neighbor_seen,
        "healthy": interface_seen && full_neighbor_seen,
        "neighbor_state_count": neighbor_state_count,
        "state_counts": state_counts,
        "interface_states": interface_states,
    })
}

fn line_references_interface(line: &str, interface_name: &str) -> bool {
    let expected = interface_name.to_ascii_lowercase();
    line.split_whitespace()
        .map(|part| {
            part.trim_matches(|character: char| {
                matches!(
                    character,
                    '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']'
                )
            })
            .to_ascii_lowercase()
        })
        .any(|part| part == expected)
}

fn extract_ospf_neighbor_state(line: &str) -> Option<&'static str> {
    line.split_whitespace()
        .filter_map(normalize_ospf_state_token)
        .next()
}

fn normalize_ospf_state_token(token: &str) -> Option<&'static str> {
    let state = token
        .trim_matches(|character: char| {
            matches!(
                character,
                '"' | '\'' | ',' | ';' | ':' | '(' | ')' | '[' | ']'
            )
        })
        .to_ascii_lowercase();
    let state = state.split('/').next().unwrap_or(&state);
    match state {
        "full" => Some("full"),
        "2way" | "two-way" | "twoway" => Some("two_way"),
        "exstart" => Some("exstart"),
        "exchange" => Some("exchange"),
        "loading" => Some("loading"),
        "init" => Some("init"),
        "attempt" => Some("attempt"),
        "down" => Some("down"),
        _ => None,
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
    timeout_secs: u64,
    max_output_bytes: usize,
) -> Result<serde_json::Value> {
    if argv.is_empty() {
        anyhow::bail!("network status probe {label} argv is empty");
    }
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    command.kill_on_drop(true);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to run network status probe {label}"))?;
    let stdout = child
        .stdout
        .take()
        .context("network status probe stdout pipe missing")?;
    let stderr = child
        .stderr
        .take()
        .context("network status probe stderr pipe missing")?;
    let mut stdout_task = Some(tokio::spawn(read_limited(stdout, max_output_bytes)));
    let mut stderr_task = Some(tokio::spawn(read_limited(stderr, max_output_bytes)));
    let deadline = Instant::now() + Duration::from_secs(timeout_secs.clamp(1, 30));
    let mut timed_out = false;
    let mut killed_for_output_limit = false;
    let mut stdout_output = None;
    let mut stderr_output = None;

    let status = loop {
        if let Some(exit_status) = child.try_wait()? {
            break Some(exit_status);
        }
        if stdout_output.is_none() && task_is_finished(&stdout_task) {
            let output = join_limited(stdout_task.take()).await?;
            killed_for_output_limit |= output.truncated;
            stdout_output = Some(output);
        }
        if stderr_output.is_none() && task_is_finished(&stderr_task) {
            let output = join_limited(stderr_task.take()).await?;
            killed_for_output_limit |= output.truncated;
            stderr_output = Some(output);
        }
        if killed_for_output_limit {
            child.start_kill()?;
            break child.wait().await.ok();
        }
        if Instant::now() >= deadline {
            timed_out = true;
            child.start_kill()?;
            break child.wait().await.ok();
        }
        time::sleep(Duration::from_millis(20)).await;
    };

    let stdout = match stdout_output {
        Some(output) => output,
        None => join_limited(stdout_task.take()).await?,
    };
    let stderr = match stderr_output {
        Some(output) => output,
        None => join_limited(stderr_task.take()).await?,
    };
    Ok(probe_report(ProbeReportInput {
        label,
        argv,
        status,
        timed_out,
        killed_for_output_limit,
        max_output_bytes,
        stdout,
        stderr,
    }))
}

fn task_is_finished(task: &Option<JoinHandle<std::io::Result<LimitedOutput>>>) -> bool {
    task.as_ref().is_some_and(JoinHandle::is_finished)
}

async fn join_limited(
    task: Option<JoinHandle<std::io::Result<LimitedOutput>>>,
) -> Result<LimitedOutput> {
    let task = task.context("network status probe output task missing")?;
    Ok(task
        .await
        .context("network status probe output task panicked")??)
}

async fn read_limited<R>(mut reader: R, limit: usize) -> std::io::Result<LimitedOutput>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut chunk = [0_u8; 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        if read > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok(LimitedOutput { bytes, truncated })
}

struct ProbeReportInput<'a> {
    label: &'a str,
    argv: &'a [String],
    status: Option<ExitStatus>,
    timed_out: bool,
    killed_for_output_limit: bool,
    max_output_bytes: usize,
    stdout: LimitedOutput,
    stderr: LimitedOutput,
}

fn probe_report(input: ProbeReportInput<'_>) -> serde_json::Value {
    let success = input.status.as_ref().is_some_and(|status| status.success())
        && !input.timed_out
        && !input.killed_for_output_limit;
    let exit_code = input.status.as_ref().and_then(ExitStatus::code);
    serde_json::json!({
        "configured": true,
        "label": input.label,
        "argv": input.argv,
        "success": success,
        "exit_code": exit_code,
        "timed_out": input.timed_out,
        "killed_for_output_limit": input.killed_for_output_limit,
        "max_output_bytes": input.max_output_bytes,
        "stdout": output_json(input.stdout),
        "stderr": output_json(input.stderr),
    })
}

fn output_json(output: LimitedOutput) -> serde_json::Value {
    let utf8 = std::str::from_utf8(&output.bytes).ok();
    serde_json::json!({
        "size_bytes": output.bytes.len(),
        "sha256_hex": payload_hash(&output.bytes),
        "truncated": output.truncated,
        "utf8": utf8.is_some(),
        "text": utf8.map(str::to_string),
    })
}

struct LimitedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

#[cfg(test)]
mod tests;
