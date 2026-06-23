use std::{net::IpAddr, path::Path, time::Duration};

use anyhow::{Context, Result};
use tokio::time;
use vpsman_common::{
    render_tunnel_endpoint_config, AgentConfig, AgentRuntimeUnprivilegedMutationPolicy,
    RuntimeTunnelCommand, RuntimeTunnelManager, RuntimeTunnelRoute, RuntimeTunnelTrafficLimit,
    TunnelEndpointConfig, TunnelEndpointSide, TunnelKind, TunnelPlan,
};

mod command_runner;

use crate::command_worker::CommandCancelToken;

use self::command_runner::run_runtime_command_cancelable;

pub(crate) struct NetworkRuntimeReconcileInput<'a> {
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) max_timeout_secs: u64,
    #[cfg(test)]
    pub(crate) effective_uid_override: Option<u32>,
}

pub(crate) struct NetworkRuntimeRemoveInput<'a> {
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) max_timeout_secs: u64,
    #[cfg(test)]
    pub(crate) effective_uid_override: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeCommandSpec {
    label: &'static str,
    argv: Vec<String>,
    mutates: bool,
    required: bool,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn execute_runtime_tunnel_reconcile_report(
    input: NetworkRuntimeReconcileInput<'_>,
) -> Result<serde_json::Value> {
    execute_runtime_tunnel_reconcile_report_cancelable(input, CommandCancelToken::default()).await
}

pub(crate) async fn execute_runtime_tunnel_reconcile_report_cancelable(
    input: NetworkRuntimeReconcileInput<'_>,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    time::timeout(
        Duration::from_secs(input.max_timeout_secs.max(1)),
        reconcile_runtime_tunnel(input, cancel_token),
    )
    .await
    .context("runtime tunnel reconcile timed out")?
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn execute_runtime_tunnel_remove_report(
    input: NetworkRuntimeRemoveInput<'_>,
) -> Result<serde_json::Value> {
    execute_runtime_tunnel_remove_report_cancelable(input, CommandCancelToken::default()).await
}

pub(crate) async fn execute_runtime_tunnel_remove_report_cancelable(
    input: NetworkRuntimeRemoveInput<'_>,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    time::timeout(
        Duration::from_secs(input.max_timeout_secs.max(1)),
        remove_runtime_tunnel(input, cancel_token),
    )
    .await
    .context("runtime tunnel remove timed out")?
}

async fn reconcile_runtime_tunnel(
    input: NetworkRuntimeReconcileInput<'_>,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    let endpoint = render_tunnel_endpoint_config(input.plan, input.side)
        .map_err(|error| anyhow::anyhow!("invalid runtime tunnel endpoint config: {error}"))?;
    if endpoint.local_client_id != input.config.client_id {
        anyhow::bail!(
            "runtime tunnel side targets {}, but this agent is {}",
            endpoint.local_client_id,
            input.config.client_id
        );
    }

    if !input.config.network.runtime_reconcile_enabled {
        return Ok(serde_json::json!({
            "type": "runtime_tunnel_reconcile",
            "status": "skipped",
            "reason": "runtime_reconcile_disabled",
            "plan": input.plan.name,
            "interface": input.plan.interface_name,
        }));
    }
    if !input.config.network.apply_enabled {
        return Ok(serde_json::json!({
            "type": "runtime_tunnel_reconcile",
            "status": "skipped",
            "reason": "runtime_tunnel_mutation_disabled",
            "plan": input.plan.name,
            "interface": input.plan.interface_name,
        }));
    }

    let root = Path::new(&input.config.network.root_dir);
    let link_exists = runtime_link_exists(root, &input.plan.interface_name).await;
    let mut preflight_reports = Vec::new();
    let mut existing_link_validation = serde_json::Value::Null;
    if link_exists
        && input.plan.runtime_control.manager == RuntimeTunnelManager::AgentIproute2Managed
    {
        let (reports, validation) = validate_existing_iproute2_tunnel(
            input.config,
            input.plan,
            &endpoint,
            cancel_token.clone(),
        )
        .await?;
        preflight_reports.extend(reports);
        existing_link_validation = validation;
    }
    let cleanup_specs = build_runtime_topology_cleanup_steps(input.config, input.plan)?;
    let specs = match input.plan.runtime_control.manager {
        RuntimeTunnelManager::AgentIproute2Managed => {
            build_iproute2_reconcile_steps(input.config, input.plan, &endpoint, link_exists)?
        }
        RuntimeTunnelManager::ExternalObserved => Vec::new(),
        RuntimeTunnelManager::ExternalManagedAdapter => {
            build_external_adapter_steps(input.plan, &endpoint)?
        }
    };

    let specs = cleanup_specs.into_iter().chain(specs).collect::<Vec<_>>();
    let effective_uid = effective_uid(input.effective_uid_override());
    let unprivileged_mutation_policy = input.config.network.runtime_unprivileged_mutation_policy;
    let mut reports = preflight_reports;
    let mut degraded = false;
    let mut failed = false;
    let mut failed_required_label = None;
    for spec in specs {
        if should_skip_unprivileged_mutation(
            spec.mutates,
            effective_uid,
            input.plan.runtime_control.manager,
            unprivileged_mutation_policy,
        ) {
            degraded = true;
            reports.push(serde_json::json!({
                "label": spec.label,
                "argv": spec.argv,
                "mutates": spec.mutates,
                "required": spec.required,
                "skipped": true,
                "success": false,
                "reason": "agent_unprivileged",
            }));
            if spec.required {
                failed = true;
            }
            continue;
        }
        let report = run_runtime_command_cancelable(
            spec.label,
            &spec.argv,
            spec.mutates,
            spec.required,
            input.config.network.runtime_command_timeout_secs,
            input.config.network.runtime_command_max_output_bytes as usize,
            cancel_token.clone(),
        )
        .await?;
        if spec.required && report["success"].as_bool() != Some(true) {
            failed = true;
            failed_required_label = Some(spec.label);
            reports.push(report);
            break;
        }
        reports.push(report);
    }
    let compensation = if failed && !degraded {
        Some(
            run_runtime_compensation(
                input.config,
                input.plan,
                &endpoint,
                link_exists,
                failed_required_label.unwrap_or("unknown_required_step"),
                cancel_token.clone(),
            )
            .await?,
        )
    } else {
        None
    };

    let status = if input.plan.runtime_control.manager == RuntimeTunnelManager::ExternalObserved {
        "observed_only"
    } else if failed {
        "failed"
    } else if degraded {
        "degraded_unprivileged"
    } else {
        "converged"
    };

    Ok(serde_json::json!({
        "type": "runtime_tunnel_reconcile",
        "status": status,
        "plan": input.plan.name,
        "interface": input.plan.interface_name,
        "side": side_name(input.side),
        "client_id": input.config.client_id,
        "peer_client_id": endpoint.peer_client_id,
        "manager": input.plan.runtime_control.manager,
        "topology_version": &input.plan.runtime_topology.version,
        "desired_interfaces": &input.plan.runtime_topology.desired_interfaces,
        "stale_interfaces": &input.plan.runtime_topology.stale_interfaces,
        "link_existed_before": link_exists,
        "existing_link_validation": existing_link_validation,
        "effective_uid": effective_uid,
        "unprivileged_mutation_policy": unprivileged_mutation_policy,
        "commands": reports,
        "compensation": compensation,
    }))
}

