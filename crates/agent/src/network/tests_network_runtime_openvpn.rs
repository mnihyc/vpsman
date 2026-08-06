use super::*;
use vpsman_common::{
    plan_tunnel, RuntimeTunnelControl, RuntimeTunnelOpenvpnOptions, RuntimeTunnelOpenvpnTransport,
    TunnelAddressFamily, TunnelAddressPair, TunnelPlanInput,
};

fn openvpn_plan(
    transport: RuntimeTunnelOpenvpnTransport,
    listener_side: TunnelEndpointSide,
    left_local_underlay: Option<&str>,
    right_local_underlay: Option<&str>,
) -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "openvpn-test".to_string(),
        interface_name: "ovpn0".to_string(),
        kind: vpsman_common::TunnelKind::Openvpn,
        runtime_control: RuntimeTunnelControl {
            openvpn: RuntimeTunnelOpenvpnOptions {
                transport,
                listener_side,
                port: 1194,
            },
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "v-1".to_string(),
        right_client_id: "v-2".to_string(),
        left_remote_underlay: "198.51.100.20".to_string(),
        left_local_underlay: left_local_underlay.map(str::to_string),
        right_remote_underlay: "198.51.100.10".to_string(),
        right_local_underlay: right_local_underlay.map(str::to_string),
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
        left_mtu: Some(1500),
        right_mtu: Some(1500),
        ospf: None,
    })
    .unwrap()
}

fn rendered_for_version(plan: &TunnelPlan, side: TunnelEndpointSide, version: Version) -> String {
    let endpoint = vpsman_common::render_tunnel_endpoint_config(plan, side).unwrap();
    render_openvpn_config(
        plan,
        &endpoint,
        Path::new("/state/openvpn.key"),
        Path::new("/state/openvpn.crt"),
        Path::new("/state/openvpn-peer-ca.crt"),
        Path::new("/state/openvpn.pid"),
        Path::new("/state/openvpn.status"),
        &version,
    )
    .unwrap()
}

fn rendered(plan: &TunnelPlan, side: TunnelEndpointSide) -> String {
    rendered_for_version(plan, side, Version::new(2, 6, 12))
}

#[test]
fn listener_config_is_pinned_p2p_tls_and_carries_both_inner_families() {
    let config = rendered(
        &openvpn_plan(
            RuntimeTunnelOpenvpnTransport::Udp,
            TunnelEndpointSide::Left,
            Some("198.51.100.10"),
            None,
        ),
        TunnelEndpointSide::Left,
    );
    for expected in [
        "mode p2p",
        "dev ovpn0",
        "proto udp4",
        "tls-server",
        "remote-cert-tls client",
        "ca \"/state/openvpn-peer-ca.crt\"",
        "dh none",
        "lport 1194",
        "local 198.51.100.10",
        "ifconfig 10.0.0.0 10.0.0.1",
        "ifconfig-ipv6 fd00::/127 fd00::1",
    ] {
        assert!(config.lines().any(|line| line == expected), "{expected}");
    }
    assert!(!config.contains("nobind"));
    assert!(!config.contains("script-security"));
    assert!(!config.contains("compress"));
    assert!(!config.contains("peer-fingerprint"));
}

#[test]
fn initiator_with_explicit_source_uses_ephemeral_port_without_nobind() {
    let config = rendered(
        &openvpn_plan(
            RuntimeTunnelOpenvpnTransport::Tcp,
            TunnelEndpointSide::Left,
            None,
            Some("198.51.100.20"),
        ),
        TunnelEndpointSide::Right,
    );
    for expected in [
        "proto tcp4-client",
        "tls-client",
        "remote-cert-tls server",
        "remote 198.51.100.10 1194",
        "local 198.51.100.20",
        "lport 0",
    ] {
        assert!(config.lines().any(|line| line == expected), "{expected}");
    }
    assert!(!config.contains("nobind"));
}

#[test]
fn initiator_without_source_uses_nobind() {
    let config = rendered(
        &openvpn_plan(
            RuntimeTunnelOpenvpnTransport::Udp,
            TunnelEndpointSide::Left,
            None,
            None,
        ),
        TunnelEndpointSide::Right,
    );
    assert!(config.lines().any(|line| line == "nobind"));
    assert!(!config.lines().any(|line| line == "lport 0"));
}

