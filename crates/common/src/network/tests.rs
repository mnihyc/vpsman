use super::*;

fn ipv4_pair(left: &str, right: &str) -> TunnelAddressPair {
    TunnelAddressPair {
        left: left.to_string(),
        right: right.to_string(),
        prefix_len: 31,
    }
}

#[test]
fn higher_bandwidth_is_preferred_when_latency_close() {
    let policy = OspfCostPolicy::default();
    let slow = ospf_cost(
        policy,
        TunnelObservation {
            latency_ms: 40.0,
            packet_loss_ratio: 0.0,
            bandwidth_mbps: 10,
            preference: 1.0,
        },
    );
    let fast = ospf_cost(
        policy,
        TunnelObservation {
            latency_ms: 40.0,
            packet_loss_ratio: 0.0,
            bandwidth_mbps: 1000,
            preference: 1.0,
        },
    );
    assert!(fast < slow);
}

#[test]
fn bandwidth_cost_curve_is_smooth_across_operator_range() {
    let policy = OspfCostPolicy::default();
    let cost_for = |bandwidth_mbps| {
        ospf_cost(
            policy,
            TunnelObservation {
                latency_ms: 20.0,
                packet_loss_ratio: 0.0,
                bandwidth_mbps,
                preference: 1.0,
            },
        )
    };
    let ten = cost_for(10);
    let twenty = cost_for(20);
    let fifty = cost_for(50);
    let hundred = cost_for(100);
    let two_hundred_fifty = cost_for(250);
    let five_hundred = cost_for(500);
    let thousand = cost_for(1000);
    let five_thousand = cost_for(5000);
    let ten_thousand = cost_for(10_000);

    assert_eq!(ten, 52);
    assert_eq!(twenty, 42);
    assert_eq!(fifty, 34);
    assert_eq!(hundred, 30);
    assert_eq!(two_hundred_fifty, 26);
    assert_eq!(five_hundred, 24);
    assert_eq!(thousand, 23);
    assert_eq!(five_thousand, 21);
    assert_eq!(ten_thousand, 21);
    assert!(ten > hundred);
    assert!(hundred > thousand);
    assert!(thousand >= ten_thousand);
    assert!(ten - hundred > thousand - ten_thousand);
}

#[test]
fn bandwidth_cost_curve_handles_non_preset_operator_values() {
    let policy = OspfCostPolicy::default();
    let cost_for = |bandwidth_mbps| {
        ospf_cost(
            policy,
            TunnelObservation {
                latency_ms: 20.0,
                packet_loss_ratio: 0.0,
                bandwidth_mbps,
                preference: 1.0,
            },
        )
    };

    assert_eq!(cost_for(123), 29);
    assert_eq!(cost_for(1234), 23);
    assert_eq!(cost_for(9876), 21);
    assert!(cost_for(10) - cost_for(100) > cost_for(100) - cost_for(1000));
    assert!(cost_for(100) - cost_for(1000) > cost_for(1000) - cost_for(10_000));
}

#[test]
fn bandwidth_cost_curve_has_no_legacy_tier_cliffs() {
    let policy = OspfCostPolicy::default();
    let cost_for = |bandwidth_mbps| {
        ospf_cost(
            policy,
            TunnelObservation {
                latency_ms: 20.0,
                packet_loss_ratio: 0.0,
                bandwidth_mbps,
                preference: 1.0,
            },
        )
    };

    for legacy_tier in [100_u32, 1000, 5000, 10_000] {
        let lower = legacy_tier.saturating_sub(1).max(MIN_TUNNEL_BANDWIDTH_MBPS);
        let upper = (legacy_tier + 1).min(MAX_TUNNEL_BANDWIDTH_MBPS);
        let costs: Vec<u16> = (lower..=upper).map(cost_for).collect();
        let min_cost = costs.iter().min().copied().expect("costs");
        let max_cost = costs.iter().max().copied().expect("costs");
        assert!(
            max_cost - min_cost <= 1,
            "legacy tier {legacy_tier} has a cost cliff across {lower}..={upper}: {costs:?}"
        );
    }
}