async fn remove_runtime_tunnel(
    input: NetworkRuntimeRemoveInput<'_>,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    let endpoint = render_tunnel_endpoint_config(input.plan, input.side)
        .map_err(|error| anyhow::anyhow!("invalid runtime tunnel endpoint config: {error}"))?;
    if endpoint.local_client_id != input.config.client_id {
        anyhow::bail!(
            "runtime tunnel side targets {}, but this agent is {}",
            endpoint.local_client_id,
            input.config.client_id
        );
    }

    if !input.config.network.runtime_reconcile_enabled {
        return Ok(serde_json::json!({
            "type": "runtime_tunnel_remove",
            "status": "skipped",
            "reason": "runtime_reconcile_disabled",
            "plan": input.plan.name,
            "interface": input.plan.interface_name,
        }));
    }
    if !input.config.network.apply_enabled {
        return Ok(serde_json::json!({
            "type": "runtime_tunnel_remove",
            "status": "skipped",
            "reason": "runtime_tunnel_mutation_disabled",
            "plan": input.plan.name,
            "interface": input.plan.interface_name,
        }));
    }

    let root = Path::new(&input.config.network.root_dir);
    let link_exists = runtime_link_exists(root, &input.plan.interface_name).await;
    let specs = match input.plan.runtime_control.manager {
        RuntimeTunnelManager::AgentIproute2Managed => {
            build_iproute2_remove_steps(input.config, input.plan, link_exists)?
        }
        RuntimeTunnelManager::ExternalObserved => Vec::new(),
        RuntimeTunnelManager::ExternalManagedAdapter => {
            build_external_adapter_remove_steps(input.plan, &endpoint)?
        }
    };
    let effective_uid = effective_uid(input.effective_uid_override());
    let unprivileged_mutation_policy = input.config.network.runtime_unprivileged_mutation_policy;
    let adapter_remove_available = input.plan.runtime_control.stop.is_some()
        || input.plan.runtime_control.cleanup.is_some()
        || input.plan.runtime_control.manager != RuntimeTunnelManager::ExternalManagedAdapter;
    let mut reports = Vec::new();
    let mut degraded = false;
    let mut failed = false;
    for spec in specs {
        if should_skip_unprivileged_mutation(
            spec.mutates,
            effective_uid,
            input.plan.runtime_control.manager,
            unprivileged_mutation_policy,
        ) {
            degraded = true;
            reports.push(serde_json::json!({
                "label": spec.label,
                "argv": spec.argv,
                "mutates": spec.mutates,
                "required": spec.required,
                "skipped": true,
                "success": false,
                "reason": "agent_unprivileged",
            }));
            if spec.required {
                failed = true;
            }
            continue;
        }
        let report = run_runtime_command_cancelable(
            spec.label,
            &spec.argv,
            spec.mutates,
            spec.required,
            input.config.network.runtime_command_timeout_secs,
            input.config.network.runtime_command_max_output_bytes as usize,
            cancel_token.clone(),
        )
        .await?;
        if spec.required && report["success"].as_bool() != Some(true) {
            failed = true;
        }
        reports.push(report);
    }

    let status = if input.plan.runtime_control.manager == RuntimeTunnelManager::ExternalObserved {
        "observed_only"
    } else if failed {
        "failed"
    } else if degraded {
        "degraded_unprivileged"
    } else if !adapter_remove_available {
        "remove_unavailable"
    } else {
        "removed"
    };

    Ok(serde_json::json!({
        "type": "runtime_tunnel_remove",
        "status": status,
        "plan": input.plan.name,
        "interface": input.plan.interface_name,
        "side": side_name(input.side),
        "client_id": input.config.client_id,
        "peer_client_id": endpoint.peer_client_id,
        "manager": input.plan.runtime_control.manager,
        "link_existed_before": link_exists,
        "effective_uid": effective_uid,
        "unprivileged_mutation_policy": unprivileged_mutation_policy,
        "commands": reports,
    }))
}

