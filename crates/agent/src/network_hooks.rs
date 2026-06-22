use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::process::Command;
use vpsman_common::{AgentNetworkConfig, AgentNetworkPreset, TunnelEndpointConfig, TunnelPlan};

use crate::{
    child_process::{run_child_with_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult},
    command_worker::{CommandCancelToken, CommandCanceled},
};

pub(crate) struct NetworkHookContext<'a> {
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) endpoint: &'a TunnelEndpointConfig,
}

pub(crate) struct NetworkHookSpec {
    label: String,
    argv: Vec<String>,
}

const PRESET_IFUPDOWN2_IFDOWN: &str = "/usr/sbin/ifdown";
const PRESET_IFUPDOWN2_IFRELOAD: &str = "/usr/sbin/ifreload";
const PRESET_IFUPDOWN_IFDOWN: &str = "/sbin/ifdown";
const PRESET_IFUPDOWN_IFUP: &str = "/sbin/ifup";
const PRESET_BIRD2: &str = "/usr/sbin/bird";
const PRESET_BIRD2_CONFIG: &str = "/etc/bird/bird.conf";
const PRESET_BIRD2_CLIENT: &str = "/usr/sbin/birdc";
const PRESET_NETPLAN: &str = "/usr/sbin/netplan";
const PRESET_SYSTEMD_ANALYZE: &str = "/usr/bin/systemd-analyze";
const PRESET_SYSTEMD_NETWORKD_NETDEV: &str = "/etc/systemd/network/90-vpsman-tunnels.netdev";
const PRESET_SYSTEMD_NETWORKD_NETWORK: &str = "/etc/systemd/network/90-vpsman-tunnels.network";
const PRESET_NETWORKCTL: &str = "/usr/bin/networkctl";

pub(crate) fn validation_hook_specs(
    config: &AgentNetworkConfig,
    context: NetworkHookContext<'_>,
) -> Vec<NetworkHookSpec> {
    if !config.validate_enabled {
        return Vec::new();
    }
    let mut specs = Vec::new();
    if !config.ifupdown_validate_argv.is_empty() {
        specs.push(NetworkHookSpec::new(
            "ifupdown_validate",
            render_hook_argv(
                &config.ifupdown_validate_argv,
                context.plan,
                context.endpoint,
            ),
        ));
    }
    if !config.bird2_validate_argv.is_empty() {
        specs.push(NetworkHookSpec::new(
            "bird2_validate",
            render_hook_argv(&config.bird2_validate_argv, context.plan, context.endpoint),
        ));
    }
    if let Some(preset) = config.preset {
        specs.extend(preset_validation_specs(preset, context));
    }
    specs
}

pub(crate) fn reload_hook_specs(
    config: &AgentNetworkConfig,
    context: NetworkHookContext<'_>,
) -> Vec<NetworkHookSpec> {
    if !config.reload_enabled {
        return Vec::new();
    }
    let mut specs = Vec::new();
    for (index, argv) in config.reload_argv.iter().enumerate() {
        specs.push(NetworkHookSpec::new(
            format!("reload_{index}"),
            render_hook_argv(argv, context.plan, context.endpoint),
        ));
    }
    if let Some(preset) = config.preset {
        specs.extend(preset_reload_specs(preset, context));
    }
    specs
}

pub(crate) fn bird2_validation_hook_specs(
    config: &AgentNetworkConfig,
    context: NetworkHookContext<'_>,
) -> Vec<NetworkHookSpec> {
    if !config.validate_enabled {
        return Vec::new();
    }
    let mut specs = Vec::new();
    if !config.bird2_validate_argv.is_empty() {
        specs.push(NetworkHookSpec::new(
            "bird2_validate",
            render_hook_argv(&config.bird2_validate_argv, context.plan, context.endpoint),
        ));
    }
    if let Some(preset) = config.preset {
        specs.extend(preset_bird2_validation_specs(preset));
    }
    specs
}