#[test]
fn listener_family_follows_the_initiators_destination_for_asymmetric_underlays() {
    let mut left_listener = openvpn_plan(
        RuntimeTunnelOpenvpnTransport::Udp,
        TunnelEndpointSide::Left,
        None,
        None,
    );
    left_listener.left_remote_underlay = "198.51.100.20".to_string();
    left_listener.right_remote_underlay = "2001:db8::10".to_string();
    assert!(rendered(&left_listener, TunnelEndpointSide::Left)
        .lines()
        .any(|line| line == "proto udp6"));
    assert!(rendered(&left_listener, TunnelEndpointSide::Right)
        .lines()
        .any(|line| line == "proto udp6"));

    let mut right_listener = openvpn_plan(
        RuntimeTunnelOpenvpnTransport::Tcp,
        TunnelEndpointSide::Right,
        None,
        None,
    );
    right_listener.left_remote_underlay = "2001:db8::20".to_string();
    right_listener.right_remote_underlay = "198.51.100.10".to_string();
    assert!(rendered(&right_listener, TunnelEndpointSide::Right)
        .lines()
        .any(|line| line == "proto tcp6-server"));
    assert!(rendered(&right_listener, TunnelEndpointSide::Left)
        .lines()
        .any(|line| line == "proto tcp6-client"));
}

#[test]
fn version_parser_accepts_supported_lines_and_rejects_unversioned_output() {
    assert_eq!(
        parse_openvpn_version("OpenVPN 2.4.12 x86_64-pc-linux-gnu"),
        Some(Version::new(2, 4, 12))
    );
    assert_eq!(
        parse_openvpn_version("OpenVPN 2.5.11 x86_64-pc-linux-gnu"),
        Some(Version::new(2, 5, 11))
    );
    assert_eq!(
        parse_openvpn_version("OpenVPN 2.6.12 x86_64-pc-linux-gnu"),
        Some(Version::new(2, 6, 12))
    );
    assert_eq!(parse_openvpn_version("OpenVPN unknown"), None);
    assert_eq!(
        parse_openvpn_version("OpenVPN unknown built with OpenSSL 3.0.0"),
        None
    );
    assert_eq!(parse_openvpn_version("OpenSSL 3.0.0"), None);
}

#[tokio::test]
async fn prerequisite_probe_rejects_partial_version_output_from_failed_command() {
    let mut config = AgentConfig::default();
    config.network.runtime_openvpn_argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "printf 'OpenVPN 2.6.12 partial\\n'; exit 1".to_string(),
    ];

    let error = inspect_openvpn_prerequisites(&config, CommandCancelToken::default())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("version probe failed"));
}

#[test]
fn reconcile_always_restores_declared_mtu_and_link_state() {
    let plan = openvpn_plan(
        RuntimeTunnelOpenvpnTransport::Udp,
        TunnelEndpointSide::Left,
        None,
        None,
    );
    let endpoint =
        vpsman_common::render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let prepared = PreparedOpenvpnState {
        endpoint_dir: PathBuf::from("/state"),
        config_path: PathBuf::from("/state/openvpn.conf"),
        pid_path: PathBuf::from("/state/openvpn.pid"),
        config_hash: "hash".to_string(),
    };
    let steps =
        build_openvpn_reconcile_steps(&AgentConfig::default(), &plan, &endpoint, &prepared, true)
            .unwrap();
    let labels = steps.iter().map(|step| step.label).collect::<Vec<_>>();
    assert!(labels.contains(&"runtime_link_mtu"));
    assert!(labels.contains(&"runtime_link_up"));
    assert!(!labels.contains(&"runtime_openvpn_start"));
}

#[test]
fn cipher_directive_is_explicitly_adapted_for_openvpn_24_through_26() {
    let plan = openvpn_plan(
        RuntimeTunnelOpenvpnTransport::Udp,
        TunnelEndpointSide::Left,
        None,
        None,
    );
    let version_24 = rendered_for_version(&plan, TunnelEndpointSide::Left, Version::new(2, 4, 12));
    assert!(version_24.contains("ncp-ciphers AES-256-GCM:AES-128-GCM"));
    assert!(!version_24.contains("data-ciphers"));

    for version in [Version::new(2, 5, 11), Version::new(2, 6, 12)] {
        let config = rendered_for_version(&plan, TunnelEndpointSide::Left, version);
        assert!(config.contains("data-ciphers AES-256-GCM:AES-128-GCM"));
        assert!(!config.contains("ncp-ciphers"));
    }
}

#[test]
fn applied_hash_changes_when_the_local_identity_changes() {
    let first = openvpn_applied_hash(
        b"same config",
        b"key one",
        b"certificate one",
        b"peer issuer one",
        b"peer fingerprint one",
    );
    let second = openvpn_applied_hash(
        b"same config",
        b"key two",
        b"certificate two",
        b"peer issuer one",
        b"peer fingerprint one",
    );
    assert_ne!(first, second);
}

#[test]
fn applied_hash_changes_when_the_peer_identity_changes() {
    let first = openvpn_applied_hash(
        b"same config",
        b"same key",
        b"same certificate",
        b"peer issuer one",
        b"peer fingerprint one",
    );
    let second = openvpn_applied_hash(
        b"same config",
        b"same key",
        b"same certificate",
        b"peer issuer two",
        b"peer fingerprint two",
    );
    assert_ne!(first, second);
}