fn should_skip_unprivileged_mutation(
    mutates: bool,
    effective_uid: u32,
    manager: RuntimeTunnelManager,
    policy: AgentRuntimeUnprivilegedMutationPolicy,
) -> bool {
    if !mutates || effective_uid == 0 {
        return false;
    }
    match policy {
        AgentRuntimeUnprivilegedMutationPolicy::Skip => true,
        AgentRuntimeUnprivilegedMutationPolicy::TryExternalAdapters => {
            manager != RuntimeTunnelManager::ExternalManagedAdapter
        }
        AgentRuntimeUnprivilegedMutationPolicy::TryAll => false,
    }
}

fn build_iproute2_reconcile_steps(
    config: &AgentConfig,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    link_exists: bool,
) -> Result<Vec<RuntimeCommandSpec>> {
    ensure_command_base(&config.network.runtime_ip_argv, "runtime ip")?;
    let mut steps = Vec::new();
    steps.push(RuntimeCommandSpec {
        label: "runtime_link_show",
        argv: extend_argv(
            &config.network.runtime_ip_argv,
            ["link", "show", "dev", &plan.interface_name],
        ),
        mutates: false,
        required: false,
    });

    if plan.kind == TunnelKind::Fou {
        let fou_port = plan.runtime_control.fou.port.to_string();
        let fou_ipproto = plan.runtime_control.fou.ipproto.to_string();
        steps.push(RuntimeCommandSpec {
            label: "runtime_fou_add",
            argv: extend_argv(
                &config.network.runtime_ip_argv,
                ["fou", "add", "port", &fou_port, "ipproto", &fou_ipproto],
            ),
            mutates: true,
            required: false,
        });
    }

    if !link_exists {
        steps.push(RuntimeCommandSpec {
            label: "runtime_tunnel_add",
            argv: build_ip_tunnel_argv(&config.network.runtime_ip_argv, "add", plan, endpoint)?,
            mutates: true,
            required: true,
        });
    }
    steps.push(RuntimeCommandSpec {
        label: "runtime_addr_replace",
        argv: extend_argv(
            &config.network.runtime_ip_argv,
            [
                "addr",
                "replace",
                &format!(
                    "{}/{}",
                    local_address(plan, endpoint),
                    plan.tunnel_prefix_len
                ),
                "peer",
                remote_address(plan, endpoint),
                "dev",
                &plan.interface_name,
            ],
        ),
        mutates: true,
        required: true,
    });
    steps.push(RuntimeCommandSpec {
        label: "runtime_link_up",
        argv: extend_argv(
            &config.network.runtime_ip_argv,
            ["link", "set", "dev", &plan.interface_name, "up"],
        ),
        mutates: true,
        required: true,
    });
    steps.extend(build_route_replace_steps(
        &config.network.runtime_ip_argv,
        &plan.interface_name,
        &plan.runtime_topology.routes,
    )?);
    steps.extend(build_traffic_limit_steps(
        &config.network.runtime_tc_argv,
        &plan.interface_name,
        &plan.runtime_control.traffic_limit,
    )?);
    Ok(steps)
}

fn build_runtime_topology_cleanup_steps(
    config: &AgentConfig,
    plan: &TunnelPlan,
) -> Result<Vec<RuntimeCommandSpec>> {
    ensure_command_base(&config.network.runtime_ip_argv, "runtime ip")?;
    let mut steps = Vec::new();
    for route in &plan.runtime_topology.stale_routes {
        steps.push(RuntimeCommandSpec {
            label: "runtime_route_delete",
            argv: build_ip_route_argv(
                &config.network.runtime_ip_argv,
                "del",
                route,
                &plan.interface_name,
            ),
            mutates: true,
            required: false,
        });
    }
    for interface in &plan.runtime_topology.stale_interfaces {
        steps.push(RuntimeCommandSpec {
            label: "runtime_stale_link_delete",
            argv: extend_argv(
                &config.network.runtime_ip_argv,
                ["link", "delete", "dev", interface],
            ),
            mutates: true,
            required: false,
        });
    }
    Ok(steps)
}

fn build_iproute2_remove_steps(
    config: &AgentConfig,
    plan: &TunnelPlan,
    link_exists: bool,
) -> Result<Vec<RuntimeCommandSpec>> {
    ensure_command_base(&config.network.runtime_ip_argv, "runtime ip")?;
    let mut steps = Vec::new();
    for route in &plan.runtime_topology.routes {
        steps.push(RuntimeCommandSpec {
            label: "runtime_route_delete",
            argv: build_ip_route_argv(
                &config.network.runtime_ip_argv,
                "del",
                route,
                &plan.interface_name,
            ),
            mutates: true,
            required: false,
        });
    }
    if link_exists {
        steps.push(RuntimeCommandSpec {
            label: "runtime_link_delete",
            argv: extend_argv(
                &config.network.runtime_ip_argv,
                ["link", "delete", "dev", &plan.interface_name],
            ),
            mutates: true,
            required: true,
        });
    }
    if plan.kind == TunnelKind::Fou {
        let fou_port = plan.runtime_control.fou.port.to_string();
        steps.push(RuntimeCommandSpec {
            label: "runtime_fou_delete",
            argv: extend_argv(
                &config.network.runtime_ip_argv,
                ["fou", "del", "port", &fou_port],
            ),
            mutates: true,
            required: false,
        });
    }
    Ok(steps)
}

