use std::{net::IpAddr, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vpsman_common::{
    ensure_private_dir_tree_async, write_private_file_atomically_async, AgentConfig,
    TunnelEndpointBuiltinCredentials, TunnelEndpointConfig, TunnelEndpointSide, TunnelPlan,
};

use crate::{command_worker::CommandCancelToken, state_dir::agent_state_dir};

use super::{
    build_address_replace_steps, build_route_replace_steps, build_traffic_limit_steps,
    ensure_command_base, extend_argv, run_runtime_command_cancelable, RuntimeCommandSpec,
};

pub(super) struct PreparedWireguardState {
    pub(super) private_key_path: PathBuf,
    applied_state_path: PathBuf,
    pending_state_path: PathBuf,
    previous_applied: Option<AppliedWireguardState>,
    pending: Option<AppliedWireguardState>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct AppliedWireguardState {
    local_public_key_base64: String,
    peer_public_key_base64: String,
    peer_endpoint_configured: bool,
}

pub(super) async fn prepare_wireguard_state(
    plan_id: Option<&str>,
    side: TunnelEndpointSide,
    credentials: Option<&TunnelEndpointBuiltinCredentials>,
) -> Result<PreparedWireguardState> {
    let plan_id = parse_plan_id(plan_id)?;
    let TunnelEndpointBuiltinCredentials::Wireguard {
        local_private_key_base64,
        ..
    } = credentials.context("WireGuard endpoint credentials are required")?
    else {
        anyhow::bail!("WireGuard endpoint credentials have the wrong kind");
    };
    let root = agent_state_dir()?.join("network-tunnels");
    let endpoint_dir = endpoint_state_dir(&root, plan_id, side);
    ensure_private_dir_tree_async(&root, &endpoint_dir)
        .await
        .context("create private WireGuard state directory")?;
    let prepared = load_wireguard_state_for(plan_id, side).await?;
    let private_key_path = prepared.private_key_path.clone();
    let mut key = local_private_key_base64.trim().as_bytes().to_vec();
    key.push(b'\n');
    write_private_file_atomically_async(&private_key_path, &key)
        .await
        .context("write private WireGuard key")?;
    Ok(prepared)
}

pub(super) async fn load_wireguard_state(
    plan_id: Option<&str>,
    side: TunnelEndpointSide,
) -> Result<PreparedWireguardState> {
    load_wireguard_state_for(parse_plan_id(plan_id)?, side).await
}

async fn load_wireguard_state_for(
    plan_id: Uuid,
    side: TunnelEndpointSide,
) -> Result<PreparedWireguardState> {
    let root = agent_state_dir()?.join("network-tunnels");
    let endpoint_dir = endpoint_state_dir(&root, plan_id, side);
    let applied_state_path = endpoint_dir.join("wireguard.applied.json");
    let pending_state_path = endpoint_dir.join("wireguard.pending.json");
    let previous_applied = read_wireguard_public_state(&applied_state_path).await?;
    let pending = read_wireguard_public_state(&pending_state_path).await?;
    Ok(PreparedWireguardState {
        private_key_path: endpoint_dir.join("wireguard.key"),
        applied_state_path,
        pending_state_path,
        previous_applied,
        pending,
    })
}

async fn read_wireguard_public_state(
    path: &std::path::Path,
) -> Result<Option<AppliedWireguardState>> {
    Ok(match tokio::fs::read_to_string(path).await {
        Ok(raw) => Some(serde_json::from_str(&raw).context("parse WireGuard ownership state")?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    })
}

pub(super) async fn validate_existing_wireguard(
    config: &AgentConfig,
    plan: &TunnelPlan,
    credentials: Option<&TunnelEndpointBuiltinCredentials>,
    prepared: &PreparedWireguardState,
    cancel_token: CommandCancelToken,
) -> Result<(Vec<serde_json::Value>, serde_json::Value, Vec<String>)> {
    let TunnelEndpointBuiltinCredentials::Wireguard {
        local_public_key_base64,
        peer_public_key_base64,
        ..
    } = credentials.context("WireGuard endpoint credentials are required")?
    else {
        anyhow::bail!("WireGuard endpoint credentials have the wrong kind");
    };
    ensure_command_base(&config.network.runtime_wg_argv, "runtime wg")?;
    let argv = extend_argv(
        &config.network.runtime_wg_argv,
        ["show", &plan.interface_name, "public-key"],
    );
    let report = run_runtime_command_cancelable(
        "runtime_wireguard_public_key_inspect",
        &argv,
        false,
        true,
        config.network.runtime_command_timeout_secs,
        config.network.runtime_command_max_output_bytes as usize,
        cancel_token.clone(),
    )
    .await?;
    if report["success"].as_bool() != Some(true) {
        anyhow::bail!(
            "existing interface {} is not an inspectable WireGuard interface",
            plan.interface_name
        );
    }
    let actual = report["stdout"]["text"].as_str().unwrap_or_default().trim();
    let owned_local = prepared
        .previous_applied
        .iter()
        .chain(prepared.pending.iter())
        .any(|state| state.local_public_key_base64 == actual);
    if actual != local_public_key_base64 && !owned_local {
        anyhow::bail!(
            "existing WireGuard interface {} is foreign: public key does not match the saved plan",
            plan.interface_name
        );
    }
    let peer_argv = extend_argv(
        &config.network.runtime_wg_argv,
        ["show", &plan.interface_name, "peers"],
    );
    let peer_report = run_runtime_command_cancelable(
        "runtime_wireguard_peer_inspect",
        &peer_argv,
        false,
        true,
        config.network.runtime_command_timeout_secs,
        config.network.runtime_command_max_output_bytes as usize,
        cancel_token,
    )
    .await?;
    if peer_report["success"].as_bool() != Some(true) {
        anyhow::bail!(
            "existing interface {} does not expose WireGuard peers",
            plan.interface_name
        );
    }
    let peers = peer_report["stdout"]["text"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if peers.iter().any(|peer| {
        peer != peer_public_key_base64
            && !prepared
                .previous_applied
                .iter()
                .chain(prepared.pending.iter())
                .any(|state| state.peer_public_key_base64 == *peer)
    }) {
        anyhow::bail!(
            "existing WireGuard interface {} contains a peer not owned by the saved plan",
            plan.interface_name
        );
    }
    Ok((
        vec![report, peer_report],
        serde_json::json!({
            "status": "matched",
            "interface": plan.interface_name,
            "driver": "wireguard",
            "public_key_matches_desired_or_previous": true,
            "peer_count": peers.len(),
        }),
        peers,
    ))
}

pub(super) fn build_wireguard_reconcile_steps(
    config: &AgentConfig,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    credentials: Option<&TunnelEndpointBuiltinCredentials>,
    prepared: &PreparedWireguardState,
    link_exists: bool,
    existing_peers: &[String],
) -> Result<Vec<RuntimeCommandSpec>> {
    let TunnelEndpointBuiltinCredentials::Wireguard {
        peer_public_key_base64,
        ..
    } = credentials.context("WireGuard endpoint credentials are required")?
    else {
        anyhow::bail!("WireGuard endpoint credentials have the wrong kind");
    };
    ensure_command_base(&config.network.runtime_ip_argv, "runtime ip")?;
    ensure_command_base(&config.network.runtime_wg_argv, "runtime wg")?;
    let mut steps = Vec::new();
    if !link_exists {
        steps.push(RuntimeCommandSpec {
            label: "runtime_wireguard_link_add",
            argv: extend_argv(
                &config.network.runtime_ip_argv,
                [
                    "link",
                    "add",
                    "dev",
                    &plan.interface_name,
                    "type",
                    "wireguard",
                ],
            ),
            mutates: true,
            required: true,
        });
    }
    let options = &plan.runtime_control.wireguard;
    steps.push(RuntimeCommandSpec {
        label: "runtime_wireguard_configure",
        argv: build_wireguard_configure_argv(
            config,
            plan,
            endpoint,
            prepared,
            peer_public_key_base64,
        )?,
        mutates: true,
        required: true,
    });
    steps.push(RuntimeCommandSpec {
        label: "runtime_wireguard_public_key_verify",
        argv: extend_argv(
            &config.network.runtime_wg_argv,
            ["show", &plan.interface_name, "public-key"],
        ),
        mutates: false,
        required: true,
    });
    for peer in existing_peers
        .iter()
        .filter(|peer| peer.as_str() != peer_public_key_base64)
    {
        steps.push(RuntimeCommandSpec {
            label: "runtime_wireguard_peer_remove",
            argv: extend_argv(
                &config.network.runtime_wg_argv,
                ["set", &plan.interface_name, "peer", peer, "remove"],
            ),
            mutates: true,
            required: true,
        });
    }
    let current_endpoint_configured = options.configures_peer_endpoint(endpoint.side);
    let clear_previous_endpoint = prepared
        .previous_applied
        .iter()
        .chain(prepared.pending.iter())
        .any(|state| {
            state.peer_public_key_base64.as_str() == peer_public_key_base64
                && state.peer_endpoint_configured
                && !current_endpoint_configured
        });
    if clear_previous_endpoint {
        steps.push(RuntimeCommandSpec {
            label: "runtime_wireguard_roaming_peer_reset",
            argv: extend_argv(
                &config.network.runtime_wg_argv,
                [
                    "set",
                    &plan.interface_name,
                    "peer",
                    peer_public_key_base64,
                    "remove",
                ],
            ),
            mutates: true,
            required: true,
        });
        steps.push(RuntimeCommandSpec {
            label: "runtime_wireguard_configure_roaming",
            argv: build_wireguard_configure_argv(
                config,
                plan,
                endpoint,
                prepared,
                peer_public_key_base64,
            )?,
            mutates: true,
            required: true,
        });
    }
    steps.push(RuntimeCommandSpec {
        label: "runtime_wireguard_peer_verify",
        argv: extend_argv(
            &config.network.runtime_wg_argv,
            ["show", &plan.interface_name, "peers"],
        ),
        mutates: false,
        required: true,
    });
    let mtu = endpoint
        .local_mtu
        .context("Agent builtin WireGuard endpoint MTU is required")?
        .to_string();
    steps.push(RuntimeCommandSpec {
        label: "runtime_link_mtu",
        argv: extend_argv(
            &config.network.runtime_ip_argv,
            ["link", "set", "dev", &plan.interface_name, "mtu", &mtu],
        ),
        mutates: true,
        required: true,
    });
    steps.extend(build_address_replace_steps(
        &config.network.runtime_ip_argv,
        plan,
        endpoint,
    )?);
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

fn build_wireguard_configure_argv(
    config: &AgentConfig,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    prepared: &PreparedWireguardState,
    peer_public_key_base64: &str,
) -> Result<Vec<String>> {
    let options = &plan.runtime_control.wireguard;
    let listen_port = options.listen_port(endpoint.side).to_string();
    let peer_port = options.peer_listen_port(endpoint.side);
    let keepalive = options.keepalive_secs(endpoint.side).to_string();
    let allowed_ips = allowed_ips(plan)?;
    let mut argv = extend_argv(
        &config.network.runtime_wg_argv,
        [
            "set",
            &plan.interface_name,
            "private-key",
            prepared
                .private_key_path
                .to_str()
                .context("WireGuard private key path is not UTF-8")?,
            "listen-port",
            &listen_port,
            "peer",
            peer_public_key_base64,
        ],
    );
    if options.configures_peer_endpoint(endpoint.side) {
        let peer_endpoint = format_peer_endpoint(&endpoint.remote_underlay, peer_port)?;
        argv.extend(["endpoint".to_string(), peer_endpoint]);
    }
    argv.extend([
        "persistent-keepalive".to_string(),
        keepalive,
        "allowed-ips".to_string(),
        allowed_ips,
    ]);
    Ok(argv)
}

pub(super) async fn mark_wireguard_applied(
    prepared: &PreparedWireguardState,
    credentials: Option<&TunnelEndpointBuiltinCredentials>,
    peer_endpoint_configured: bool,
) -> Result<()> {
    let TunnelEndpointBuiltinCredentials::Wireguard {
        local_public_key_base64,
        peer_public_key_base64,
        ..
    } = credentials.context("WireGuard endpoint credentials are required")?
    else {
        anyhow::bail!("WireGuard endpoint credentials have the wrong kind");
    };
    let state = AppliedWireguardState {
        local_public_key_base64: local_public_key_base64.clone(),
        peer_public_key_base64: peer_public_key_base64.clone(),
        peer_endpoint_configured,
    };
    let mut encoded = serde_json::to_vec(&state).context("encode WireGuard ownership state")?;
    encoded.push(b'\n');
    write_private_file_atomically_async(&prepared.applied_state_path, &encoded)
        .await
        .context("record applied WireGuard ownership state")?;
    remove_file_if_present(prepared.pending_state_path.clone()).await
}

pub(super) async fn mark_wireguard_pending(
    prepared: &PreparedWireguardState,
    credentials: Option<&TunnelEndpointBuiltinCredentials>,
    peer_endpoint_configured: bool,
) -> Result<()> {
    let TunnelEndpointBuiltinCredentials::Wireguard {
        local_public_key_base64,
        peer_public_key_base64,
        ..
    } = credentials.context("WireGuard endpoint credentials are required")?
    else {
        anyhow::bail!("WireGuard endpoint credentials have the wrong kind");
    };
    let state = AppliedWireguardState {
        local_public_key_base64: local_public_key_base64.clone(),
        peer_public_key_base64: peer_public_key_base64.clone(),
        peer_endpoint_configured,
    };
    let mut encoded =
        serde_json::to_vec(&state).context("encode pending WireGuard ownership state")?;
    encoded.push(b'\n');
    write_private_file_atomically_async(&prepared.pending_state_path, &encoded)
        .await
        .context("record pending WireGuard ownership transition")
}

pub(super) async fn cleanup_wireguard_state(
    plan_id: Option<&str>,
    side: TunnelEndpointSide,
) -> Result<()> {
    let plan_id = parse_plan_id(plan_id)?;
    let root = agent_state_dir()?.join("network-tunnels");
    let endpoint_dir = endpoint_state_dir(&root, plan_id, side);
    remove_file_if_present(endpoint_dir.join("wireguard.key")).await?;
    remove_file_if_present(endpoint_dir.join("wireguard.applied.json")).await?;
    remove_file_if_present(endpoint_dir.join("wireguard.pending.json")).await?;
    remove_empty_dir(&endpoint_dir).await?;
    remove_empty_dir(&root.join(plan_id.to_string())).await?;
    Ok(())
}

fn parse_plan_id(plan_id: Option<&str>) -> Result<Uuid> {
    Uuid::parse_str(plan_id.context("runtime tunnel plan ID is required")?)
        .context("runtime tunnel plan ID is invalid")
}

fn endpoint_state_dir(root: &std::path::Path, plan_id: Uuid, side: TunnelEndpointSide) -> PathBuf {
    let side = match side {
        TunnelEndpointSide::Left => "left",
        TunnelEndpointSide::Right => "right",
    };
    root.join(plan_id.to_string()).join(side)
}

fn format_peer_endpoint(address: &str, port: u16) -> Result<String> {
    let address = address
        .parse::<IpAddr>()
        .context("WireGuard remote underlay is invalid")?;
    Ok(match address {
        IpAddr::V4(address) => format!("{address}:{port}"),
        IpAddr::V6(address) => format!("[{address}]:{port}"),
    })
}

fn allowed_ips(plan: &TunnelPlan) -> Result<String> {
    let mut values = Vec::new();
    if plan.ipv4_tunnel.is_some() {
        values.push("0.0.0.0/0");
    }
    if plan.ipv6_tunnel.is_some() {
        values.push("::/0");
    }
    if values.is_empty() {
        anyhow::bail!("WireGuard requires at least one inner address family");
    }
    Ok(values.join(","))
}

async fn remove_file_if_present(path: PathBuf) -> Result<()> {
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

async fn remove_empty_dir(path: &std::path::Path) -> Result<()> {
    match tokio::fs::remove_dir(path).await {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

#[cfg(test)]
#[path = "tests_network_runtime_wireguard.rs"]
mod tests;
