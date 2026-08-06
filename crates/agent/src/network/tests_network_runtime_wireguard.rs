use super::*;
use vpsman_common::{
    plan_tunnel, RuntimeTunnelControl, RuntimeTunnelWireguardEndpointMode,
    RuntimeTunnelWireguardOptions, TunnelAddressFamily, TunnelAddressPair, TunnelPlanInput,
};

const LOCAL_PRIVATE: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const LOCAL_PUBLIC: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";
const PEER_PUBLIC: &str = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC=";
const OLD_PEER_PUBLIC: &str = "DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD=";

fn wireguard_plan(endpoint_mode: RuntimeTunnelWireguardEndpointMode) -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "wireguard-test".to_string(),
        interface_name: "wg-test".to_string(),
        kind: vpsman_common::TunnelKind::Wireguard,
        runtime_control: RuntimeTunnelControl {
            wireguard: RuntimeTunnelWireguardOptions {
                endpoint_mode,
                left_listen_port: 51820,
                right_listen_port: 51821,
                left_keepalive_secs: 25,
                right_keepalive_secs: 0,
            },
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "v-1".to_string(),
        right_client_id: "v-2".to_string(),
        left_remote_underlay: "2001:db8::2".to_string(),
        left_local_underlay: None,
        right_remote_underlay: "2001:db8::1".to_string(),
        right_local_underlay: None,
        address_pool_cidr: "10.0.0.0/31".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.0.0.0".to_string(),
            right: "10.0.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: Some("fd00::/127".to_string()),
        ipv6_tunnel: Some(TunnelAddressPair {
            left: "fd00::".to_string(),
            right: "fd00::1".to_string(),
            prefix_len: 127,
        }),
        latency_primary_family: TunnelAddressFamily::Ipv4,
        bandwidth_mbps: 100,
        left_mtu: Some(1420),
        right_mtu: Some(1420),
        ospf: None,
    })
    .unwrap()
}

fn credentials() -> TunnelEndpointBuiltinCredentials {
    TunnelEndpointBuiltinCredentials::Wireguard {
        generation: 1,
        local_private_key_base64: LOCAL_PRIVATE.to_string(),
        local_public_key_base64: LOCAL_PUBLIC.to_string(),
        peer_public_key_base64: PEER_PUBLIC.to_string(),
    }
}

fn prepared(previous_applied: Option<AppliedWireguardState>) -> PreparedWireguardState {
    PreparedWireguardState {
        private_key_path: PathBuf::from("/state/wireguard.key"),
        applied_state_path: PathBuf::from("/state/wireguard.applied.json"),
        pending_state_path: PathBuf::from("/state/wireguard.pending.json"),
        previous_applied,
        pending: None,
    }
}

fn prepared_with_pending(
    previous_applied: Option<AppliedWireguardState>,
    pending: Option<AppliedWireguardState>,
) -> PreparedWireguardState {
    PreparedWireguardState {
        pending,
        ..prepared(previous_applied)
    }
}

fn steps(
    mode: RuntimeTunnelWireguardEndpointMode,
    side: TunnelEndpointSide,
    existing_peers: &[String],
    previous_applied: Option<AppliedWireguardState>,
) -> Vec<RuntimeCommandSpec> {
    let plan = wireguard_plan(mode);
    let endpoint = vpsman_common::render_tunnel_endpoint_config(&plan, side).unwrap();
    build_wireguard_reconcile_steps(
        &AgentConfig::default(),
        &plan,
        &endpoint,
        Some(&credentials()),
        &prepared(previous_applied),
        true,
        existing_peers,
    )
    .unwrap()
}

#[test]
fn both_mode_sets_bracketed_ipv6_endpoint_and_both_allowed_families() {
    let steps = steps(
        RuntimeTunnelWireguardEndpointMode::Both,
        TunnelEndpointSide::Left,
        &[],
        None,
    );
    let configure = steps
        .iter()
        .find(|step| step.label == "runtime_wireguard_configure")
        .unwrap();
    assert!(configure
        .argv
        .windows(2)
        .any(|pair| { pair == ["endpoint", "[2001:db8::2]:51821"] }));
    assert!(configure
        .argv
        .windows(2)
        .any(|pair| pair == ["allowed-ips", "0.0.0.0/0,::/0"]));
    assert_eq!(
        steps
            .iter()
            .filter(|step| step.label == "runtime_addr_replace")
            .count(),
        2
    );
}