fn build_ip_tunnel_argv(
    base: &[String],
    action: &str,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Result<Vec<String>> {
    let tunnel_mode = linux_tunnel_mode(plan.kind)?;
    let mut argv = extend_argv(
        base,
        [
            "tunnel",
            action,
            &plan.interface_name,
            "mode",
            tunnel_mode,
            "remote",
            remote_underlay(plan, endpoint),
            "local",
            local_underlay(plan, endpoint),
            "ttl",
            "255",
        ],
    );
    if plan.kind == TunnelKind::Fou {
        argv.extend([
            "encap".to_string(),
            "fou".to_string(),
            "encap-sport".to_string(),
            "auto".to_string(),
            "encap-dport".to_string(),
            plan.runtime_control.fou.peer_port.to_string(),
        ]);
    }
    Ok(argv)
}

async fn validate_existing_iproute2_tunnel(
    config: &AgentConfig,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    cancel_token: CommandCancelToken,
) -> Result<(Vec<serde_json::Value>, serde_json::Value)> {
    ensure_command_base(&config.network.runtime_ip_argv, "runtime ip")?;
    let link_argv = extend_argv(
        &config.network.runtime_ip_argv,
        [
            "-details",
            "-json",
            "link",
            "show",
            "dev",
            &plan.interface_name,
        ],
    );
    let link_report = run_runtime_command_cancelable(
        "runtime_tunnel_inspect",
        &link_argv,
        false,
        true,
        config.network.runtime_command_timeout_secs,
        config.network.runtime_command_max_output_bytes as usize,
        cancel_token.clone(),
    )
    .await?;
    if link_report["success"].as_bool() != Some(true) {
        anyhow::bail!(
            "existing runtime tunnel {} could not be inspected: {}",
            plan.interface_name,
            runtime_report_failure_summary(&link_report)
        );
    }
    let link_stdout = link_report["stdout"]["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("existing runtime tunnel inspect output was not UTF-8"))?;
    let link = parse_iproute2_link_json(link_stdout, &plan.interface_name)?;

    let addr_argv = extend_argv(
        &config.network.runtime_ip_argv,
        [
            "-details",
            "-json",
            "addr",
            "show",
            "dev",
            &plan.interface_name,
        ],
    );
    let addr_report = run_runtime_command_cancelable(
        "runtime_addr_inspect",
        &addr_argv,
        false,
        true,
        config.network.runtime_command_timeout_secs,
        config.network.runtime_command_max_output_bytes as usize,
        cancel_token,
    )
    .await?;
    if addr_report["success"].as_bool() != Some(true) {
        anyhow::bail!(
            "existing runtime tunnel {} address assignment could not be inspected: {}",
            plan.interface_name,
            runtime_report_failure_summary(&addr_report)
        );
    }
    let addr_stdout = addr_report["stdout"]["text"].as_str().ok_or_else(|| {
        anyhow::anyhow!("existing runtime tunnel address inspect output was not UTF-8")
    })?;
    let addresses = parse_iproute2_addr_json(addr_stdout, &plan.interface_name)?;

    let mut mismatches = existing_iproute2_tunnel_mismatches(&link, plan, endpoint)?;
    let matched_address = matching_existing_iproute2_address(&addresses, plan, endpoint);
    if matched_address.is_none() {
        mismatches.push(address_mismatch_message(&addresses, plan, endpoint));
    }
    if !mismatches.is_empty() {
        anyhow::bail!(
            "existing runtime tunnel {} does not match saved plan: {}",
            plan.interface_name,
            mismatches.join("; ")
        );
    }
    Ok((
        vec![link_report, addr_report],
        serde_json::json!({
            "status": "matched",
            "interface": plan.interface_name,
            "mode": link.kind,
            "local_underlay": link.local,
            "remote_underlay": link.remote,
            "ttl": link.ttl,
            "encap": link.encap,
            "encap_dport": link.encap_dport,
            "address": matched_address,
        }),
    ))
}

#[derive(Debug)]
struct ExistingIproute2Tunnel {
    kind: Option<String>,
    local: Option<String>,
    remote: Option<String>,
    ttl: Option<String>,
    encap: Option<String>,
    encap_dport: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
struct ExistingIproute2Address {
    family: Option<String>,
    local: Option<String>,
    prefix_len: Option<u8>,
    peer: Option<String>,
}

fn parse_iproute2_link_json(stdout: &str, interface_name: &str) -> Result<ExistingIproute2Tunnel> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .context("failed to parse existing runtime tunnel inspect JSON")?;
    let candidates = value
        .as_array()
        .map(|items| items.iter().collect::<Vec<_>>())
        .unwrap_or_else(|| vec![&value]);
    let link = candidates
        .into_iter()
        .find(|candidate| {
            candidate
                .get("ifname")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|ifname| ifname == interface_name)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "existing runtime tunnel inspect JSON did not include interface {interface_name}"
            )
        })?;
    let linkinfo = link.get("linkinfo").unwrap_or(link);
    let data = linkinfo.get("info_data").unwrap_or(linkinfo);
    Ok(ExistingIproute2Tunnel {
        kind: string_field(linkinfo, &["info_kind", "kind"])
            .or_else(|| string_field(data, &["info_kind", "kind", "mode"])),
        local: string_field(data, &["local", "local_address", "local-address"]),
        remote: string_field(data, &["remote", "remote_address", "remote-address"]),
        ttl: string_field(data, &["ttl", "hoplimit", "hop_limit", "hop-limit"]),
        encap: string_field(data, &["encap", "encap_type", "encap-type"]),
        encap_dport: string_field(
            data,
            &[
                "encap_dport",
                "encap-dport",
                "encap_dport_be16",
                "encap-dport-be16",
            ],
        ),
    })
}