#[test]
fn bandwidth_cost_curve_is_monotonic_for_every_operator_mbps() {
    let policy = OspfCostPolicy::default();
    let cost_for = |bandwidth_mbps| {
        ospf_cost(
            policy,
            TunnelObservation {
                latency_ms: 20.0,
                packet_loss_ratio: 0.0,
                bandwidth_mbps,
                preference: 1.0,
            },
        )
    };

    let mut previous = cost_for(MIN_TUNNEL_BANDWIDTH_MBPS);
    for bandwidth_mbps in (MIN_TUNNEL_BANDWIDTH_MBPS + 1)..=MAX_TUNNEL_BANDWIDTH_MBPS {
        let current = cost_for(bandwidth_mbps);
        assert!(
            current <= previous,
            "cost increased from {previous} to {current} at {bandwidth_mbps} Mbps"
        );
        assert!(
            previous - current <= 2,
            "cost changed too abruptly from {previous} to {current} at {bandwidth_mbps} Mbps"
        );
        previous = current;
    }
}

#[test]
fn bandwidth_cost_clamps_to_operator_range_and_keeps_latency_primary() {
    let policy = OspfCostPolicy::default();
    let cost_for = |bandwidth_mbps, latency_ms| {
        ospf_cost(
            policy,
            TunnelObservation {
                latency_ms,
                packet_loss_ratio: 0.0,
                bandwidth_mbps,
                preference: 1.0,
            },
        )
    };

    assert_eq!(cost_for(1, 20.0), cost_for(10, 20.0));
    assert_eq!(cost_for(20_000, 20.0), cost_for(10_000, 20.0));
    assert!(cost_for(10_000, 70.0) > cost_for(10, 20.0));
}

#[test]
fn bandwidth_cost_curve_balances_bandwidth_latency_loss_and_preference() {
    let policy = OspfCostPolicy::default();
    let cost_for = |bandwidth_mbps, latency_ms, packet_loss_ratio, preference| {
        ospf_cost(
            policy,
            TunnelObservation {
                latency_ms,
                packet_loss_ratio,
                bandwidth_mbps,
                preference,
            },
        )
    };
    let baseline_low_bandwidth = cost_for(10, 20.0, 0.0, 1.0);

    assert!(cost_for(10_000, 70.0, 0.0, 1.0) > baseline_low_bandwidth);
    assert!(cost_for(10_000, 20.0, 0.1, 1.0) > baseline_low_bandwidth);
    assert!(cost_for(100, 20.0, 0.0, 1.2) < cost_for(10_000, 20.0, 0.0, 0.8));
}

#[test]
fn bandwidth_cost_curve_keeps_bandwidth_advantage_bounded() {
    let policy = OspfCostPolicy::default();
    let cost_for = |bandwidth_mbps, latency_ms, packet_loss_ratio, preference| {
        ospf_cost(
            policy,
            TunnelObservation {
                latency_ms,
                packet_loss_ratio,
                bandwidth_mbps,
                preference,
            },
        )
    };

    let slow_healthy = cost_for(10, 20.0, 0.0, 1.0);
    let fast_healthy = cost_for(10_000, 20.0, 0.0, 1.0);
    let full_bandwidth_advantage = slow_healthy - fast_healthy;

    assert_eq!(full_bandwidth_advantage, 31);
    assert!(
        cost_for(
            10_000,
            20.0 + f64::from(full_bandwidth_advantage) + 1.0,
            0.0,
            1.0,
        ) > slow_healthy
    );
    assert!(
        cost_for(
            10_000,
            20.0,
            (f64::from(full_bandwidth_advantage) + 1.0) / policy.loss_weight,
            1.0,
        ) > slow_healthy
    );
}

#[test]
fn bandwidth_cost_curve_stays_secondary_to_path_health() {
    let policy = OspfCostPolicy::default();
    let cost_for = |bandwidth_mbps, latency_ms, packet_loss_ratio, preference| {
        ospf_cost(
            policy,
            TunnelObservation {
                latency_ms,
                packet_loss_ratio,
                bandwidth_mbps,
                preference,
            },
        )
    };

    for latency_ms in [5.0, 20.0, 80.0, 180.0] {
        assert!(
            cost_for(10_000, latency_ms + 32.0, 0.0, 1.0) > cost_for(10, latency_ms, 0.0, 1.0),
            "bandwidth overpowered latency at {latency_ms} ms"
        );
        assert!(
            cost_for(10_000, latency_ms, 0.08, 1.0) > cost_for(10, latency_ms, 0.0, 1.0),
            "bandwidth overpowered packet loss at {latency_ms} ms"
        );
    }

    assert!(cost_for(10_000, 20.0, 0.0, 0.8) > cost_for(10_000, 20.0, 0.0, 1.2));
}

