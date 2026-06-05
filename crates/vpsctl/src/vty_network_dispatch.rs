use anyhow::Result;

use crate::vty_jobs::VtyProofContext;
use crate::vty_network::{
    parse_vty_tunnel_apply, parse_vty_tunnel_plan, parse_vty_tunnel_promote_telemetry,
    parse_vty_tunnel_rollback, parse_vty_tunnel_status, submit_or_render_vty_tunnel_plan,
    submit_vty_tunnel_apply, submit_vty_tunnel_promote_telemetry, submit_vty_tunnel_rollback,
    submit_vty_tunnel_status,
};
use crate::vty_network_adapter::{
    parse_vty_tunnel_promote_adapter, submit_vty_tunnel_promote_adapter,
};
use crate::vty_network_ospf::{
    parse_vty_tunnel_ospf_cost_update, submit_vty_tunnel_ospf_cost_update,
};
use crate::vty_network_probe::{parse_vty_tunnel_probe, submit_vty_tunnel_probe};
use crate::vty_network_speed::{parse_vty_tunnel_speed_test, submit_vty_tunnel_speed_test};

pub(crate) fn is_vty_network_dispatch_command(command: &str) -> bool {
    command.starts_with("tunnel-plan ")
        || command.starts_with("tunnel-promote-adapter ")
        || command.starts_with("tunnel-promote-telemetry ")
        || command.starts_with("tunnel-apply ")
        || command.starts_with("tunnel-ospf-cost-update ")
        || command.starts_with("tunnel-rollback ")
        || command.starts_with("tunnel-status ")
        || command.starts_with("tunnel-probe ")
        || command.starts_with("tunnel-speed-test ")
}

pub(crate) fn submit_vty_network_dispatch_command(
    api_url: &str,
    token: Option<&str>,
    proof_context: &VtyProofContext,
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
                        "usage: tunnel-plan --name <name> --interface-name <ifname> --kind <gre|ipip|sit|fou|openvpn|wireguard|tun_tap|custom> --left-client-id <id> --right-client-id <id> --left-underlay <ip> --right-underlay <ip> --address-pool-cidr <cidr> --bandwidth <10m|100m|1000m> --latency-ms <ms> [--runtime-manager <agent|observed|adapter>] [--runtime-startup-argv <abs,arg>] [--runtime-stop-argv <abs,arg>] [--runtime-cleanup-argv <abs,arg>] [--runtime-status-argv <abs,arg>] [--fou-port <1-65535>] [--fou-peer-port <1-65535>] [--fou-ipproto <1-255>] [--packet-loss-ratio <0-1>] [--preference <value>] [--reserved-address <ip>] [--save]"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_or_render_vty_tunnel_plan(api_url, token, request)?
            );
        }
        "tunnel-promote-telemetry" => {
            let request = match parse_vty_tunnel_promote_telemetry(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-promote-telemetry --client-id <id> --interface <ifname> --peer-client-id <id> --local-underlay <ip> --peer-underlay <ip> --address-pool-cidr <cidr> [--side <left|right>] [--name <name>] [--bandwidth <10m|100m|1000m>]"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_promote_telemetry(api_url, token, request)?
            );
        }
        "tunnel-promote-adapter" => {
            let request = match parse_vty_tunnel_promote_adapter(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-promote-adapter --plan-id <uuid> --runtime-status-argv <abs,arg> [--runtime-startup-argv <abs,arg>] [--runtime-stop-argv <abs,arg>] [--runtime-cleanup-argv <abs,arg>] [--name <name>] --confirmed"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_promote_adapter(api_url, token, request)?
            );
        }
        "tunnel-apply" => {
            if !require_proof(proof_context) {
                return Ok(());
            }
            let request = match parse_vty_tunnel_apply(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-apply --plan-file <plan.json> --side <left|right> [--backend <ifupdown|netplan|systemd-networkd>] [--timeout <1-3600>] [--proof-ttl <1-3600>] [--force-unprivileged] --confirmed"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_apply(api_url, token, proof_context, request)?
            );
        }
        "tunnel-ospf-cost-update" => {
            if !require_proof(proof_context) {
                return Ok(());
            }
            let request = match parse_vty_tunnel_ospf_cost_update(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-ospf-cost-update --plan-file <plan.json> --side <left|right> --current-ospf-cost <1-65535> --recommended-ospf-cost <1-65535> [--timeout <1-3600>] [--proof-ttl <1-3600>] [--force-unprivileged] --confirmed"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_ospf_cost_update(api_url, token, proof_context, request)?
            );
        }
        "tunnel-rollback" => {
            if !require_proof(proof_context) {
                return Ok(());
            }
            let request = match parse_vty_tunnel_rollback(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-rollback --plan-file <plan.json> --side <left|right> [--timeout <1-3600>] [--proof-ttl <1-3600>] [--force-unprivileged] --confirmed"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_rollback(api_url, token, proof_context, request)?
            );
        }
        "tunnel-status" => {
            if !require_proof(proof_context) {
                return Ok(());
            }
            let request = match parse_vty_tunnel_status(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-status --plan-file <plan.json> --side <left|right> [--timeout <1-3600>] [--proof-ttl <1-3600>]"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_status(api_url, token, proof_context, request)?
            );
        }
        "tunnel-probe" => {
            if !require_proof(proof_context) {
                return Ok(());
            }
            let request = match parse_vty_tunnel_probe(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-probe --plan-file <plan.json> --side <left|right> [--count <1-20>] [--interval-ms <200-10000>] [--timeout <1-3600>] [--proof-ttl <1-3600>]"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_probe(api_url, token, proof_context, request)?
            );
        }
        "tunnel-speed-test" => {
            if !require_proof(proof_context) {
                return Ok(());
            }
            let request = match parse_vty_tunnel_speed_test(&parts[1..]) {
                Ok(request) => request,
                Err(error) => {
                    println!("usage error: {error}");
                    println!(
                        "usage: tunnel-speed-test --plan-file <plan.json> --server-side <left|right> [--duration-secs <1-30>] [--max-bytes <16384-268435456>] [--rate-limit-kbps <64-1000000>] [--port <1024-65535>] [--connect-timeout-ms <100-30000>] [--timeout <1-3600>] [--proof-ttl <1-3600>]"
                    );
                    return Ok(());
                }
            };
            println!(
                "{}",
                submit_vty_tunnel_speed_test(api_url, token, proof_context, request)?
            );
        }
        _ => {}
    }

    Ok(())
}

fn require_proof(proof_context: &VtyProofContext) -> bool {
    if proof_context.enabled {
        return true;
    }
    println!(
        "privileged proof is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
    );
    false
}

#[cfg(test)]
mod tests {
    use super::is_vty_network_dispatch_command;

    #[test]
    fn recognizes_network_dispatch_commands() {
        for command in [
            "tunnel-plan --name n",
            "tunnel-promote-adapter --plan-id 00000000-0000-0000-0000-000000000000",
            "tunnel-promote-telemetry --client-id edge-a",
            "tunnel-apply --plan-file plan.json",
            "tunnel-ospf-cost-update --plan-file plan.json",
            "tunnel-rollback --plan-file plan.json",
            "tunnel-status --plan-file plan.json",
            "tunnel-probe --plan-file plan.json",
            "tunnel-speed-test --plan-file plan.json",
        ] {
            assert!(is_vty_network_dispatch_command(command), "{command}");
        }

        for command in [
            "tunnel-plans",
            "network-observations",
            "job-create uptime id:edge",
            "tunnel-plan",
        ] {
            assert!(!is_vty_network_dispatch_command(command), "{command}");
        }
    }
}