fn parse_iproute2_addr_json(
    stdout: &str,
    interface_name: &str,
) -> Result<Vec<ExistingIproute2Address>> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .context("failed to parse existing runtime tunnel address inspect JSON")?;
    let candidates = value
        .as_array()
        .map(|items| items.iter().collect::<Vec<_>>())
        .unwrap_or_else(|| vec![&value]);
    let link = candidates
        .into_iter()
        .find(|candidate| {
            candidate
                .get("ifname")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|ifname| ifname == interface_name)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "existing runtime tunnel address inspect JSON did not include interface {interface_name}"
            )
        })?;
    let Some(addr_info) = link.get("addr_info").and_then(serde_json::Value::as_array) else {
        return Ok(Vec::new());
    };
    Ok(addr_info
        .iter()
        .map(|address| ExistingIproute2Address {
            family: string_field(address, &["family"]),
            local: string_field(address, &["local"]),
            prefix_len: address
                .get("prefixlen")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u8::try_from(value).ok()),
            peer: string_field(address, &["peer", "local_peer", "local-peer"]),
        })
        .collect())
}

fn existing_iproute2_tunnel_mismatches(
    link: &ExistingIproute2Tunnel,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Result<Vec<String>> {
    let expected_mode = linux_tunnel_mode(plan.kind)?;
    let expected_local = local_underlay(plan, endpoint);
    let expected_remote = remote_underlay(plan, endpoint);
    let mut mismatches = Vec::new();
    push_string_mismatch(&mut mismatches, "mode", link.kind.as_deref(), expected_mode);
    push_ip_mismatch(
        &mut mismatches,
        "local_underlay",
        link.local.as_deref(),
        expected_local,
    );
    push_ip_mismatch(
        &mut mismatches,
        "remote_underlay",
        link.remote.as_deref(),
        expected_remote,
    );
    push_string_mismatch(&mut mismatches, "ttl", link.ttl.as_deref(), "255");
    if plan.kind == TunnelKind::Fou {
        push_string_mismatch(&mut mismatches, "encap", link.encap.as_deref(), "fou");
        let expected_dport = plan.runtime_control.fou.peer_port.to_string();
        push_string_mismatch(
            &mut mismatches,
            "encap_dport",
            link.encap_dport.as_deref(),
            &expected_dport,
        );
    }
    Ok(mismatches)
}

fn matching_existing_iproute2_address(
    addresses: &[ExistingIproute2Address],
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Option<ExistingIproute2Address> {
    let expected_local = local_address(plan, endpoint);
    let expected_remote = remote_address(plan, endpoint);
    addresses
        .iter()
        .find(|address| {
            address
                .local
                .as_deref()
                .is_some_and(|actual| ip_values_match(actual, expected_local))
                && address.prefix_len == Some(plan.tunnel_prefix_len)
                && address
                    .peer
                    .as_deref()
                    .is_some_and(|actual| ip_values_match(actual, expected_remote))
        })
        .cloned()
}

fn address_mismatch_message(
    addresses: &[ExistingIproute2Address],
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> String {
    let expected = format!(
        "{}/{} peer {}",
        local_address(plan, endpoint),
        plan.tunnel_prefix_len,
        remote_address(plan, endpoint)
    );
    let actual = if addresses.is_empty() {
        "<missing>".to_string()
    } else {
        addresses
            .iter()
            .map(|address| {
                format!(
                    "{}/{} peer {}",
                    address.local.as_deref().unwrap_or("<missing>"),
                    address
                        .prefix_len
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "<missing>".to_string()),
                    address.peer.as_deref().unwrap_or("<missing>")
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("tunnel_address expected {expected} got {actual}")
}

fn push_string_mismatch(
    mismatches: &mut Vec<String>,
    field: &str,
    actual: Option<&str>,
    expected: &str,
) {
    let actual = actual.map(str::trim).filter(|value| !value.is_empty());
    if actual.is_none_or(|actual| !actual.eq_ignore_ascii_case(expected)) {
        mismatches.push(format!(
            "{field} expected {expected} got {}",
            actual.unwrap_or("<missing>")
        ));
    }
}

fn push_ip_mismatch(
    mismatches: &mut Vec<String>,
    field: &str,
    actual: Option<&str>,
    expected: &str,
) {
    let actual = actual.map(str::trim).filter(|value| !value.is_empty());
    if actual.is_none_or(|actual| !ip_values_match(actual, expected)) {
        mismatches.push(format!(
            "{field} expected {expected} got {}",
            actual.unwrap_or("<missing>")
        ));
    }
}

fn ip_values_match(actual: &str, expected: &str) -> bool {
    if actual == expected {
        return true;
    }
    match (actual.parse::<IpAddr>(), expected.parse::<IpAddr>()) {
        (Ok(actual), Ok(expected)) => actual == expected,
        _ => false,
    }
}

fn string_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        let Some(value) = value.get(*key) else {
            continue;
        };
        if let Some(text) = value.as_str() {
            return Some(text.to_string());
        }
        if let Some(number) = value.as_u64() {
            return Some(number.to_string());
        }
        if let Some(number) = value.as_i64() {
            return Some(number.to_string());
        }
    }
    None
}

fn runtime_report_failure_summary(report: &serde_json::Value) -> String {
    let exit_code = report
        .get("exit_code")
        .and_then(serde_json::Value::as_i64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    let stderr = report["stderr"]["text"].as_str().unwrap_or_default().trim();
    let stdout = report["stdout"]["text"].as_str().unwrap_or_default().trim();
    let detail = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "no command output"
    };
    format!("exit_code={exit_code}, {detail}")
}

fn build_route_replace_steps(
    base: &[String],
    interface_name: &str,
    routes: &[RuntimeTunnelRoute],
) -> Result<Vec<RuntimeCommandSpec>> {
    if routes.is_empty() {
        return Ok(Vec::new());
    }
    ensure_command_base(base, "runtime ip")?;
    Ok(routes
        .iter()
        .map(|route| RuntimeCommandSpec {
            label: "runtime_route_replace",
            argv: build_ip_route_argv(base, "replace", route, interface_name),
            mutates: true,
            required: true,
        })
        .collect())
}

fn build_ip_route_argv(
    base: &[String],
    action: &str,
    route: &RuntimeTunnelRoute,
    default_interface_name: &str,
) -> Vec<String> {
    let interface_name = route
        .interface_name
        .as_deref()
        .unwrap_or(default_interface_name);
    let mut argv = extend_argv(base, ["route", action, &route.destination_cidr]);
    if let Some(via) = &route.via {
        argv.extend(["via".to_string(), via.clone()]);
    }
    argv.extend(["dev".to_string(), interface_name.to_string()]);
    if let Some(metric) = route.metric {
        argv.extend(["metric".to_string(), metric.to_string()]);
    }
    argv
}

fn build_traffic_limit_steps(
    base: &[String],
    interface_name: &str,
    limit: &RuntimeTunnelTrafficLimit,
) -> Result<Vec<RuntimeCommandSpec>> {
    if limit.is_default() {
        return Ok(Vec::new());
    }
    ensure_command_base(base, "runtime tc")?;
    let burst = limit.burst_kb.unwrap_or(32).to_string();
    let mut steps = Vec::new();
    if let Some(egress) = limit.egress_kbps {
        steps.push(RuntimeCommandSpec {
            label: "runtime_traffic_egress_limit",
            argv: extend_argv(
                base,
                [
                    "qdisc",
                    "replace",
                    "dev",
                    interface_name,
                    "root",
                    "tbf",
                    "rate",
                    &format!("{egress}kbit"),
                    "burst",
                    &format!("{burst}kb"),
                    "latency",
                    "50ms",
                ],
            ),
            mutates: true,
            required: true,
        });
    }
    if let Some(ingress) = limit.ingress_kbps {
        steps.push(RuntimeCommandSpec {
            label: "runtime_traffic_ingress_qdisc",
            argv: extend_argv(base, ["qdisc", "replace", "dev", interface_name, "ingress"]),
            mutates: true,
            required: true,
        });
        steps.push(RuntimeCommandSpec {
            label: "runtime_traffic_ingress_filter",
            argv: extend_argv(
                base,
                [
                    "filter",
                    "replace",
                    "dev",
                    interface_name,
                    "parent",
                    "ffff:",
                    "protocol",
                    "all",
                    "u32",
                    "match",
                    "u32",
                    "0",
                    "0",
                    "police",
                    "rate",
                    &format!("{ingress}kbit"),
                    "burst",
                    &format!("{burst}kb"),
                    "conform-exceed",
                    "drop",
                ],
            ),
            mutates: true,
            required: true,
        });
    }
    Ok(steps)
}

fn build_external_adapter_steps(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Result<Vec<RuntimeCommandSpec>> {
    let control = &plan.runtime_control;
    let mut steps = Vec::new();
    if let Some(command) = control.restart.as_ref().or(control.startup.as_ref()) {
        steps.push(RuntimeCommandSpec {
            label: if control.restart.is_some() {
                "runtime_adapter_restart"
            } else {
                "runtime_adapter_startup"
            },
            argv: render_runtime_adapter_command(command, plan, endpoint)?,
            mutates: true,
            required: true,
        });
    }
    if !control.traffic_limit.is_default() {
        if let Some(command) = &control.traffic_limit_apply {
            steps.push(RuntimeCommandSpec {
                label: "runtime_adapter_traffic_limit",
                argv: render_runtime_adapter_command(command, plan, endpoint)?,
                mutates: true,
                required: true,
            });
        }
    }
    if let Some(command) = &control.status {
        steps.push(RuntimeCommandSpec {
            label: "runtime_adapter_status",
            argv: render_runtime_adapter_command(command, plan, endpoint)?,
            mutates: false,
            required: false,
        });
    }
    Ok(steps)
}

fn build_external_adapter_remove_steps(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Result<Vec<RuntimeCommandSpec>> {
    let control = &plan.runtime_control;
    let mut steps = Vec::new();
    if let Some(command) = &control.stop {
        steps.push(RuntimeCommandSpec {
            label: "runtime_adapter_stop",
            argv: render_runtime_adapter_command(command, plan, endpoint)?,
            mutates: true,
            required: true,
        });
    }
    if let Some(command) = &control.cleanup {
        steps.push(RuntimeCommandSpec {
            label: "runtime_adapter_cleanup",
            argv: render_runtime_adapter_command(command, plan, endpoint)?,
            mutates: true,
            required: true,
        });
    }
    if let Some(command) = &control.status {
        steps.push(RuntimeCommandSpec {
            label: "runtime_adapter_status",
            argv: render_runtime_adapter_command(command, plan, endpoint)?,
            mutates: false,
            required: false,
        });
    }
    Ok(steps)
}

async fn run_runtime_compensation(
    config: &AgentConfig,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    link_exists_before: bool,
    triggered_by: &'static str,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    let (specs, unavailable_reason) =
        build_runtime_compensation_steps(config, plan, endpoint, link_exists_before)?;
    if specs.is_empty() {
        return Ok(serde_json::json!({
            "status": "not_available",
            "triggered_by": triggered_by,
            "reason": unavailable_reason.unwrap_or("no_compensation_steps"),
            "commands": [],
        }));
    }

    let mut reports = Vec::new();
    for spec in specs {
        reports.push(
            run_runtime_command_cancelable(
                spec.label,
                &spec.argv,
                spec.mutates,
                spec.required,
                config.network.runtime_command_timeout_secs,
                config.network.runtime_command_max_output_bytes as usize,
                cancel_token.clone(),
            )
            .await?,
        );
    }
    let all_steps_successful = reports
        .iter()
        .all(|report| report["success"].as_bool() == Some(true));
    Ok(serde_json::json!({
        "status": if all_steps_successful { "completed" } else { "attempted" },
        "triggered_by": triggered_by,
        "all_steps_successful": all_steps_successful,
        "commands": reports,
    }))
}

fn build_runtime_compensation_steps(
    config: &AgentConfig,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    link_exists_before: bool,
) -> Result<(Vec<RuntimeCommandSpec>, Option<&'static str>)> {
    match plan.runtime_control.manager {
        RuntimeTunnelManager::AgentIproute2Managed => {
            if link_exists_before {
                return Ok((Vec::new(), Some("existing_link_preserved")));
            }
            ensure_command_base(&config.network.runtime_ip_argv, "runtime ip")?;
            Ok((
                vec![RuntimeCommandSpec {
                    label: "runtime_compensate_link_delete",
                    argv: extend_argv(
                        &config.network.runtime_ip_argv,
                        ["link", "delete", "dev", &plan.interface_name],
                    ),
                    mutates: true,
                    required: false,
                }],
                None,
            ))
        }
        RuntimeTunnelManager::ExternalObserved => Ok((Vec::new(), Some("observed_only"))),
        RuntimeTunnelManager::ExternalManagedAdapter => {
            let mut specs = Vec::new();
            if let Some(command) = &plan.runtime_control.stop {
                specs.push(RuntimeCommandSpec {
                    label: "runtime_adapter_compensate_stop",
                    argv: render_runtime_adapter_command(command, plan, endpoint)?,
                    mutates: true,
                    required: false,
                });
            }
            if let Some(command) = &plan.runtime_control.cleanup {
                specs.push(RuntimeCommandSpec {
                    label: "runtime_adapter_compensate_cleanup",
                    argv: render_runtime_adapter_command(command, plan, endpoint)?,
                    mutates: true,
                    required: false,
                });
            }
            if specs.is_empty() {
                return Ok((Vec::new(), Some("adapter_remove_unavailable")));
            }
            Ok((specs, None))
        }
    }
}

pub(crate) fn render_runtime_adapter_command(
    command: &RuntimeTunnelCommand,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Result<Vec<String>> {
    ensure_command_base(&command.argv, "runtime adapter")?;
    Ok(command
        .argv
        .iter()
        .map(|part| {
            part.replace("{interface}", &plan.interface_name)
                .replace("{plan}", &plan.name)
                .replace("{kind}", runtime_kind_name(plan.kind))
                .replace("{local_client_id}", &endpoint.local_client_id)
                .replace("{peer_client_id}", &endpoint.peer_client_id)
                .replace("{local_underlay}", local_underlay(plan, endpoint))
                .replace("{remote_underlay}", remote_underlay(plan, endpoint))
                .replace("{local_address}", local_address(plan, endpoint))
                .replace("{remote_address}", remote_address(plan, endpoint))
                .replace("{prefix_len}", &endpoint.tunnel_prefix_len.to_string())
                .replace("{local_ipv4}", &family_address(plan, endpoint, true, true))
                .replace(
                    "{remote_ipv4}",
                    &family_address(plan, endpoint, true, false),
                )
                .replace("{prefix_len_ipv4}", &family_prefix_len(plan, true))
                .replace("{local_ipv6}", &family_address(plan, endpoint, false, true))
                .replace(
                    "{remote_ipv6}",
                    &family_address(plan, endpoint, false, false),
                )
                .replace("{prefix_len_ipv6}", &family_prefix_len(plan, false))
                .replace("{fou_port}", &plan.runtime_control.fou.port.to_string())
                .replace(
                    "{fou_peer_port}",
                    &plan.runtime_control.fou.peer_port.to_string(),
                )
                .replace(
                    "{fou_ipproto}",
                    &plan.runtime_control.fou.ipproto.to_string(),
                )
                .replace(
                    "{egress_kbps}",
                    &plan
                        .runtime_control
                        .traffic_limit
                        .egress_kbps
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                )
                .replace(
                    "{ingress_kbps}",
                    &plan
                        .runtime_control
                        .traffic_limit
                        .ingress_kbps
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                )
        })
        .collect())
}

async fn runtime_link_exists(root: &Path, interface_name: &str) -> bool {
    tokio::fs::metadata(root.join("sys/class/net").join(interface_name))
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

fn ensure_command_base(argv: &[String], label: &str) -> Result<()> {
    if argv.is_empty() {
        anyhow::bail!("{label} argv is empty");
    }
    if !argv[0].starts_with('/') {
        anyhow::bail!("{label} executable must be absolute");
    }
    Ok(())
}

fn extend_argv<'a>(base: &[String], parts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    base.iter()
        .cloned()
        .chain(parts.into_iter().map(str::to_string))
        .collect()
}

fn local_underlay<'a>(plan: &'a TunnelPlan, endpoint: &TunnelEndpointConfig) -> &'a str {
    if endpoint.side == TunnelEndpointSide::Left {
        &plan.left_underlay
    } else {
        &plan.right_underlay
    }
}

fn remote_underlay<'a>(plan: &'a TunnelPlan, endpoint: &TunnelEndpointConfig) -> &'a str {
    if endpoint.side == TunnelEndpointSide::Left {
        &plan.right_underlay
    } else {
        &plan.left_underlay
    }
}

fn local_address<'a>(_plan: &TunnelPlan, endpoint: &'a TunnelEndpointConfig) -> &'a str {
    &endpoint.local_tunnel_address
}

fn remote_address<'a>(_plan: &TunnelPlan, endpoint: &'a TunnelEndpointConfig) -> &'a str {
    &endpoint.remote_tunnel_address
}