#[test]
fn ospf_cost_sanitizes_non_finite_inputs_before_clamping() {
    let policy = OspfCostPolicy::default();
    let safe_zero_observation = TunnelObservation {
        latency_ms: 0.0,
        packet_loss_ratio: 0.0,
        bandwidth_mbps: 100,
        preference: 1.0,
    };
    let unsafe_observation = TunnelObservation {
        latency_ms: f64::NAN,
        packet_loss_ratio: f64::INFINITY,
        bandwidth_mbps: 100,
        preference: f64::NAN,
    };

    assert_eq!(
        ospf_cost(policy, unsafe_observation),
        ospf_cost(policy, safe_zero_observation)
    );

    let mut policy_with_bad_weights = policy;
    policy_with_bad_weights.latency_weight = f64::NAN;
    policy_with_bad_weights.loss_weight = f64::INFINITY;
    policy_with_bad_weights.bandwidth_weight = f64::NAN;
    policy_with_bad_weights.preference_bias = f64::NAN;
    assert_eq!(
        ospf_cost(policy_with_bad_weights, safe_zero_observation),
        ospf_cost(policy, safe_zero_observation)
    );
}

#[test]
fn ospf_cost_orders_policy_bounds_before_clamping() {
    let mut policy = OspfCostPolicy::default();
    policy.min_cost = 100;
    policy.max_cost = 20;

    assert_eq!(
        ospf_cost(
            policy,
            TunnelObservation {
                latency_ms: 20.0,
                packet_loss_ratio: 0.0,
                bandwidth_mbps: 100,
                preference: 1.0,
            },
        ),
        30
    );
}

#[test]
fn observed_ospf_cost_downgrades_bandwidth_when_speed_is_below_burst() {
    let policy = OspfCostPolicy::default();
    let (healthy_cost, healthy_bandwidth) =
        observed_ospf_cost(policy, 1000, 20.0, 0.0, 1.0, Some(950.0));
    let (degraded_cost, degraded_bandwidth) =
        observed_ospf_cost(policy, 1000, 20.0, 0.0, 1.0, Some(40.0));

    assert_eq!(healthy_bandwidth, 950);
    assert_eq!(degraded_bandwidth, 40);
    assert!(degraded_cost > healthy_cost);
}

#[test]
fn observed_ospf_cost_never_exceeds_configured_bandwidth_burst() {
    let (_cost, effective_bandwidth) =
        observed_ospf_cost(OspfCostPolicy::default(), 10, 20.0, 0.0, 1.0, Some(950.0));

    assert_eq!(effective_bandwidth, 10);
}

#[test]
fn observed_ospf_cost_effective_bandwidth_uses_operator_range() {
    let (_cost, low_effective_bandwidth) =
        observed_ospf_cost(OspfCostPolicy::default(), 1, 20.0, 0.0, 1.0, Some(5.0));
    let (_cost, high_effective_bandwidth) = observed_ospf_cost(
        OspfCostPolicy::default(),
        20_000,
        20.0,
        0.0,
        1.0,
        Some(12_500.0),
    );

    assert_eq!(low_effective_bandwidth, 10);
    assert_eq!(high_effective_bandwidth, 10_000);
}

#[test]
fn parses_legacy_bird_ptp_peers() {
    let parsed = parse_legacy_bird_config(
        "router id 192.0.2.1;",
        r#"
        protocol ospf v3 ospflax2 {
          area 0 {
            interface "wgpeerhk" {
              type ptp;
              cost 42;
            };
            interface "lo" {
              cost 1;
            };
          };
        }
        "#,
    );

    assert_eq!(parsed.router_id.as_deref(), Some("192.0.2.1"));
    assert_eq!(parsed.node_name.as_deref(), Some("ospflax2"));
    assert_eq!(parsed.peers.len(), 1);
    assert_eq!(parsed.peers[0].protocol_name, "ospflax2");
    assert_eq!(parsed.peers[0].interface_name, "wgpeerhk");
    assert_eq!(parsed.peers[0].peer_name.as_deref(), Some("hk"));
    assert_eq!(parsed.peers[0].cost, Some(42));
}