pub(crate) fn bird2_reload_hook_specs(
    config: &AgentNetworkConfig,
    context: NetworkHookContext<'_>,
) -> Vec<NetworkHookSpec> {
    if !config.reload_enabled {
        return Vec::new();
    }
    let mut specs = Vec::new();
    for (index, argv) in config.bird2_reload_argv.iter().enumerate() {
        specs.push(NetworkHookSpec::new(
            format!("bird2_reload_{index}"),
            render_hook_argv(argv, context.plan, context.endpoint),
        ));
    }
    if let Some(preset) = config.preset {
        specs.extend(preset_bird2_reload_specs(preset));
    }
    specs
}

pub(crate) fn pre_rollback_hook_specs(
    config: &AgentNetworkConfig,
    context: NetworkHookContext<'_>,
) -> Vec<NetworkHookSpec> {
    if !config.reload_enabled {
        return Vec::new();
    }
    let mut specs = Vec::new();
    if let Some(preset) = config.preset {
        specs.extend(preset_pre_rollback_specs(preset, context));
    }
    specs
}

pub(crate) async fn run_network_hooks(
    specs: &[NetworkHookSpec],
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<Vec<serde_json::Value>> {
    let mut reports = Vec::new();
    for spec in specs {
        reports.push(run_hook(spec, max_timeout_secs, cancel_token.clone()).await?);
    }
    Ok(reports)
}

fn preset_pre_rollback_specs(
    preset: AgentNetworkPreset,
    context: NetworkHookContext<'_>,
) -> Vec<NetworkHookSpec> {
    match preset {
        AgentNetworkPreset::DebianIfupdown2Bird2 => vec![NetworkHookSpec::new(
            "preset_debian_ifupdown2_bird2_ifupdown_pre_rollback",
            vec![
                PRESET_IFUPDOWN2_IFDOWN.to_string(),
                "-f".to_string(),
                context.plan.interface_name.clone(),
            ],
        )],
        AgentNetworkPreset::DebianIfupdownBird2 => vec![NetworkHookSpec::new(
            "preset_debian_ifupdown_bird2_ifupdown_pre_rollback",
            vec![
                PRESET_IFUPDOWN_IFDOWN.to_string(),
                "-f".to_string(),
                context.plan.interface_name.clone(),
            ],
        )],
        AgentNetworkPreset::DebianNetplanBird2 | AgentNetworkPreset::DebianSystemdNetworkdBird2 => {
            Vec::new()
        }
    }
}

fn preset_validation_specs(
    preset: AgentNetworkPreset,
    context: NetworkHookContext<'_>,
) -> Vec<NetworkHookSpec> {
    match preset {
        AgentNetworkPreset::DebianIfupdown2Bird2 => vec![
            NetworkHookSpec::new(
                "preset_debian_ifupdown2_bird2_ifupdown_syntax_validate",
                vec![
                    PRESET_IFUPDOWN2_IFRELOAD.to_string(),
                    "-a".to_string(),
                    "-s".to_string(),
                ],
            ),
            NetworkHookSpec::new(
                "preset_debian_ifupdown2_bird2_bird2_parse_validate",
                vec![
                    PRESET_BIRD2.to_string(),
                    "-p".to_string(),
                    "-c".to_string(),
                    PRESET_BIRD2_CONFIG.to_string(),
                ],
            ),
        ],
        AgentNetworkPreset::DebianIfupdownBird2 => vec![
            NetworkHookSpec::new(
                "preset_debian_ifupdown_bird2_ifupdown_no_act_validate",
                vec![
                    PRESET_IFUPDOWN_IFUP.to_string(),
                    "--no-act".to_string(),
                    context.plan.interface_name.clone(),
                ],
            ),
            NetworkHookSpec::new(
                "preset_debian_ifupdown_bird2_bird2_parse_validate",
                vec![
                    PRESET_BIRD2.to_string(),
                    "-p".to_string(),
                    "-c".to_string(),
                    PRESET_BIRD2_CONFIG.to_string(),
                ],
            ),
        ],
        AgentNetworkPreset::DebianNetplanBird2 => vec![
            NetworkHookSpec::new(
                "preset_debian_netplan_bird2_netplan_generate_validate",
                vec![PRESET_NETPLAN.to_string(), "generate".to_string()],
            ),
            NetworkHookSpec::new(
                "preset_debian_netplan_bird2_bird2_parse_validate",
                bird2_parse_argv(),
            ),
        ],
        AgentNetworkPreset::DebianSystemdNetworkdBird2 => vec![
            NetworkHookSpec::new(
                "preset_debian_systemd_networkd_bird2_systemd_analyze_verify",
                vec![
                    PRESET_SYSTEMD_ANALYZE.to_string(),
                    "verify".to_string(),
                    PRESET_SYSTEMD_NETWORKD_NETDEV.to_string(),
                    PRESET_SYSTEMD_NETWORKD_NETWORK.to_string(),
                ],
            ),
            NetworkHookSpec::new(
                "preset_debian_systemd_networkd_bird2_bird2_parse_validate",
                bird2_parse_argv(),
            ),
        ],
    }
}

fn preset_bird2_validation_specs(preset: AgentNetworkPreset) -> Vec<NetworkHookSpec> {
    match preset {
        AgentNetworkPreset::DebianIfupdown2Bird2 => vec![NetworkHookSpec::new(
            "preset_debian_ifupdown2_bird2_bird2_parse_validate",
            bird2_parse_argv(),
        )],
        AgentNetworkPreset::DebianIfupdownBird2 => vec![NetworkHookSpec::new(
            "preset_debian_ifupdown_bird2_bird2_parse_validate",
            bird2_parse_argv(),
        )],
        AgentNetworkPreset::DebianNetplanBird2 => vec![NetworkHookSpec::new(
            "preset_debian_netplan_bird2_bird2_parse_validate",
            bird2_parse_argv(),
        )],
        AgentNetworkPreset::DebianSystemdNetworkdBird2 => vec![NetworkHookSpec::new(
            "preset_debian_systemd_networkd_bird2_bird2_parse_validate",
            bird2_parse_argv(),
        )],
    }
}

fn preset_reload_specs(
    preset: AgentNetworkPreset,
    context: NetworkHookContext<'_>,
) -> Vec<NetworkHookSpec> {
    match preset {
        AgentNetworkPreset::DebianIfupdown2Bird2 => vec![
            NetworkHookSpec::new(
                "preset_debian_ifupdown2_bird2_ifupdown_reload",
                vec![PRESET_IFUPDOWN2_IFRELOAD.to_string(), "-a".to_string()],
            ),
            NetworkHookSpec::new(
                "preset_debian_ifupdown2_bird2_bird2_reload",
                vec![PRESET_BIRD2_CLIENT.to_string(), "configure".to_string()],
            ),
        ],
        AgentNetworkPreset::DebianIfupdownBird2 => vec![
            NetworkHookSpec::new(
                "preset_debian_ifupdown_bird2_ifdown_before_ifup",
                vec![
                    PRESET_IFUPDOWN_IFDOWN.to_string(),
                    "-f".to_string(),
                    context.plan.interface_name.clone(),
                ],
            ),
            NetworkHookSpec::new(
                "preset_debian_ifupdown_bird2_ifup",
                vec![
                    PRESET_IFUPDOWN_IFUP.to_string(),
                    context.plan.interface_name.clone(),
                ],
            ),
            NetworkHookSpec::new(
                "preset_debian_ifupdown_bird2_bird2_reload",
                vec![PRESET_BIRD2_CLIENT.to_string(), "configure".to_string()],
            ),
        ],
        AgentNetworkPreset::DebianNetplanBird2 => vec![
            NetworkHookSpec::new(
                "preset_debian_netplan_bird2_netplan_apply",
                vec![PRESET_NETPLAN.to_string(), "apply".to_string()],
            ),
            NetworkHookSpec::new(
                "preset_debian_netplan_bird2_bird2_reload",
                vec![PRESET_BIRD2_CLIENT.to_string(), "configure".to_string()],
            ),
        ],
        AgentNetworkPreset::DebianSystemdNetworkdBird2 => vec![
            NetworkHookSpec::new(
                "preset_debian_systemd_networkd_bird2_networkd_reload",
                vec![PRESET_NETWORKCTL.to_string(), "reload".to_string()],
            ),
            NetworkHookSpec::new(
                "preset_debian_systemd_networkd_bird2_networkd_reconfigure",
                vec![
                    PRESET_NETWORKCTL.to_string(),
                    "reconfigure".to_string(),
                    context.plan.interface_name.clone(),
                ],
            ),
            NetworkHookSpec::new(
                "preset_debian_systemd_networkd_bird2_bird2_reload",
                vec![PRESET_BIRD2_CLIENT.to_string(), "configure".to_string()],
            ),
        ],
    }
}

fn preset_bird2_reload_specs(preset: AgentNetworkPreset) -> Vec<NetworkHookSpec> {
    match preset {
        AgentNetworkPreset::DebianIfupdown2Bird2 => vec![NetworkHookSpec::new(
            "preset_debian_ifupdown2_bird2_bird2_reload",
            vec![PRESET_BIRD2_CLIENT.to_string(), "configure".to_string()],
        )],
        AgentNetworkPreset::DebianIfupdownBird2 => vec![NetworkHookSpec::new(
            "preset_debian_ifupdown_bird2_bird2_reload",
            vec![PRESET_BIRD2_CLIENT.to_string(), "configure".to_string()],
        )],
        AgentNetworkPreset::DebianNetplanBird2 => vec![NetworkHookSpec::new(
            "preset_debian_netplan_bird2_bird2_reload",
            vec![PRESET_BIRD2_CLIENT.to_string(), "configure".to_string()],
        )],
        AgentNetworkPreset::DebianSystemdNetworkdBird2 => vec![NetworkHookSpec::new(
            "preset_debian_systemd_networkd_bird2_bird2_reload",
            vec![PRESET_BIRD2_CLIENT.to_string(), "configure".to_string()],
        )],
    }
}

fn bird2_parse_argv() -> Vec<String> {
    vec![
        PRESET_BIRD2.to_string(),
        "-p".to_string(),
        "-c".to_string(),
        PRESET_BIRD2_CONFIG.to_string(),
    ]
}

fn render_hook_argv(
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

impl NetworkHookSpec {
    fn new(label: impl Into<String>, argv: Vec<String>) -> Self {
        Self {
            label: label.into(),
            argv,
        }
    }
}

async fn run_hook(
    spec: &NetworkHookSpec,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<serde_json::Value> {
    if spec.argv.is_empty() {
        anyhow::bail!("network hook {} argv is empty", spec.label);
    }
    let mut command = Command::new(&spec.argv[0]);
    command.args(&spec.argv[1..]);
    command.kill_on_drop(true);
    command.stdin(Stdio::null());
    let result = run_child_with_bounded_output_cancelable(
        command,
        max_timeout_secs.clamp(1, 120),
        0,
        ChildCleanupPolicy::ProcessGroup,
        cancel_token,
    )
    .await
    .with_context(|| format!("failed to run network hook {}", spec.label))?;
    let exit_code = match result {
        ChildRunResult::Completed(output) => output.exit_code,
        ChildRunResult::TimedOut(cleanup) => {
            anyhow::bail!(
                "network hook {} timed out; cleanup={}",
                spec.label,
                serde_json::to_string(&cleanup).unwrap_or_else(|_| "<unavailable>".to_string())
            );
        }
        ChildRunResult::Canceled { reason, .. } => {
            return Err(CommandCanceled::new("network_hook", reason).into());
        }
    };
    if exit_code != Some(0) {
        anyhow::bail!(
            "network hook {} failed with exit code {:?}",
            spec.label,
            exit_code
        );
    }
    Ok(serde_json::json!({
        "label": spec.label,
        "argv": spec.argv,
        "exit_code": exit_code,
    }))
}

#[cfg(test)]
mod tests;