#[test]
fn one_sided_endpoint_mode_points_the_roaming_side_at_the_fixed_vps() {
    let left = steps(
        RuntimeTunnelWireguardEndpointMode::Right,
        TunnelEndpointSide::Left,
        &[],
        None,
    );
    let configure = left
        .iter()
        .find(|step| step.label == "runtime_wireguard_configure")
        .unwrap();
    assert!(configure.argv.iter().any(|value| value == "endpoint"));
    assert!(configure
        .argv
        .windows(2)
        .any(|pair| pair == ["persistent-keepalive", "25"]));

    let right = steps(
        RuntimeTunnelWireguardEndpointMode::Right,
        TunnelEndpointSide::Right,
        &[],
        None,
    );
    let configure = right
        .iter()
        .find(|step| step.label == "runtime_wireguard_configure")
        .unwrap();
    assert!(!configure.argv.iter().any(|value| value == "endpoint"));
}

#[test]
fn idempotent_reconcile_does_not_remove_the_desired_peer() {
    let steps = steps(
        RuntimeTunnelWireguardEndpointMode::Both,
        TunnelEndpointSide::Left,
        &[PEER_PUBLIC.to_string()],
        Some(AppliedWireguardState {
            local_public_key_base64: LOCAL_PUBLIC.to_string(),
            peer_public_key_base64: PEER_PUBLIC.to_string(),
            peer_endpoint_configured: true,
        }),
    );
    assert!(!steps
        .iter()
        .any(|step| step.label == "runtime_wireguard_peer_remove"));
}

#[test]
fn peer_rotation_configures_and_verifies_before_removing_the_previous_peer() {
    let steps = steps(
        RuntimeTunnelWireguardEndpointMode::Both,
        TunnelEndpointSide::Left,
        &[OLD_PEER_PUBLIC.to_string()],
        Some(AppliedWireguardState {
            local_public_key_base64: LOCAL_PUBLIC.to_string(),
            peer_public_key_base64: OLD_PEER_PUBLIC.to_string(),
            peer_endpoint_configured: true,
        }),
    );
    let labels = steps.iter().map(|step| step.label).collect::<Vec<_>>();
    let configure = labels
        .iter()
        .position(|label| *label == "runtime_wireguard_configure")
        .unwrap();
    let verify = labels
        .iter()
        .position(|label| *label == "runtime_wireguard_public_key_verify")
        .unwrap();
    let remove = labels
        .iter()
        .position(|label| *label == "runtime_wireguard_peer_remove")
        .unwrap();
    assert!(configure < verify && verify < remove);
}

#[test]
fn changing_from_fixed_to_roaming_explicitly_resets_the_desired_peer() {
    let steps = steps(
        RuntimeTunnelWireguardEndpointMode::Left,
        TunnelEndpointSide::Left,
        &[PEER_PUBLIC.to_string()],
        Some(AppliedWireguardState {
            local_public_key_base64: LOCAL_PUBLIC.to_string(),
            peer_public_key_base64: PEER_PUBLIC.to_string(),
            peer_endpoint_configured: true,
        }),
    );
    let labels = steps.iter().map(|step| step.label).collect::<Vec<_>>();
    assert!(labels.contains(&"runtime_wireguard_roaming_peer_reset"));
    assert!(labels.contains(&"runtime_wireguard_configure_roaming"));
}

#[test]
fn interrupted_fixed_to_roaming_transition_still_clears_the_old_endpoint() {
    let plan = wireguard_plan(RuntimeTunnelWireguardEndpointMode::Left);
    let endpoint =
        vpsman_common::render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let prepared = prepared_with_pending(
        Some(AppliedWireguardState {
            local_public_key_base64: LOCAL_PUBLIC.to_string(),
            peer_public_key_base64: PEER_PUBLIC.to_string(),
            peer_endpoint_configured: true,
        }),
        Some(AppliedWireguardState {
            local_public_key_base64: LOCAL_PUBLIC.to_string(),
            peer_public_key_base64: PEER_PUBLIC.to_string(),
            peer_endpoint_configured: false,
        }),
    );
    let steps = build_wireguard_reconcile_steps(
        &AgentConfig::default(),
        &plan,
        &endpoint,
        Some(&credentials()),
        &prepared,
        true,
        &[PEER_PUBLIC.to_string()],
    )
    .unwrap();
    assert!(steps
        .iter()
        .any(|step| step.label == "runtime_wireguard_roaming_peer_reset"));
}