fn family_address(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    ipv4: bool,
    local: bool,
) -> String {
    let pair = if ipv4 {
        plan.ipv4_tunnel.as_ref()
    } else {
        plan.ipv6_tunnel.as_ref()
    };
    let Some(pair) = pair else {
        return String::new();
    };
    match (endpoint.side, local) {
        (TunnelEndpointSide::Left, true) | (TunnelEndpointSide::Right, false) => pair.left.clone(),
        (TunnelEndpointSide::Left, false) | (TunnelEndpointSide::Right, true) => pair.right.clone(),
    }
}

fn family_prefix_len(plan: &TunnelPlan, ipv4: bool) -> String {
    if ipv4 {
        plan.ipv4_tunnel
            .as_ref()
            .map(|pair| pair.prefix_len.to_string())
            .unwrap_or_default()
    } else {
        plan.ipv6_tunnel
            .as_ref()
            .map(|pair| pair.prefix_len.to_string())
            .unwrap_or_default()
    }
}

fn linux_tunnel_mode(kind: TunnelKind) -> Result<&'static str> {
    match kind {
        TunnelKind::Gre => Ok("gre"),
        TunnelKind::Ipip | TunnelKind::Fou => Ok("ipip"),
        TunnelKind::Sit => Ok("sit"),
        TunnelKind::Openvpn | TunnelKind::Wireguard | TunnelKind::TunTap | TunnelKind::Custom => {
            anyhow::bail!("tunnel kind is not supported by agent iproute2 runtime")
        }
    }
}