#[test]
fn parses_ifupdown_managed_tunnel_snippets() {
    let parsed = parse_ifupdown_configs(&[
        (
            "/etc/network/interfaces",
            r#"
            source /etc/network/interfaces.d/*
            auto lo
            iface lo inet loopback
            "#,
        ),
        (
            "/etc/network/interfaces.d/tunnels",
            r#"
            auto ypeerhk
            iface ypeerhk inet static
              address 10.255.0.0
              pointopoint 10.255.0.1
              pre-up ip tunnel add $IFACE mode gre remote 203.0.113.2 local 198.51.100.2 ttl 255
            "#,
        ),
    ]);

    let tunnel = parsed
        .interfaces
        .iter()
        .find(|interface| interface.name == "ypeerhk")
        .expect("tunnel interface");
    assert_eq!(tunnel.source_path, "/etc/network/interfaces.d/tunnels");
    assert_eq!(tunnel.address.as_deref(), Some("10.255.0.0"));
    assert_eq!(tunnel.point_to_point.as_deref(), Some("10.255.0.1"));
    assert_eq!(tunnel.tunnel_kind, Some(TunnelKind::Gre));
    assert_eq!(tunnel.tunnel_local.as_deref(), Some("198.51.100.2"));
    assert_eq!(tunnel.tunnel_remote.as_deref(), Some("203.0.113.2"));
}

#[test]
fn renders_safe_tunnel_plan_without_mutation() {
    let plan = plan_tunnel(&TunnelPlanInput {
        name: "lax-hkg".to_string(),
        interface_name: "vpnlaxhkg".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "lax-edge-01".to_string(),
        right_client_id: "hkg-edge-01".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(ipv4_pair("10.255.0.0", "10.255.0.1")),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 1000,
        latency_ms: 138.0,
        packet_loss_ratio: 0.002,
        preference: 1.2,
        ospf_policy: OspfCostPolicy::default(),
    })
    .expect("plan");

    assert!(!plan.mutates_host);
    assert_eq!(
        plan.runtime_control.manager,
        RuntimeTunnelManager::AgentIproute2Managed
    );
    assert_eq!(plan.left_tunnel_address, "10.255.0.0");
    assert_eq!(plan.right_tunnel_address, "10.255.0.1");
    assert_eq!(plan.tunnel_prefix_len, 31);
    assert!(plan.ifupdown_snippet.contains("mode gre"));
    assert!(plan.ifupdown_snippet.contains("remote 203.0.113.20"));
    assert!(plan.bird2_interface_snippet.contains("type ptp;"));
    assert!(plan.bird2_interface_snippet.contains("cost "));
    assert_eq!(
        plan.touched_files,
        vec![
            "/etc/network/interfaces.d/vpsman-tunnels".to_string(),
            "/etc/bird/vpsman-ospf.conf".to_string()
        ]
    );
}

#[test]
fn plans_explicit_dual_stack_tunnel_addresses() {
    let plan = plan_tunnel(&TunnelPlanInput {
        name: "dual-stack".to_string(),
        interface_name: "tun6".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "edge-a".to_string(),
        right_client_id: "edge-b".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: String::new(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.255.10.0".to_string(),
            right: "10.255.10.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: Some(TunnelAddressPair {
            left: "fd00:10::0".to_string(),
            right: "fd00:10::1".to_string(),
            prefix_len: 127,
        }),
        latency_primary_family: TunnelAddressFamily::Ipv6,
        bandwidth_mbps: 1000,
        latency_ms: 20.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .expect("plan");

    assert_eq!(plan.latency_primary_family, TunnelAddressFamily::Ipv6);
    assert_eq!(plan.left_tunnel_address, "fd00:10::0");
    assert_eq!(plan.right_tunnel_address, "fd00:10::1");
    assert_eq!(plan.tunnel_prefix_len, 127);
    assert!(plan.ifupdown_snippet.contains("iface tun6 inet static"));
    assert!(plan.ifupdown_snippet.contains("iface tun6 inet6 static"));
    assert!(plan.ifupdown_snippet.contains("address 10.255.10.0"));
    assert!(plan.ifupdown_snippet.contains("address fd00:10::0"));
}

#[test]
fn allocates_endpoint_suggestions_without_planning_side_effects() {
    let allocation = allocate_tunnel_endpoints(
        Some("10.255.30.0/29"),
        Some("fd00:30::/126"),
        &["10.255.30.0".to_string(), "10.255.30.1".to_string()],
        true,
        true,
    )
    .expect("allocation");

    assert_eq!(
        allocation.ipv4_tunnel,
        Some(ipv4_pair("10.255.30.2", "10.255.30.3"))
    );
    assert_eq!(
        allocation.ipv6_tunnel,
        Some(TunnelAddressPair {
            left: "fd00:30::".to_string(),
            right: "fd00:30::1".to_string(),
            prefix_len: 127,
        })
    );
    assert_eq!(allocation.latency_primary_family, TunnelAddressFamily::Ipv4);
}

#[test]
fn rejects_tunnel_plan_without_any_endpoint_addresses() {
    assert_eq!(
        plan_tunnel(&TunnelPlanInput {
            name: "empty".to_string(),
            interface_name: "tunempty".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "edge-a".to_string(),
            right_client_id: "edge-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: String::new(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: None,
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth_mbps: 100,
            latency_ms: 20.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        }),
        Err(NetworkPlanError::TunnelAddressRequired)
    );
    assert_eq!(
        plan_tunnel(&TunnelPlanInput {
            name: "pool-only".to_string(),
            interface_name: "tunpool".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "edge-a".to_string(),
            right_client_id: "edge-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.99.0/30".to_string(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: None,
            ipv6_address_pool_cidr: Some("fd00:99::/127".to_string()),
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth_mbps: 100,
            latency_ms: 20.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        }),
        Err(NetworkPlanError::TunnelAddressRequired)
    );
}

#[test]
fn plans_external_observed_tunnel_without_ifupdown_mutation() {
    let plan = plan_tunnel(&TunnelPlanInput {
        name: "wg-import".to_string(),
        interface_name: "wg42".to_string(),
        kind: TunnelKind::Wireguard,
        runtime_control: RuntimeTunnelControl {
            manager: RuntimeTunnelManager::ExternalObserved,
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: RuntimeTunnelTopologyIntent {
            version: Some("provider-a:42".to_string()),
            desired_interfaces: vec!["wg42".to_string()],
            ..RuntimeTunnelTopologyIntent::default()
        },
        left_client_id: "lax-edge-01".to_string(),
        right_client_id: "hkg-edge-01".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(ipv4_pair("10.255.0.0", "10.255.0.1")),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        latency_ms: 80.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .expect("plan");

    assert_eq!(
        plan.runtime_control.manager,
        RuntimeTunnelManager::ExternalObserved
    );
    assert!(plan.ifupdown_snippet.contains("external observed"));
    assert!(!plan.ifupdown_snippet.contains("ip tunnel add"));
    assert_eq!(plan.touched_files, vec![MANAGED_BIRD2_FILE.to_string()]);
    assert!(plan
        .validation_steps
        .iter()
        .any(|step| step.contains("external interface exists")));
}

#[test]
fn plans_external_managed_adapter_tunnel_with_commands() {
    let plan = plan_tunnel(&TunnelPlanInput {
        name: "openvpn-import".to_string(),
        interface_name: "ovpn42".to_string(),
        kind: TunnelKind::Openvpn,
        runtime_control: RuntimeTunnelControl {
            manager: RuntimeTunnelManager::ExternalManagedAdapter,
            startup: Some(RuntimeTunnelCommand {
                argv: vec![
                    "/usr/local/libexec/vpsman-openvpn-adapter".to_string(),
                    "start".to_string(),
                    "{interface}".to_string(),
                ],
                max_timeout_secs: 20,
                max_output_bytes: 8192,
            }),
            status: Some(RuntimeTunnelCommand {
                argv: vec![
                    "/usr/local/libexec/vpsman-openvpn-adapter".to_string(),
                    "status".to_string(),
                    "{interface}".to_string(),
                ],
                max_timeout_secs: 10,
                max_output_bytes: 4096,
            }),
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: RuntimeTunnelTopologyIntent {
            desired_interfaces: vec!["ovpn42".to_string()],
            routes: vec![RuntimeTunnelRoute {
                destination_cidr: "10.42.0.0/24".to_string(),
                interface_name: Some("ovpn42".to_string()),
                metric: Some(42),
                ..RuntimeTunnelRoute::default()
            }],
            ..RuntimeTunnelTopologyIntent::default()
        },
        left_client_id: "lax-edge-01".to_string(),
        right_client_id: "hkg-edge-01".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(ipv4_pair("10.255.0.0", "10.255.0.1")),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 1000,
        latency_ms: 80.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .expect("plan");

    assert_eq!(
        plan.runtime_control.manager,
        RuntimeTunnelManager::ExternalManagedAdapter
    );
    assert!(plan.ifupdown_snippet.contains("custom adapter"));
    assert_eq!(plan.touched_files, vec![MANAGED_BIRD2_FILE.to_string()]);
    assert!(plan
        .validation_steps
        .iter()
        .any(|step| step.contains("adapter status/start")));
}

#[test]
fn rejects_custom_kind_without_external_runtime_manager() {
    assert_eq!(
        plan_tunnel(&TunnelPlanInput {
            name: "custom-bad".to_string(),
            interface_name: "cust42".to_string(),
            kind: TunnelKind::Custom,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "lax-edge-01".to_string(),
            right_client_id: "hkg-edge-01".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: None,
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth_mbps: 100,
            latency_ms: 80.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        }),
        Err(NetworkPlanError::UnsupportedBackendTunnelKind)
    );
}

#[test]
fn validates_external_managed_runtime_tunnel_controls() {
    let control = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalManagedAdapter,
        startup: Some(RuntimeTunnelCommand {
            argv: vec![
                "/usr/local/libexec/vpsman-openvpn-adapter".to_string(),
                "start".to_string(),
                "{interface}".to_string(),
            ],
            max_timeout_secs: 20,
            max_output_bytes: 8192,
        }),
        restart: Some(RuntimeTunnelCommand {
            argv: vec![
                "/usr/local/libexec/vpsman-openvpn-adapter".to_string(),
                "restart".to_string(),
                "{interface}".to_string(),
            ],
            max_timeout_secs: 20,
            max_output_bytes: 8192,
        }),
        cleanup: Some(RuntimeTunnelCommand {
            argv: vec![
                "/usr/local/libexec/vpsman-openvpn-adapter".to_string(),
                "cleanup".to_string(),
                "{interface}".to_string(),
            ],
            max_timeout_secs: 20,
            max_output_bytes: 8192,
        }),
        status: Some(RuntimeTunnelCommand {
            argv: vec![
                "/usr/local/libexec/vpsman-openvpn-adapter".to_string(),
                "status".to_string(),
                "{interface}".to_string(),
            ],
            max_timeout_secs: 5,
            max_output_bytes: 4096,
        }),
        traffic_limit_apply: Some(RuntimeTunnelCommand {
            argv: vec![
                "/usr/local/libexec/vpsman-openvpn-adapter".to_string(),
                "shape".to_string(),
                "{interface}".to_string(),
            ],
            max_timeout_secs: 10,
            max_output_bytes: 4096,
        }),
        traffic_limit: RuntimeTunnelTrafficLimit {
            ingress_kbps: Some(100_000),
            egress_kbps: Some(100_000),
            burst_kb: Some(4096),
        },
        ..RuntimeTunnelControl::default()
    };

    validate_runtime_tunnel_control(&control).unwrap();
    assert_eq!(control.cleanup.as_ref().unwrap().argv[1], "cleanup");
}

#[test]
fn rejects_unbounded_or_mutating_runtime_tunnel_controls() {
    let observed = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalObserved,
        restart: Some(RuntimeTunnelCommand {
            argv: vec!["/usr/local/bin/restart-tun".to_string()],
            max_timeout_secs: 10,
            max_output_bytes: 4096,
        }),
        ..RuntimeTunnelControl::default()
    };
    assert_eq!(
        validate_runtime_tunnel_control(&observed),
        Err(NetworkPlanError::RuntimeTunnelObservedCannotMutate)
    );

    let relative_command = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalManagedAdapter,
        startup: Some(RuntimeTunnelCommand {
            argv: vec![
                "openvpn".to_string(),
                "--config".to_string(),
                "edge.conf".to_string(),
            ],
            max_timeout_secs: 10,
            max_output_bytes: 4096,
        }),
        ..RuntimeTunnelControl::default()
    };
    assert_eq!(
        validate_runtime_tunnel_control(&relative_command),
        Err(NetworkPlanError::InvalidRuntimeTunnelCommand)
    );

    let unbounded_traffic = RuntimeTunnelControl {
        traffic_limit: RuntimeTunnelTrafficLimit {
            ingress_kbps: Some(1),
            egress_kbps: None,
            burst_kb: None,
        },
        ..RuntimeTunnelControl::default()
    };
    assert_eq!(
        validate_runtime_tunnel_control(&unbounded_traffic),
        Err(NetworkPlanError::InvalidRuntimeTunnelTrafficLimit)
    );
}

#[test]
fn validates_runtime_topology_intent_routes_and_stale_interfaces() {
    let topology = RuntimeTunnelTopologyIntent {
        version: Some("provider-a:42".to_string()),
        desired_interfaces: vec!["tun42".to_string()],
        stale_interfaces: vec!["old42".to_string()],
        routes: vec![RuntimeTunnelRoute {
            destination_cidr: "10.42.0.0/24".to_string(),
            via: Some("10.255.0.1".to_string()),
            interface_name: None,
            metric: Some(50),
        }],
        stale_routes: vec![RuntimeTunnelRoute {
            destination_cidr: "10.41.0.0/24".to_string(),
            interface_name: Some("old42".to_string()),
            ..RuntimeTunnelRoute::default()
        }],
    };

    validate_runtime_topology_intent(&topology, "tun42").unwrap();

    let mut missing_current = topology.clone();
    missing_current.desired_interfaces = vec!["tun43".to_string()];
    assert_eq!(
        validate_runtime_topology_intent(&missing_current, "tun42"),
        Err(NetworkPlanError::InvalidRuntimeTunnelTopology)
    );

    let mut stale_current = topology.clone();
    stale_current.stale_interfaces = vec!["tun42".to_string()];
    assert_eq!(
        validate_runtime_topology_intent(&stale_current, "tun42"),
        Err(NetworkPlanError::InvalidRuntimeTunnelTopology)
    );

    let mut invalid_route = topology;
    invalid_route.routes[0].destination_cidr = "not-cidr".to_string();
    assert_eq!(
        validate_runtime_topology_intent(&invalid_route, "tun42"),
        Err(NetworkPlanError::InvalidRuntimeTunnelRoute)
    );
}

#[test]
fn renders_side_specific_runtime_tunnel_snippets() {
    let plan = plan_tunnel(&TunnelPlanInput {
        name: "lax-hkg".to_string(),
        interface_name: "tun42".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "lax".to_string(),
        right_client_id: "hkg".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(ipv4_pair("10.255.0.0", "10.255.0.1")),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        latency_ms: 50.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .unwrap();

    let left = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let right = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Right).unwrap();

    assert_eq!(left.local_client_id, "lax");
    assert_eq!(right.local_client_id, "hkg");
    assert!(left.ifupdown_snippet.contains("address 10.255.0.0"));
    assert!(left.ifupdown_snippet.contains("pointopoint 10.255.0.1"));
    assert!(left
        .ifupdown_snippet
        .contains("remote 203.0.113.20 local 198.51.100.10"));
    assert!(right.ifupdown_snippet.contains("address 10.255.0.1"));
    assert!(right.ifupdown_snippet.contains("pointopoint 10.255.0.0"));
    assert!(right
        .ifupdown_snippet
        .contains("remote 198.51.100.10 local 203.0.113.20"));
    assert!(left.bird2_interface_snippet.contains("lax -> hkg"));
    assert!(right.bird2_interface_snippet.contains("hkg -> lax"));
}

#[test]
fn renders_all_initial_tunnel_kinds() {
    for (kind, expected) in [
        (TunnelKind::Gre, "mode gre"),
        (TunnelKind::Ipip, "mode ipip"),
        (TunnelKind::Sit, "mode sit"),
        (TunnelKind::Fou, "encap fou"),
    ] {
        let plan = plan_tunnel(&TunnelPlanInput {
            name: format!("{kind:?}"),
            interface_name: format!("vps{:?}", kind).to_lowercase(),
            kind,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "left".to_string(),
            right_client_id: "right".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.10.0/29".to_string(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: Some(ipv4_pair("10.255.10.0", "10.255.10.1")),
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth_mbps: 100,
            latency_ms: 20.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .expect("plan");

        assert!(plan.ifupdown_snippet.contains(expected));
    }
}

#[test]
fn renders_custom_fou_runtime_options_without_hardcoded_ports() {
    let runtime_control = RuntimeTunnelControl {
        fou: RuntimeTunnelFouOptions {
            port: 6655,
            peer_port: 7755,
            ipproto: 47,
        },
        ..RuntimeTunnelControl::default()
    };
    let mut input = TunnelPlanInput {
        name: "custom-fou".to_string(),
        interface_name: "fou42".to_string(),
        kind: TunnelKind::Fou,
        runtime_control,
        runtime_topology: Default::default(),
        left_client_id: "left".to_string(),
        right_client_id: "right".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.20.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(ipv4_pair("10.255.20.0", "10.255.20.1")),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        latency_ms: 20.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    };
    let plan = plan_tunnel(&input).unwrap();

    assert!(plan
        .ifupdown_snippet
        .contains("ip fou add port 6655 ipproto 47"));
    assert!(plan.ifupdown_snippet.contains("encap-dport 7755"));
    assert!(plan.ifupdown_snippet.contains("ip fou del port 6655"));

    let networkd = render_tunnel_endpoint_backend_config(
        &plan,
        TunnelEndpointSide::Left,
        TunnelConfigBackend::SystemdNetworkd,
    )
    .unwrap();
    assert!(networkd.files[0].contents.contains("Port=6655"));
    assert!(networkd.files[0].contents.contains("PeerPort=7755"));
    assert!(networkd.files[0].contents.contains("Protocol=47"));

    input.kind = TunnelKind::Gre;
    assert_eq!(
        plan_tunnel(&input),
        Err(NetworkPlanError::InvalidRuntimeTunnelCommand)
    );
}

#[test]
fn renders_backend_specific_tunnel_files_and_signature_payload() {
    let plan = plan_tunnel(&TunnelPlanInput {
        name: "lax-hkg".to_string(),
        interface_name: "tun42".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "lax".to_string(),
        right_client_id: "hkg".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(ipv4_pair("10.255.0.0", "10.255.0.1")),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        latency_ms: 50.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .unwrap();

    let netplan = render_tunnel_endpoint_backend_config(
        &plan,
        TunnelEndpointSide::Right,
        TunnelConfigBackend::Netplan,
    )
    .unwrap();
    assert_eq!(netplan.files[0].managed_path, MANAGED_NETPLAN_FILE);
    assert!(netplan.files[0].contents.contains("mode: gre"));
    assert!(netplan.files[0].contents.contains("local: 203.0.113.20"));
    assert!(netplan.files[0].contents.contains("remote: 198.51.100.10"));
    assert!(netplan.files[0].contents.contains("10.255.0.1/31"));
    assert!(!backend_config_signature_payload(&netplan).is_empty());

    let networkd = render_tunnel_endpoint_backend_config(
        &plan,
        TunnelEndpointSide::Left,
        TunnelConfigBackend::SystemdNetworkd,
    )
    .unwrap();
    assert_eq!(networkd.files.len(), 2);
    assert_eq!(
        networkd.files[0].managed_path,
        MANAGED_SYSTEMD_NETWORKD_NETDEV_FILE
    );
    assert_eq!(
        networkd.files[1].managed_path,
        MANAGED_SYSTEMD_NETWORKD_NETWORK_FILE
    );
    assert!(networkd.files[0].contents.contains("Kind=gre"));
    assert!(networkd.files[1].contents.contains("Address=10.255.0.0/31"));
}

#[test]
fn rejects_conflicting_or_invalid_tunnel_plans() {
    assert_eq!(
        plan_tunnel(&TunnelPlanInput {
            name: "bad".to_string(),
            interface_name: "interface-name-too-long".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "left".to_string(),
            right_client_id: "right".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: None,
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth_mbps: 10,
            latency_ms: 10.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        }),
        Err(NetworkPlanError::InvalidInterfaceName)
    );

    assert_eq!(
        allocate_tunnel_endpoints(
            Some("10.255.0.0/31"),
            None,
            &["10.255.0.0".to_string(), "10.255.0.1".to_string()],
            true,
            false
        ),
        Err(NetworkPlanError::AddressPoolExhausted)
    );
}
