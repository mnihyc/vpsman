use std::{
    net::IpAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use semver::Version;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use vpsman_common::{
    ensure_private_dir_tree_async, write_private_file_atomically_async, AgentConfig,
    RuntimeTunnelOpenvpnTransport, TunnelEndpointBuiltinCredentials, TunnelEndpointConfig,
    TunnelEndpointSide, TunnelPlan,
};

use crate::{command_worker::CommandCancelToken, state_dir::agent_state_dir};

use super::{
    build_address_replace_steps, build_route_replace_steps, build_traffic_limit_steps,
    ensure_command_base, extend_argv, run_runtime_command_cancelable, runtime_link_exists,
    RuntimeCommandSpec,
};

pub(super) struct PreparedOpenvpnState {
    pub(super) endpoint_dir: PathBuf,
    pub(super) config_path: PathBuf,
    pub(super) pid_path: PathBuf,
    pub(super) config_hash: String,
}

pub(super) async fn prepare_openvpn_state(
    plan_id: Option<&str>,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    credentials: Option<&TunnelEndpointBuiltinCredentials>,
    openvpn_version: &Version,
) -> Result<PreparedOpenvpnState> {
    let plan_id = parse_plan_id(plan_id)?;
    let TunnelEndpointBuiltinCredentials::Openvpn {
        local_private_key_pem,
        local_certificate_pem,
        peer_issuer_certificate_pem,
        peer_certificate_sha256_fingerprint,
        ..
    } = credentials.context("OpenVPN endpoint credentials are required")?
    else {
        anyhow::bail!("OpenVPN endpoint credentials have the wrong kind");
    };
    let root = agent_state_dir()?.join("network-tunnels");
    let endpoint_dir = endpoint_state_dir(&root, plan_id, endpoint.side);
    ensure_private_dir_tree_async(&root, &endpoint_dir)
        .await
        .context("create private OpenVPN state directory")?;
    let key_path = endpoint_dir.join("openvpn.key");
    let certificate_path = endpoint_dir.join("openvpn.crt");
    let peer_ca_path = endpoint_dir.join("openvpn-peer-ca.crt");
    let config_path = endpoint_dir.join("openvpn.conf");
    let pid_path = endpoint_dir.join("openvpn.pid");
    let status_path = endpoint_dir.join("openvpn.status");
    write_private_file_atomically_async(&key_path, local_private_key_pem.as_bytes())
        .await
        .context("write OpenVPN private key")?;
    write_private_file_atomically_async(&certificate_path, local_certificate_pem.as_bytes())
        .await
        .context("write OpenVPN certificate")?;
    write_private_file_atomically_async(&peer_ca_path, peer_issuer_certificate_pem.as_bytes())
        .await
        .context("write OpenVPN peer issuer certificate")?;
    let config = render_openvpn_config(
        plan,
        endpoint,
        &key_path,
        &certificate_path,
        &peer_ca_path,
        &pid_path,
        &status_path,
        openvpn_version,
    )?;
    let config_hash = openvpn_applied_hash(
        config.as_bytes(),
        local_private_key_pem.as_bytes(),
        local_certificate_pem.as_bytes(),
        peer_issuer_certificate_pem.as_bytes(),
        peer_certificate_sha256_fingerprint.as_bytes(),
    );
    write_private_file_atomically_async(&config_path, config.as_bytes())
        .await
        .context("write OpenVPN configuration")?;
    Ok(PreparedOpenvpnState {
        endpoint_dir,
        config_path,
        pid_path,
        config_hash,
    })
}

fn openvpn_applied_hash(
    config: &[u8],
    private_key: &[u8],
    certificate: &[u8],
    peer_issuer_certificate: &[u8],
    peer_fingerprint: &[u8],
) -> String {
    let mut digest = Sha256::new();
    for value in [
        config,
        private_key,
        certificate,
        peer_issuer_certificate,
        peer_fingerprint,
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    hex::encode(digest.finalize())
}

pub(super) async fn inspect_openvpn_prerequisites(
    config: &AgentConfig,
    cancel_token: CommandCancelToken,
) -> Result<(Vec<serde_json::Value>, Version)> {
    ensure_command_base(&config.network.runtime_openvpn_argv, "runtime openvpn")?;
    let argv = extend_argv(&config.network.runtime_openvpn_argv, ["--version"]);
    let report = run_runtime_command_cancelable(
        "runtime_openvpn_version",
        &argv,
        false,
        true,
        config.network.runtime_command_timeout_secs,
        config.network.runtime_command_max_output_bytes as usize,
        cancel_token,
    )
    .await?;
    if report["success"].as_bool() != Some(true) {
        let reason = if report["timed_out"].as_bool() == Some(true) {
            "OpenVPN version probe timed out"
        } else if report["killed_for_output_limit"].as_bool() == Some(true) {
            "OpenVPN version probe exceeded its output limit"
        } else {
            "OpenVPN version probe failed"
        };
        anyhow::bail!(reason);
    }
    let output = report["stdout"]["text"].as_str().unwrap_or_default();
    let version = parse_openvpn_version(output)
        .context("OpenVPN version output did not contain a semantic version")?;
    let identified_openvpn = output.lines().any(|line| line.starts_with("OpenVPN "));
    if !identified_openvpn {
        anyhow::bail!("OpenVPN version output did not identify OpenVPN");
    }
    if version < Version::new(2, 4, 0) {
        anyhow::bail!("OpenVPN 2.4 or newer is required; found {version}");
    }
    Ok((vec![report], version))
}

pub(super) async fn reconcile_existing_openvpn(
    config: &AgentConfig,
    plan: &TunnelPlan,
    prepared: &PreparedOpenvpnState,
) -> Result<(bool, serde_json::Value)> {
    let pid = read_owned_pid(config, prepared)
        .await?
        .context("OpenVPN interface exists without an owned plan process")?;
    let applied_hash = tokio::fs::read_to_string(prepared.endpoint_dir.join("applied.sha256"))
        .await
        .unwrap_or_default();
    if applied_hash.trim() == prepared.config_hash {
        return Ok((
            true,
            serde_json::json!({
                "status": "matched",
                "interface": plan.interface_name,
                "driver": "openvpn",
                "pid": pid,
                "config_hash_matches": true,
            }),
        ));
    }
    stop_pid(pid).await?;
    wait_for_process_exit(pid).await?;
    let root = Path::new(&config.network.root_dir);
    wait_for_link_state(
        root,
        &plan.interface_name,
        false,
        CommandCancelToken::default(),
    )
    .await?;
    Ok((
        false,
        serde_json::json!({
            "status": "restarted_for_configuration_change",
            "interface": plan.interface_name,
            "driver": "openvpn",
            "pid": pid,
            "config_hash_matches": false,
        }),
    ))
}

pub(super) async fn ensure_openvpn_start_is_safe(
    config: &AgentConfig,
    plan: &TunnelPlan,
    prepared: &PreparedOpenvpnState,
) -> Result<Option<serde_json::Value>> {
    if let Some(pid) = read_owned_pid(config, prepared).await? {
        stop_pid(pid).await?;
        wait_for_process_exit(pid).await?;
        return Ok(Some(serde_json::json!({
            "status": "recovered_owned_process_without_interface",
            "interface": plan.interface_name,
            "driver": "openvpn",
            "pid": pid,
        })));
    }
    Ok(None)
}

pub(super) fn build_openvpn_reconcile_steps(
    config: &AgentConfig,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    prepared: &PreparedOpenvpnState,
    link_exists: bool,
) -> Result<Vec<RuntimeCommandSpec>> {
    ensure_command_base(&config.network.runtime_openvpn_argv, "runtime openvpn")?;
    ensure_command_base(&config.network.runtime_ip_argv, "runtime ip")?;
    let mut steps = Vec::new();
    if !link_exists {
        steps.push(RuntimeCommandSpec {
            label: "runtime_openvpn_start",
            argv: extend_argv(
                &config.network.runtime_openvpn_argv,
                [
                    "--config",
                    prepared
                        .config_path
                        .to_str()
                        .context("OpenVPN config path is not UTF-8")?,
                ],
            ),
            mutates: true,
            required: true,
        });
    }
    let local_mtu = endpoint
        .local_mtu
        .context("Agent builtin OpenVPN endpoint MTU is required")?
        .to_string();
    steps.push(RuntimeCommandSpec {
        label: "runtime_link_mtu",
        argv: extend_argv(
            &config.network.runtime_ip_argv,
            [
                "link",
                "set",
                "dev",
                &plan.interface_name,
                "mtu",
                &local_mtu,
            ],
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

pub(super) async fn mark_openvpn_started(
    config: &AgentConfig,
    plan: &TunnelPlan,
    prepared: &PreparedOpenvpnState,
    cancel_token: CommandCancelToken,
) -> Result<()> {
    wait_for_link_state(
        Path::new(&config.network.root_dir),
        &plan.interface_name,
        true,
        cancel_token,
    )
    .await?;
    let _ = read_owned_pid(config, prepared)
        .await?
        .context("OpenVPN start did not create an owned process")?;
    write_private_file_atomically_async(
        &prepared.endpoint_dir.join("applied.sha256"),
        format!("{}\n", prepared.config_hash).as_bytes(),
    )
    .await
    .context("record applied OpenVPN configuration")?;
    Ok(())
}

pub(super) async fn stop_openvpn_for_remove(
    config: &AgentConfig,
    plan_id: Option<&str>,
    side: TunnelEndpointSide,
    require_owned_process: bool,
) -> Result<bool> {
    let plan_id = parse_plan_id(plan_id)?;
    let root = agent_state_dir()?.join("network-tunnels");
    let endpoint_dir = endpoint_state_dir(&root, plan_id, side);
    let prepared = PreparedOpenvpnState {
        config_path: endpoint_dir.join("openvpn.conf"),
        pid_path: endpoint_dir.join("openvpn.pid"),
        endpoint_dir,
        config_hash: String::new(),
    };
    match read_owned_pid(config, &prepared).await? {
        Some(pid) => {
            stop_pid(pid).await?;
            wait_for_process_exit(pid).await?;
            Ok(true)
        }
        None if require_owned_process => {
            anyhow::bail!("OpenVPN interface exists without an owned plan process")
        }
        None => Ok(false),
    }
}

pub(super) async fn cleanup_openvpn_state(
    plan_id: Option<&str>,
    side: TunnelEndpointSide,
) -> Result<()> {
    let plan_id = parse_plan_id(plan_id)?;
    let root = agent_state_dir()?.join("network-tunnels");
    let endpoint_dir = endpoint_state_dir(&root, plan_id, side);
    for name in [
        "openvpn.key",
        "openvpn.crt",
        "openvpn-peer-ca.crt",
        "openvpn.conf",
        "openvpn.pid",
        "openvpn.status",
        "applied.sha256",
    ] {
        remove_file_if_present(endpoint_dir.join(name)).await?;
    }
    remove_empty_dir(&endpoint_dir).await?;
    remove_empty_dir(&root.join(plan_id.to_string())).await?;
    Ok(())
}

fn render_openvpn_config(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    key_path: &Path,
    certificate_path: &Path,
    peer_ca_path: &Path,
    pid_path: &Path,
    status_path: &Path,
    openvpn_version: &Version,
) -> Result<String> {
    let options = &plan.runtime_control.openvpn;
    let listener = endpoint.side == options.listener_side;
    let family_address = if listener {
        endpoint
            .local_underlay
            .as_deref()
            .unwrap_or(match endpoint.side {
                TunnelEndpointSide::Left => &plan.right_remote_underlay,
                TunnelEndpointSide::Right => &plan.left_remote_underlay,
            })
    } else {
        &endpoint.remote_underlay
    };
    let family = family_address
        .parse::<IpAddr>()
        .context("OpenVPN underlay address is invalid")?;
    let protocol = match (options.transport, listener, family) {
        (RuntimeTunnelOpenvpnTransport::Udp, _, IpAddr::V4(_)) => "udp4",
        (RuntimeTunnelOpenvpnTransport::Udp, _, IpAddr::V6(_)) => "udp6",
        (RuntimeTunnelOpenvpnTransport::Tcp, true, IpAddr::V4(_)) => "tcp4-server",
        (RuntimeTunnelOpenvpnTransport::Tcp, true, IpAddr::V6(_)) => "tcp6-server",
        (RuntimeTunnelOpenvpnTransport::Tcp, false, IpAddr::V4(_)) => "tcp4-client",
        (RuntimeTunnelOpenvpnTransport::Tcp, false, IpAddr::V6(_)) => "tcp6-client",
    };
    let mut lines = vec![
        "mode p2p".to_string(),
        format!("dev {}", plan.interface_name),
        "dev-type tun".to_string(),
        "topology p2p".to_string(),
        format!("proto {protocol}"),
        format!(
            "tun-mtu {}",
            endpoint
                .local_mtu
                .context("Agent builtin OpenVPN endpoint MTU is required")?
        ),
        format!("cert {}", config_path_value(certificate_path)?),
        format!("key {}", config_path_value(key_path)?),
        format!("ca {}", config_path_value(peer_ca_path)?),
        if listener {
            "tls-server".to_string()
        } else {
            "tls-client".to_string()
        },
        if listener {
            "remote-cert-tls client".to_string()
        } else {
            "remote-cert-tls server".to_string()
        },
        "tls-version-min 1.2".to_string(),
        "cipher AES-256-GCM".to_string(),
        "auth SHA256".to_string(),
        "persist-key".to_string(),
        "persist-tun".to_string(),
        "ping 10".to_string(),
        "ping-restart 60".to_string(),
        "daemon".to_string(),
        format!("writepid {}", config_path_value(pid_path)?),
        format!("status {} 10", config_path_value(status_path)?),
        "verb 3".to_string(),
    ];
    lines.push(if openvpn_version < &Version::new(2, 5, 0) {
        "ncp-ciphers AES-256-GCM:AES-128-GCM".to_string()
    } else {
        "data-ciphers AES-256-GCM:AES-128-GCM".to_string()
    });
    if listener {
        lines.push("dh none".to_string());
        lines.push(format!("lport {}", options.port));
        if let Some(local) = endpoint.local_underlay.as_deref() {
            lines.push(format!("local {local}"));
        }
    } else {
        lines.push(format!(
            "remote {} {}",
            endpoint.remote_underlay, options.port
        ));
        if let Some(local) = endpoint.local_underlay.as_deref() {
            lines.push(format!("local {local}"));
            lines.push("lport 0".to_string());
        } else {
            lines.push("nobind".to_string());
        }
    }
    for pair in [plan.ipv4_tunnel.as_ref(), plan.ipv6_tunnel.as_ref()]
        .into_iter()
        .flatten()
    {
        let (local, remote) = match endpoint.side {
            TunnelEndpointSide::Left => (&pair.left, &pair.right),
            TunnelEndpointSide::Right => (&pair.right, &pair.left),
        };
        if local.parse::<IpAddr>()?.is_ipv4() {
            lines.push(format!("ifconfig {local} {remote}"));
        } else {
            lines.push(format!(
                "ifconfig-ipv6 {local}/{} {remote}",
                pair.prefix_len
            ));
        }
    }
    Ok(format!("{}\n", lines.join("\n")))
}

fn parse_openvpn_version(output: &str) -> Option<Version> {
    output.lines().find_map(|line| {
        let mut tokens = line.split_whitespace();
        if tokens.next()? != "OpenVPN" {
            return None;
        }
        Version::parse(
            tokens
                .next()?
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-'),
        )
        .ok()
    })
}

async fn read_owned_pid(
    config: &AgentConfig,
    prepared: &PreparedOpenvpnState,
) -> Result<Option<u32>> {
    let raw = match tokio::fs::read_to_string(&prepared.pid_path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read OpenVPN PID file"),
    };
    let pid = raw
        .trim()
        .parse::<u32>()
        .context("OpenVPN PID file is invalid")?;
    let cmdline_path = Path::new(&config.network.root_dir)
        .join("proc")
        .join(pid.to_string())
        .join("cmdline");
    let cmdline = match tokio::fs::read(&cmdline_path).await {
        Ok(cmdline) => cmdline,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            remove_file_if_present(prepared.pid_path.clone()).await?;
            return Ok(None);
        }
        Err(error) => return Err(error).context("inspect OpenVPN process"),
    };
    let args = cmdline
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect::<Vec<_>>();
    let config_path = prepared
        .config_path
        .to_str()
        .context("OpenVPN config path is not UTF-8")?;
    let config_owned = args
        .windows(2)
        .any(|args| args[0] == "--config" && args[1] == config_path);
    if !config_owned {
        anyhow::bail!("OpenVPN PID does not own this plan configuration");
    }
    Ok(Some(pid))
}

async fn stop_pid(pid: u32) -> Result<()> {
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if result == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error).context("stop owned OpenVPN process")
        }
    }
}

async fn wait_for_process_exit(pid: u32) -> Result<()> {
    for _ in 0..50 {
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("owned OpenVPN process did not stop within 5 seconds")
}

#[cfg(test)]
#[path = "tests_network_runtime_openvpn.rs"]
mod tests;

pub(super) async fn wait_for_link_state(
    root: &Path,
    interface_name: &str,
    expected: bool,
    cancel_token: CommandCancelToken,
) -> Result<()> {
    for _ in 0..50 {
        cancel_token.check("openvpn_interface_wait")?;
        if runtime_link_exists(root, interface_name).await == expected {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!(
        "OpenVPN interface {interface_name} did not become {} within 5 seconds",
        if expected { "ready" } else { "absent" }
    )
}

fn parse_plan_id(plan_id: Option<&str>) -> Result<Uuid> {
    Uuid::parse_str(plan_id.context("runtime tunnel plan ID is required")?)
        .context("runtime tunnel plan ID is invalid")
}

fn endpoint_state_dir(root: &Path, plan_id: Uuid, side: TunnelEndpointSide) -> PathBuf {
    let side = match side {
        TunnelEndpointSide::Left => "left",
        TunnelEndpointSide::Right => "right",
    };
    root.join(plan_id.to_string()).join(side)
}

fn config_path_value(path: &Path) -> Result<String> {
    let value = path.to_str().context("OpenVPN state path is not UTF-8")?;
    if value.chars().any(char::is_control) {
        anyhow::bail!("OpenVPN state path contains control characters");
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

async fn remove_file_if_present(path: PathBuf) -> Result<()> {
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

async fn remove_empty_dir(path: &Path) -> Result<()> {
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
