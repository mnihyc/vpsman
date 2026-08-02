use anyhow::Result;

use crate::vty_jobs::VtyPrivilegeContext;
use crate::vty_network::{
    parse_vty_tunnel_allocate, parse_vty_tunnel_ospf_status_refresh, parse_vty_tunnel_plan,
    parse_vty_tunnel_plan_export, parse_vty_tunnel_plan_mutation, parse_vty_tunnel_status,
    submit_or_render_vty_tunnel_plan, submit_vty_tunnel_allocate,
    submit_vty_tunnel_ospf_status_refresh, submit_vty_tunnel_plan_delete,
    submit_vty_tunnel_plan_enabled, submit_vty_tunnel_plan_export, submit_vty_tunnel_status,
};
use crate::vty_network_ospf::{
    parse_vty_tunnel_ospf_cost_update, submit_vty_tunnel_ospf_cost_update,
};
use crate::vty_network_probe::{parse_vty_tunnel_probe, submit_vty_tunnel_probe};
use crate::vty_network_speed::{parse_vty_tunnel_speed_test, submit_vty_tunnel_speed_test};

pub(crate) fn is_vty_network_dispatch_command(command: &str) -> bool {
    command.starts_with("tunnel-plan ")
        || command.starts_with("tunnel-plan-export ")
        || command.starts_with("tunnel-plan-enable ")
        || command.starts_with("tunnel-plan-disable ")
        || command.starts_with("tunnel-plan-delete ")
        || command.starts_with("tunnel-allocate ")
        || command.starts_with("tunnel-ospf-status-refresh ")
        || command.starts_with("tunnel-ospf-cost-update ")
        || command.starts_with("tunnel-status ")
        || command.starts_with("tunnel-probe ")
        || command.starts_with("tunnel-speed-test ")
}

pub(crate) fn submit_vty_network_dispatch_command(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    command: &str,
) -> Result<()> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    match parts.first().copied().unwrap_or_default() {
        "tunnel-plan" => {
            let request = match parse_vty_tunnel_plan(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-plan --name <name> --interface-name <ifname> --kind <gre|ipip|sit|fou|openvpn|wireguard|tun_tap|custom> --left-client-id <id> --right-client-id <id> --left-remote-underlay <ip> [--left-local-underlay <ip>] --right-remote-underlay <ip> [--right-local-underlay <ip>] (--left-tunnel-ipv4-cidr <ip/prefix> --right-tunnel-ipv4-cidr <ip/prefix> and/or --left-tunnel-ipv6-cidr <ip/prefix> --right-tunnel-ipv6-cidr <ip/prefix>) [--address-pool-cidr <cidr>] [--ipv6-address-pool-cidr <cidr>] [--latency-primary-family <ipv4|ipv6>] --bandwidth-mbps <10..10000> [--runtime-manager <agent|observed|adapter>] [--left-runtime-adapter-definition-id <uuid> --right-runtime-adapter-definition-id <uuid>] [--ospf --ospf-latency-ms <ms> [--left-routing-adapter-definition-id <uuid>] [--right-routing-adapter-definition-id <uuid>] --ospf-mode <reviewed|automatic> --ospf-min-cost-delta <cost> --ospf-healthy-windows <1..10> --ospf-latency-weight <number> --ospf-loss-weight <number> --ospf-bandwidth-weight <number> --ospf-preference-bias <number> --ospf-min-cost <cost> --ospf-max-cost <cost>] [--fou-port <1-65535>] [--fou-peer-port <1-65535>] [--fou-ipproto <1-255>] [--reserved-address <ip>] [--save --enabled --confirmed] [--update-plan-id <uuid> --expected-revision <revision>]"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_or_render_vty_tunnel_plan(api_url, token, request)?
            );
        }
        "tunnel-plan-export" => {
            let request = match parse_vty_tunnel_plan_export(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-plan-export --plan-id <uuid> [--output-file ./plan.json]"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_plan_export(api_url, token, request)?
            );
        }
        "tunnel-plan-enable" | "tunnel-plan-disable" | "tunnel-plan-delete" => {
            let command_name = parts[0];
            let request = match parse_vty_tunnel_plan_mutation(&parts[1..], command_name) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: {command_name} --plan-id <uuid> --expected-revision <revision> --confirmed"
                    );
                    return Ok(());
                }
            };
            let response = if command_name == "tunnel-plan-delete" {
                submit_vty_tunnel_plan_delete(api_url, token, request)?
            } else {
                submit_vty_tunnel_plan_enabled(
                    api_url,
                    token,
                    request,
                    command_name == "tunnel-plan-enable",
                )?
            };
            println!("{response}");
        }
        "tunnel-allocate" => {
            let request = match parse_vty_tunnel_allocate(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-allocate [--ipv4-pool-cidr <cidr>] [--ipv6-pool-cidr <cidr>] [--reserved-address <ip>] [--include-ipv4=true|false|--no-ipv4] [--include-ipv6|--include-ipv6=true|false]"
                    );
                    return Ok(());
                }
            };
            println!("{}", submit_vty_tunnel_allocate(api_url, token, request)?);
        }
        "tunnel-ospf-cost-update" => {
            let request = match parse_vty_tunnel_ospf_cost_update(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-ospf-cost-update --plan-id <uuid> --plan-revision <revision> --recommendation-id <id> [--left-current-ospf-cost <1-65535>] [--right-current-ospf-cost <1-65535>] --desired-ospf-cost <1-65535> --left-adapter-definition-hash <sha256> --right-adapter-definition-hash <sha256> --confirmed"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_ospf_cost_update(api_url, token, request)?
            );
        }
        "tunnel-ospf-status-refresh" => {
            let request = match parse_vty_tunnel_ospf_status_refresh(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!("usage: tunnel-ospf-status-refresh --plan-id <uuid>");
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_ospf_status_refresh(api_url, token, request)?
            );
        }
        "tunnel-status" => {
            let request = match parse_vty_tunnel_status(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-status --plan-id <uuid> --side <left|right> [--max-timeout <secs>]"
                    );
                    return Ok(());
                }
            };
            println!("{}", submit_vty_tunnel_status(api_url, token, request)?);
        }
        "tunnel-probe" => {
            if !require_privilege_unlock(privilege_context) {
                return Ok(());
            }
            let request = match parse_vty_tunnel_probe(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-probe --plan-id <uuid> --side <left|right> [--count <1-20>] [--interval-ms <200-10000>] [--max-timeout <secs>] [--privilege-ttl <15-300>]"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_probe(api_url, token, privilege_context, request)?
            );
        }
        "tunnel-speed-test" => {
            if !require_privilege_unlock(privilege_context) {
                return Ok(());
            }
            let request = match parse_vty_tunnel_speed_test(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-speed-test --plan-id <uuid> --server-side <left|right> [--duration-secs <1-30>] [--max-bytes <16384-268435456>] [--rate-limit-kbps <64-1000000>] [--port <1024-65535>] [--connect-timeout-ms <100-30000>] [--max-timeout <secs>] [--privilege-ttl <15-300>] --confirmed"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_speed_test(api_url, token, privilege_context, request)?
            );
        }
        _ => {}
    }

    Ok(())
}

fn require_privilege_unlock(privilege_context: &VtyPrivilegeContext) -> bool {
    if privilege_context.enabled {
        return true;
    }
    println!(
        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
    );
    false
}

#[cfg(test)]
#[path = "tests_vty_network_dispatch.rs"]
mod tests;
