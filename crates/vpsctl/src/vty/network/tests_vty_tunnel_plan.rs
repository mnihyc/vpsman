use super::parse_vty_tunnel_plan;
use vpsman_common::{RuntimeTunnelManager, TunnelAddressFamily, TunnelKind};

#[test]
fn parses_vty_tunnel_plan_for_local_render() {
    let request = parse_vty_tunnel_plan(&[
        "--name=lax-hkg",
        "--interface-name",
        "vpnlaxhkg",
        "--kind",
        "gre",
        "--left-client-id",
        "lax",
        "--right-client-id=hkg",
        "--left-remote-underlay",
        "203.0.113.20",
        "--left-local-underlay=10.0.0.10",
        "--right-remote-underlay=198.51.100.10",
        "--right-local-underlay=10.0.1.20",
        "--address-pool-cidr",
        "10.255.0.0/30",
        "--left-tunnel-ipv4-cidr=10.255.0.0/31",
        "--right-tunnel-ipv4-cidr=10.255.0.1/31",
        "--reserved-address",
        "10.255.0.2,10.255.0.3",
        "--bandwidth-mbps",
        "1000",
    ])
    .unwrap();

    assert!(!request.save);
    assert!(!request.enabled);
    assert_eq!(request.input.name, "lax-hkg");
    assert_eq!(request.input.interface_name, "vpnlaxhkg");
    assert_eq!(request.input.kind, TunnelKind::Gre);
    assert_eq!(request.input.left_client_id, "lax");
    assert_eq!(request.input.right_client_id, "hkg");
    assert_eq!(request.input.left_remote_underlay, "203.0.113.20");
    assert_eq!(
        request.input.left_local_underlay.as_deref(),
        Some("10.0.0.10")
    );
    assert_eq!(request.input.right_remote_underlay, "198.51.100.10");
    assert_eq!(
        request.input.right_local_underlay.as_deref(),
        Some("10.0.1.20")
    );
    assert_eq!(
        request.input.reserved_addresses,
        vec!["10.255.0.2", "10.255.0.3"]
    );
    assert_eq!(request.input.bandwidth_mbps, 1000);
    assert!(request.input.ospf.is_none());
}