fn runtime_kind_name(kind: TunnelKind) -> &'static str {
    match kind {
        TunnelKind::Gre => "gre",
        TunnelKind::Ipip => "ipip",
        TunnelKind::Sit => "sit",
        TunnelKind::Fou => "fou",
        TunnelKind::Openvpn => "openvpn",
        TunnelKind::Wireguard => "wireguard",
        TunnelKind::TunTap => "tun_tap",
        TunnelKind::Custom => "custom",
    }
}

fn side_name(side: TunnelEndpointSide) -> &'static str {
    match side {
        TunnelEndpointSide::Left => "left",
        TunnelEndpointSide::Right => "right",
    }
}

impl NetworkRuntimeReconcileInput<'_> {
    fn effective_uid_override(&self) -> Option<u32> {
        #[cfg(test)]
        {
            self.effective_uid_override
        }
        #[cfg(not(test))]
        {
            None
        }
    }
}

impl NetworkRuntimeRemoveInput<'_> {
    fn effective_uid_override(&self) -> Option<u32> {
        #[cfg(test)]
        {
            self.effective_uid_override
        }
        #[cfg(not(test))]
        {
            None
        }
    }
}

fn effective_uid(override_uid: Option<u32>) -> u32 {
    #[cfg(test)]
    if let Some(value) = override_uid {
        return value;
    }
    #[cfg(not(test))]
    let _ = override_uid;
    unsafe { libc::geteuid() as u32 }
}

#[cfg(test)]
mod tests;
