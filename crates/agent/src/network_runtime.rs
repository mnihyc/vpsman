use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use tokio::time;
use vpsman_common::{
    render_tunnel_endpoint_config, AgentConfig, AgentRuntimeUnprivilegedMutationPolicy,
    RuntimeTunnelCommand, RuntimeTunnelManager, RuntimeTunnelRoute, RuntimeTunnelTrafficLimit,
    TunnelEndpointConfig, TunnelEndpointSide, TunnelKind, TunnelPlan,
};

mod command_runner;

use self::command_runner::run_runtime_command;

pub(crate) struct NetworkRuntimeReconcileInput<'a> {
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) timeout_secs: u64,
    #[cfg(test)]
    pub(crate) effective_uid_override: Option<u32>,
}

pub(crate) struct NetworkRuntimeRemoveInput<'a> {
    pub(crate) config: &'a AgentConfig,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) timeout_secs: u64,
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

pub(crate) async fn execute_runtime_tunnel_reconcile_report(
    input: NetworkRuntimeReconcileInput<'_>,
) -> Result<serde_json::Value> {
    time::timeout(
        Duration::from_secs(input.timeout_secs.max(1)),
        reconcile_runtime_tunnel(input),
    )
    .await
    .context("runtime tunnel reconcile timed out")?
}

pub(crate) async fn execute_runtime_tunnel_remove_report(
    input: NetworkRuntimeRemoveInput<'_>,
) -> Result<serde_json::Value> {
    time::timeout(
        Duration::from_secs(input.timeout_secs.max(1)),
        remove_runtime_tunnel(input),
    )
    .await
    .context("runtime tunnel remove timed out")?
}

async fn reconcile_runtime_tunnel(
    input: NetworkRuntimeReconcileInput<'_>,
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
            "reason": "network_apply_disabled",
            "plan": input.plan.name,
            "interface": input.plan.interface_name,
        }));
    }

    let root = Path::new(&input.config.network.root_dir);
    let link_exists = runtime_link_exists(root, &input.plan.interface_name).await;
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
    let mut reports = Vec::new();
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
        let report = run_runtime_command(
            spec.label,
            &spec.argv,
            spec.mutates,
            spec.required,
            input.config.network.runtime_command_timeout_secs,
            input.config.network.runtime_command_max_output_bytes as usize,
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
            )
            .await?,
        )
    } else {
        None
    };

    let status = if input.plan.runtime_control.manager == RuntimeTunnelManager::ExternalObserved {
        "observed_only"
    } else if degraded {
        "degraded_unprivileged"
    } else if failed {
        "failed"
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
        "effective_uid": effective_uid,
        "unprivileged_mutation_policy": unprivileged_mutation_policy,
        "commands": reports,
        "compensation": compensation,
    }))
}

async fn remove_runtime_tunnel(input: NetworkRuntimeRemoveInput<'_>) -> Result<serde_json::Value> {
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
            "reason": "network_apply_disabled",
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
        let report = run_runtime_command(
            spec.label,
            &spec.argv,
            spec.mutates,
            spec.required,
            input.config.network.runtime_command_timeout_secs,
            input.config.network.runtime_command_max_output_bytes as usize,
        )
        .await?;
        if spec.required && report["success"].as_bool() != Some(true) {
            failed = true;
        }
        reports.push(report);
    }

    let status = if input.plan.runtime_control.manager == RuntimeTunnelManager::ExternalObserved {
        "observed_only"
    } else if degraded {
        "degraded_unprivileged"
    } else if failed {
        "failed"
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

    let tunnel_action = if link_exists { "change" } else { "add" };
    let tunnel_label = if link_exists {
        "runtime_tunnel_change"
    } else {
        "runtime_tunnel_add"
    };
    steps.push(RuntimeCommandSpec {
        label: tunnel_label,
        argv: build_ip_tunnel_argv(
            &config.network.runtime_ip_argv,
            tunnel_action,
            plan,
            endpoint,
        )?,
        mutates: true,
        required: true,
    });
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
            run_runtime_command(
                spec.label,
                &spec.argv,
                spec.mutates,
                spec.required,
                config.network.runtime_command_timeout_secs,
                config.network.runtime_command_max_output_bytes as usize,
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
                .replace("{prefix_len}", &plan.tunnel_prefix_len.to_string())
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

fn local_address<'a>(plan: &'a TunnelPlan, endpoint: &TunnelEndpointConfig) -> &'a str {
    if endpoint.side == TunnelEndpointSide::Left {
        &plan.left_tunnel_address
    } else {
        &plan.right_tunnel_address
    }
}

fn remote_address<'a>(plan: &'a TunnelPlan, endpoint: &TunnelEndpointConfig) -> &'a str {
    if endpoint.side == TunnelEndpointSide::Left {
        &plan.right_tunnel_address
    } else {
        &plan.left_tunnel_address
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