#[test]
fn parses_vty_tunnel_plan_save_aliases() {
    let request = parse_vty_tunnel_plan(&[
        "--save",
        "--update-plan-id=00000000-0000-4000-8000-000000000001",
        "--expected-revision=7",
        "--name",
        "edge",
        "--interface",
        "vpsedge",
        "--kind=fou",
        "--left-client",
        "left",
        "--right-client",
        "right",
        "--left-remote-underlay=198.51.100.10",
        "--right-remote-underlay=203.0.113.20",
        "--pool-cidr=10.255.10.0/29",
        "--left-tunnel-ipv4-cidr=10.255.10.0/31",
        "--right-tunnel-ipv4-cidr=10.255.10.1/31",
        "--reserved=10.255.10.2",
        "--bandwidth-mbps=100",
        "--fou-port=6655",
        "--fou-peer-port=7755",
        "--fou-ipproto=47",
        "--enabled",
        "--confirmed",
    ])
    .unwrap();

    assert!(request.save);
    assert_eq!(
        request.update_plan_id.unwrap().to_string(),
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(request.expected_revision, Some(7));
    assert!(request.enabled);
    assert!(request.confirmed);
    assert_eq!(request.input.kind, TunnelKind::Fou);
    assert_eq!(request.input.bandwidth_mbps, 100);
    assert_eq!(request.input.runtime_control.fou.port, 6655);
    assert_eq!(request.input.runtime_control.fou.peer_port, 7755);
    assert_eq!(request.input.runtime_control.fou.ipproto, 47);
}

#[test]
fn parses_vty_tunnel_plan_explicit_dual_stack_endpoints() {
    let request = parse_vty_tunnel_plan(&[
        "--name=sea-fra",
        "--interface=vpsseafra",
        "--kind=wireguard",
        "--left-client=sea",
        "--right-client=fra",
        "--left-remote-underlay=198.51.100.10",
        "--right-remote-underlay=203.0.113.20",
        "--left-tunnel-ipv4-cidr=10.255.20.0/31",
        "--right-tunnel-ipv4-cidr=10.255.20.1/31",
        "--left-tunnel-ipv6-cidr=fd7a:115c:a1e0::20/127",
        "--right-tunnel-ipv6-cidr=fd7a:115c:a1e0::21/127",
        "--latency-primary-family=ipv6",
        "--bandwidth-mbps=1000",
        "--runtime-manager=observed",
    ])
    .unwrap();

    assert_eq!(request.input.address_pool_cidr, "");
    assert_eq!(
        request.input.ipv4_tunnel.as_ref().unwrap().left,
        "10.255.20.0"
    );
    assert_eq!(
        request.input.ipv6_tunnel.as_ref().unwrap().right,
        "fd7a:115c:a1e0::21"
    );
    assert_eq!(
        request.input.latency_primary_family,
        TunnelAddressFamily::Ipv6
    );
}

#[test]
fn parses_vty_tunnel_plan_external_adapter_runtime() {
    let request = parse_vty_tunnel_plan(&[
        "--name=external-openvpn",
        "--interface=ovpn42",
        "--kind=openvpn",
        "--left-client=left",
        "--right-client=right",
        "--left-remote-underlay=198.51.100.10",
        "--right-remote-underlay=203.0.113.20",
        "--pool-cidr=10.255.10.0/29",
        "--left-tunnel-ipv4-cidr=10.255.10.0/31",
        "--right-tunnel-ipv4-cidr=10.255.10.1/31",
        "--bandwidth-mbps=100",
        "--runtime-manager=adapter",
        "--left-runtime-adapter-definition-id=11111111-1111-4111-8111-111111111111",
        "--right-runtime-adapter-definition-id=22222222-2222-4222-8222-222222222222",
        "--traffic-egress-kbps=100000",
        "--traffic-burst-kb=4096",
    ])
    .unwrap();

    assert_eq!(request.input.kind, TunnelKind::Openvpn);
    assert_eq!(
        request.input.runtime_control.manager,
        RuntimeTunnelManager::ExternalManagedAdapter
    );
    assert_eq!(
        request
            .input
            .runtime_control
            .left_adapter_definition_id
            .as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );
    assert_eq!(
        request
            .input
            .runtime_control
            .right_adapter_definition_id
            .as_deref(),
        Some("22222222-2222-4222-8222-222222222222")
    );
    assert_eq!(
        request.input.runtime_control.traffic_limit.egress_kbps,
        Some(100_000)
    );
    assert!(request.input.runtime_topology.is_default());
}

#[test]
fn parses_vty_tunnel_plan_advanced_ospf_policy_without_losing_values() {
    let request = parse_vty_tunnel_plan(&[
        "--name=ospf-edge",
        "--interface=tunospf",
        "--kind=gre",
        "--left-client=left",
        "--right-client=right",
        "--left-remote-underlay=198.51.100.10",
        "--right-remote-underlay=203.0.113.20",
        "--left-tunnel-ipv4-cidr=10.255.30.0/31",
        "--right-tunnel-ipv4-cidr=10.255.30.1/31",
        "--bandwidth-mbps=750",
        "--ospf",
        "--ospf-mode=automatic",
        "--ospf-latency-ms=32.5",
        "--ospf-packet-loss-ratio=0.01",
        "--ospf-preference=1.4",
        "--ospf-min-cost-delta=9",
        "--ospf-healthy-windows=4",
        "--ospf-latency-weight=1.5",
        "--ospf-loss-weight=550",
        "--ospf-bandwidth-weight=12.5",
        "--ospf-preference-bias=0.8",
        "--ospf-min-cost=8",
        "--ospf-max-cost=64000",
        "--left-routing-adapter-definition-id=33333333-3333-4333-8333-333333333333",
        "--right-routing-adapter-definition-id=44444444-4444-4444-8444-444444444444",
    ])
    .unwrap();

    let ospf = request.input.ospf.unwrap();
    assert_eq!(ospf.min_cost_delta, 9);
    assert_eq!(ospf.healthy_windows, 4);
    assert_eq!(ospf.policy.latency_weight, 1.5);
    assert_eq!(ospf.policy.loss_weight, 550.0);
    assert_eq!(ospf.policy.bandwidth_weight, 12.5);
    assert_eq!(ospf.policy.preference_bias, 0.8);
    assert_eq!(ospf.policy.min_cost, 8);
    assert_eq!(ospf.policy.max_cost, 64_000);
}

#[test]
fn rejects_vty_tunnel_plan_missing_required_or_bad_values() {
    assert!(parse_vty_tunnel_plan(&["--name", "edge"]).is_err());
    assert!(parse_vty_tunnel_plan(&[
        "--name=edge",
        "--interface=vpsedge",
        "--kind=badkind",
        "--left-client=left",
        "--right-client=right",
        "--left-remote-underlay=198.51.100.10",
        "--right-remote-underlay=203.0.113.20",
        "--pool-cidr=10.255.10.0/29",
        "--bandwidth-mbps=100",
    ])
    .is_err());
    assert!(parse_vty_tunnel_plan(&[
        "--name=observed",
        "--interface=wg42",
        "--kind=wireguard",
        "--left-client=left",
        "--right-client=right",
        "--left-remote-underlay=198.51.100.10",
        "--right-remote-underlay=203.0.113.20",
        "--left-tunnel-ipv4-cidr=10.255.10.0/31",
        "--right-tunnel-ipv4-cidr=10.255.10.1/31",
        "--bandwidth-mbps=100",
        "--runtime-manager=observed",
        "--topology-stale=wg-old",
    ])
    .is_err());
    assert!(parse_vty_tunnel_plan(&[
        "--name=edge",
        "--interface=vpsedge",
        "--kind=gre",
        "--left-client=left",
        "--right-client=right",
        "--left-remote-underlay=198.51.100.10",
        "--right-remote-underlay=203.0.113.20",
        "--pool-cidr=10.255.10.0/29",
        "--bandwidth-mbps=1g",
    ])
    .is_err());
    assert!(parse_vty_tunnel_plan(&[
        "--name=edge",
        "--interface=vpsedge",
        "--kind=gre",
        "--left-client=left",
        "--right-client=right",
        "--left-remote-underlay=198.51.100.10",
        "--right-remote-underlay=203.0.113.20",
        "--left-tunnel-ipv6-cidr=fd7a:115c:a1e0::20/127",
        "--bandwidth-mbps=100",
    ])
    .is_err());
    assert!(parse_vty_tunnel_plan(&[
        "--name=edge",
        "--interface=vpsedge",
        "--kind=gre",
        "--left-client=left",
        "--right-client=right",
        "--left-remote-underlay=198.51.100.10",
        "--right-remote-underlay=203.0.113.20",
        "--pool-cidr=10.255.10.0/29",
        "--bandwidth-mbps=100",
    ])
    .is_err());
}
