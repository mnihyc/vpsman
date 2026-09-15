use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use vpsman_common::{
    ensure_private_dir_tree_async, write_private_file_atomically_async, AgentConfig,
    TunnelEndpointConfig, TunnelEndpointSide, TunnelPlan,
};

use crate::{command_worker::CommandCancelToken, state_dir::agent_state_dir};

use super::{extend_argv, run_runtime_command_cancelable, RuntimeCommandSpec};

// This is local kernel-resource ownership, not a second desired configuration.
// Keep attempted addresses until convergence so a retry can undo a partially
// applied update without touching addresses supplied by another owner.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct AddressOwnership {
    interface: String,
    ifindex: u32,
    addresses: Vec<String>,
    native_addr_gen_mode: Option<u8>,
}

pub(super) struct AddressReconcileInput<'a> {
    pub config: &'a AgentConfig,
    pub plan_id: Option<&'a str>,
    pub plan: &'a TunnelPlan,
    pub previous_plan: Option<&'a TunnelPlan>,
    pub endpoint: &'a TunnelEndpointConfig,
    pub created: bool,
    pub plan_uuid: Option<uuid::Uuid>,
}

pub(super) async fn needs_address_management(
    plan_id: Option<&str>,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Result<bool> {
    if vpsman_common::tunnel_endpoint_manages_link_local(plan, endpoint)
        || !endpoint.additional_addresses.ipv4.is_empty()
        || !endpoint.additional_addresses.ipv6.is_empty()
    {
        return Ok(true);
    }
    let path = address_state_path(plan_id, &plan.interface_name, endpoint.side)?;
    Ok(read_address_ownership(&path).await?.is_some())
}

pub(super) async fn reconcile_addresses(
    input: AddressReconcileInput<'_>,
    cancel_token: CommandCancelToken,
) -> Result<(Vec<serde_json::Value>, bool)> {
    let root = Path::new(&input.config.network.root_dir);
    let interface = &input.plan.interface_name;
    let ifindex =
        tokio::fs::read_to_string(root.join("sys/class/net").join(interface).join("ifindex"))
            .await
            .context("read managed tunnel interface index")?
            .trim()
            .parse::<u32>()
            .context("parse managed tunnel interface index")?;
    let state_path = address_state_path(input.plan_id, interface, input.endpoint.side)?;
    let previous = read_address_ownership(&state_path).await?.filter(|state| {
        state.interface == *interface && state.ifindex == ifindex && !input.created
    });
    let explicit =
        vpsman_common::tunnel_endpoint_explicit_address_cidrs(input.plan, input.endpoint);
    let manages_link_local =
        vpsman_common::tunnel_endpoint_manages_link_local(input.plan, input.endpoint);
    let mut desired = explicit;
    if manages_link_local {
        let generated = vpsman_common::tunnel_generated_link_local(
            input
                .plan_uuid
                .context("automatic tunnel link-local management requires a valid plan UUID")?,
            &input.endpoint.local_client_id,
        );
        if !desired
            .iter()
            .any(|cidr| cidr.split('/').next() == generated.split('/').next())
        {
            desired.push(generated);
        }
    }
    let mut previous_addresses = previous
        .as_ref()
        .map(|state| state.addresses.clone())
        .unwrap_or_default();
    if !input.created && previous.is_none() {
        if let Some(plan) = input.previous_plan {
            let endpoint = vpsman_common::render_tunnel_endpoint_config(plan, input.endpoint.side)?;
            previous_addresses =
                vpsman_common::tunnel_endpoint_explicit_address_cidrs(plan, &endpoint);
        }
    }
    let inspection = run_runtime_command_cancelable(
        "runtime_addresses_inspect",
        &extend_argv(
            &input.config.network.runtime_ip_argv,
            ["-j", "addr", "show", "dev", interface],
        ),
        false,
        true,
        input.config.network.runtime_command_timeout_secs,
        input.config.network.runtime_command_max_output_bytes as usize,
        cancel_token.clone(),
    )
    .await?;
    let mut reports = vec![inspection];
    if reports[0]["success"].as_bool() != Some(true) {
        return Ok((reports, false));
    }
    let current = observed_address_cidrs(
        reports[0]["stdout"]["text"].as_str().unwrap_or_default(),
        interface,
    )?;
    // Enabling this plan policy explicitly transfers the link-local set of the
    // owned interface to vpsman. Global addresses remain individually owned.
    if manages_link_local {
        previous_addresses.extend(current.iter().filter(|cidr| is_link_local(cidr)).cloned());
    }
    let native_mode = if manages_link_local {
        match previous
            .as_ref()
            .and_then(|state| state.native_addr_gen_mode)
        {
            Some(mode) => Some(mode),
            None => Some(read_addr_gen_mode(root, interface).await?),
        }
    } else {
        previous
            .as_ref()
            .and_then(|state| state.native_addr_gen_mode)
    };
    let removals = obsolete_owned_addresses(&previous_addresses, &desired, &current);
    let pending = AddressOwnership {
        interface: interface.clone(),
        ifindex,
        addresses: previous_addresses
            .iter()
            .chain(desired.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        native_addr_gen_mode: native_mode,
    };
    write_address_ownership(&state_path, &pending).await?;
    let mut specs = Vec::new();
    if manages_link_local {
        specs.push(addr_gen_mode_spec(
            &input.config.network.runtime_ip_argv,
            interface,
            1,
        )?);
    }
    for address in removals {
        specs.push(RuntimeCommandSpec {
            label: "runtime_addr_remove",
            argv: extend_argv(
                &input.config.network.runtime_ip_argv,
                ["addr", "del", &address, "dev", interface],
            ),
            mutates: true,
            required: true,
        });
    }
    specs.extend(super::build_address_replace_steps(
        &input.config.network.runtime_ip_argv,
        input.plan,
        input.endpoint,
        input.plan_uuid,
    )?);
    if !manages_link_local {
        if let Some(mode) = native_mode {
            specs.push(addr_gen_mode_spec(
                &input.config.network.runtime_ip_argv,
                interface,
                mode,
            )?);
        }
    }
    for spec in specs {
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
        let success = report["success"].as_bool() == Some(true);
        reports.push(report);
        if !success {
            return Ok((reports, false));
        }
    }
    write_address_ownership(
        &state_path,
        &AddressOwnership {
            interface: interface.clone(),
            ifindex,
            addresses: desired,
            native_addr_gen_mode: manages_link_local.then_some(native_mode).flatten(),
        },
    )
    .await?;
    Ok((reports, true))
}

fn obsolete_owned_addresses(
    previous: &[String],
    desired: &[String],
    current: &[String],
) -> Vec<String> {
    previous
        .iter()
        .filter(|address| !desired.contains(address) && current.contains(address))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn is_link_local(cidr: &str) -> bool {
    cidr.split('/')
        .next()
        .and_then(|address| address.parse::<std::net::Ipv6Addr>().ok())
        .is_some_and(|address| address.is_unicast_link_local())
}

fn observed_address_cidrs(raw: &str, interface: &str) -> Result<Vec<String>> {
    let links: Vec<serde_json::Value> =
        serde_json::from_str(raw).context("parse managed tunnel addresses")?;
    let link = links
        .iter()
        .find(|link| link["ifname"].as_str() == Some(interface))
        .context("managed tunnel address inspection omitted interface")?;
    Ok(link["addr_info"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|address| {
            let local = address["local"]
                .as_str()?
                .parse::<std::net::IpAddr>()
                .ok()?;
            let prefix = address["prefixlen"].as_u64()?;
            Some(format!("{local}/{prefix}"))
        })
        .collect())
}

async fn read_addr_gen_mode(root: &Path, interface: &str) -> Result<u8> {
    tokio::fs::read_to_string(
        root.join("proc/sys/net/ipv6/conf")
            .join(interface)
            .join("addr_gen_mode"),
    )
    .await
    .context("read managed tunnel native IPv6 address generation mode")?
    .trim()
    .parse()
    .context("parse managed tunnel IPv6 address generation mode")
}

fn addr_gen_mode_spec(base: &[String], interface: &str, mode: u8) -> Result<RuntimeCommandSpec> {
    let mode = match mode {
        0 => "eui64",
        1 => "none",
        2 => "stable_secret",
        3 => "random",
        _ => anyhow::bail!("unsupported native IPv6 address generation mode {mode}"),
    };
    Ok(RuntimeCommandSpec {
        label: "runtime_addr_gen_mode",
        argv: extend_argv(base, ["link", "set", "dev", interface, "addrgenmode", mode]),
        mutates: true,
        required: true,
    })
}

fn address_state_path(
    plan_id: Option<&str>,
    interface: &str,
    side: TunnelEndpointSide,
) -> Result<PathBuf> {
    let root = agent_state_dir()?.join("network-tunnels");
    let endpoint = if let Some(plan_id) = plan_id.and_then(|id| uuid::Uuid::parse_str(id).ok()) {
        root.join(plan_id.to_string())
    } else {
        root.join("local").join(interface)
    };
    Ok(endpoint.join(super::side_name(side)).join("addresses.json"))
}

pub(super) async fn remove_address_ownership(
    plan_id: Option<&str>,
    interface: &str,
    side: TunnelEndpointSide,
) -> Result<()> {
    let path = address_state_path(plan_id, interface, side)?;
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("remove managed tunnel address ownership"),
    }
    // The driver may have removed its own files before this shared state. Only
    // remove the now-empty endpoint/plan directories, never another owner's data.
    for directory in path
        .parent()
        .into_iter()
        .chain(path.parent().and_then(Path::parent))
    {
        match tokio::fs::remove_dir(directory).await {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => return Err(error).context("remove empty managed tunnel state directory"),
        }
    }
    Ok(())
}

async fn read_address_ownership(path: &Path) -> Result<Option<AddressOwnership>> {
    match tokio::fs::read(path).await {
        Ok(raw) => Ok(Some(
            serde_json::from_slice(&raw).context("parse managed tunnel address ownership")?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("read managed tunnel address ownership"),
    }
}

async fn write_address_ownership(path: &Path, state: &AddressOwnership) -> Result<()> {
    let root = agent_state_dir()?.join("network-tunnels");
    ensure_private_dir_tree_async(
        &root,
        path.parent()
            .context("managed address state directory is absent")?,
    )
    .await?;
    write_private_file_atomically_async(path, &serde_json::to_vec(state)?).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_diff_preserves_unowned_and_still_desired_addresses() {
        let previous = vec![
            "10.0.0.1/32".into(),
            "fe80::1/64".into(),
            "fd00::1/128".into(),
        ];
        let desired = vec!["10.0.0.1/32".into(), "fe80::2/64".into()];
        let current = vec![
            "10.0.0.1/32".into(),
            "fe80::1/64".into(),
            "fe80::999/64".into(),
        ];
        assert_eq!(
            obsolete_owned_addresses(&previous, &desired, &current),
            ["fe80::1/64"]
        );
    }

    #[test]
    fn address_inspection_keeps_local_primary_peer_address() {
        let raw = r#"[{"ifname":"tunab","addr_info":[{"local":"10.0.0.1","address":"10.0.0.2","prefixlen":31},{"local":"fe80:0:0:0:0:0:0:1","prefixlen":64}]}]"#;
        assert_eq!(
            observed_address_cidrs(raw, "tunab").unwrap(),
            ["10.0.0.1/31", "fe80::1/64"]
        );
    }
}
