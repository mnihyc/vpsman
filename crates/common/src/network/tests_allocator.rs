use super::*;

fn pair(left: &str, right: &str, prefix_len: u8) -> TunnelAddressPair {
    TunnelAddressPair {
        left: left.into(),
        right: right.into(),
        prefix_len,
    }
}

fn reservations(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn plan(ipv4: Option<TunnelAddressPair>, ipv6: Option<TunnelAddressPair>) -> TunnelPlan {
    serde_json::from_value(serde_json::json!({
        "name": "allocation-test", "interface_name": "tun-test", "kind": "gre",
        "left_client_id": "left", "right_client_id": "right",
        "left_remote_underlay": "192.0.2.1", "right_remote_underlay": "192.0.2.2",
        "left_tunnel_address": "10.0.0.1", "right_tunnel_address": "10.0.0.2",
        "tunnel_prefix_len": 24, "ipv4_tunnel": ipv4, "ipv6_tunnel": ipv6,
        "bandwidth_mbps": 1000, "conflicts": []
    }))
    .unwrap()
}

#[test]
fn requested_masks_allocate_first_free_aligned_subnets_not_unused_hosts() {
    let allocation = allocate_tunnel_endpoints_with_options(
        Some("10.0.0.77/16"),
        Some("fd00::8/120"),
        &reservations(&["10.0.0.99/25", "10.0.1.254", "fd00::8/125", "fd00::1f"]),
        TunnelEndpointAllocationOptions {
            include_ipv6: true,
            ipv4_prefix_len: 24,
            ipv6_prefix_len: 124,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        allocation.ipv4_tunnel,
        Some(pair("10.0.2.1", "10.0.2.2", 24))
    );
    assert_eq!(
        allocation.ipv6_tunnel,
        Some(pair("fd00::20", "fd00::21", 124))
    );
    assert_eq!(allocation.latency_primary_family, TunnelAddressFamily::Ipv4);
}

#[test]
fn retained_pairs_preserve_exact_hosts_masks_and_work_without_pools() {
    let v4 = pair("192.0.2.70", "192.0.2.71", 24);
    let v6 = pair("fd99::de", "fd99::df", 124);
    for (pool4, pool6) in [
        (Some("10.0.0.0/24"), Some("fd00::/64")),
        (None, None),
        (Some("invalid unused pool"), Some("also unused")),
    ] {
        let allocation = allocate_tunnel_endpoints_with_options(
            pool4,
            pool6,
            &reservations(&["10.0.0.0/8", "fd00::/64"]),
            TunnelEndpointAllocationOptions {
                include_ipv6: true,
                ipv4_prefix_len: 24,
                ipv6_prefix_len: 124,
                preferred_ipv4: Some(&v4),
                preferred_ipv6: Some(&v6),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(allocation.ipv4_tunnel, Some(v4.clone()));
        assert_eq!(allocation.ipv6_tunnel, Some(v6.clone()));
    }
}

#[test]
fn retained_subnets_conflict_globally_even_outside_the_chosen_pool() {
    for (ipv4, preferred, reserved) in [
        (true, pair("192.0.2.70", "192.0.2.71", 24), "192.0.2.200/30"),
        (false, pair("fd99::de", "fd99::df", 124), "fd99::d1/128"),
    ] {
        let error = allocate_tunnel_endpoints_with_options(
            Some("10.0.0.0/16"),
            Some("fd00::/64"),
            &reservations(&[reserved]),
            TunnelEndpointAllocationOptions {
                include_ipv4: ipv4,
                include_ipv6: !ipv4,
                ipv4_prefix_len: 24,
                ipv6_prefix_len: 124,
                preferred_ipv4: ipv4.then_some(&preferred),
                preferred_ipv6: (!ipv4).then_some(&preferred),
            },
        )
        .unwrap_err();
        assert_eq!(error, NetworkPlanError::TunnelAddressConflict);
    }
}

#[test]
fn invalid_preferred_pairs_are_errors_not_silent_fresh_allocations() {
    for preferred in [
        pair("invalid", "", 24),
        pair("", "invalid", 24),
        pair("10.1.2.1", "10.1.2.1", 24),
        pair("10.1.2.1", "10.1.3.2", 24),
        pair("fd00::1", "fd00::2", 24),
        pair("10.1.2.1", "10.1.2.2", 25),
    ] {
        assert_eq!(
            allocate_tunnel_endpoints_with_options(
                Some("10.0.0.0/8"),
                None,
                &[],
                TunnelEndpointAllocationOptions {
                    ipv4_prefix_len: 24,
                    preferred_ipv4: Some(&preferred),
                    ..Default::default()
                },
            ),
            Err(NetworkPlanError::InvalidPreferredTunnelAddress),
            "{preferred:?}",
        );
    }
    for preferred in [
        pair("invalid", "", 124),
        pair("", "10.0.0.1", 124),
        pair("fd00::1", "fd00::11", 124),
        pair("fd00::1", "fd00::1", 124),
        pair("10.0.0.1", "10.0.0.2", 124),
    ] {
        assert_eq!(
            allocate_tunnel_endpoints_with_options(
                None,
                Some("fd00::/64"),
                &[],
                TunnelEndpointAllocationOptions {
                    include_ipv4: false,
                    include_ipv6: true,
                    ipv6_prefix_len: 124,
                    preferred_ipv6: Some(&preferred),
                    ..Default::default()
                },
            ),
            Err(NetworkPlanError::InvalidPreferredTunnelAddress),
        );
    }
}

#[test]
fn missing_endpoint_uses_anchored_subnet_and_preserves_other_field_exactly() {
    for (ipv4, preferred, expected) in [
        (
            true,
            pair("192.0.2.70", "", 24),
            pair("192.0.2.70", "192.0.2.1", 24),
        ),
        (
            true,
            pair("", "192.0.2.70", 24),
            pair("192.0.2.1", "192.0.2.70", 24),
        ),
        (
            true,
            pair("192.0.2.1", "", 24),
            pair("192.0.2.1", "192.0.2.2", 24),
        ),
        (
            true,
            pair("", "192.0.2.1", 24),
            pair("192.0.2.2", "192.0.2.1", 24),
        ),
        (
            false,
            pair("fd99::de", "", 124),
            pair("fd99::de", "fd99::d0", 124),
        ),
        (
            false,
            pair("", "FD99::DE", 124),
            pair("fd99::d0", "FD99::DE", 124),
        ),
        (
            false,
            pair("fd99::d0", "", 124),
            pair("fd99::d0", "fd99::d1", 124),
        ),
        (
            false,
            pair("", "fd99::d0", 124),
            pair("fd99::d1", "fd99::d0", 124),
        ),
    ] {
        for (pool4, pool6) in [(None, None), (Some("10.0.0.0/8"), Some("fd00::/64"))] {
            let result = allocate_tunnel_endpoints_with_options(
                pool4,
                pool6,
                &[],
                TunnelEndpointAllocationOptions {
                    include_ipv4: ipv4,
                    include_ipv6: !ipv4,
                    ipv4_prefix_len: 24,
                    ipv6_prefix_len: 124,
                    preferred_ipv4: ipv4.then_some(&preferred),
                    preferred_ipv6: (!ipv4).then_some(&preferred),
                },
            )
            .unwrap();
            assert_eq!(
                if ipv4 {
                    result.ipv4_tunnel
                } else {
                    result.ipv6_tunnel
                },
                Some(expected.clone())
            );
        }
    }
}

#[test]
fn both_empty_endpoints_allocate_fresh_using_selected_mask() {
    let blank4 = pair("", " ", 24);
    let blank6 = pair(" ", "", 124);
    let allocation = allocate_tunnel_endpoints_with_options(
        Some("10.0.0.0/8"),
        Some("fd00::/64"),
        &[],
        TunnelEndpointAllocationOptions {
            include_ipv6: true,
            ipv4_prefix_len: 24,
            ipv6_prefix_len: 124,
            preferred_ipv4: Some(&blank4),
            preferred_ipv6: Some(&blank6),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        allocation.ipv4_tunnel,
        Some(pair("10.0.0.1", "10.0.0.2", 24))
    );
    assert_eq!(allocation.ipv6_tunnel, Some(pair("fd00::", "fd00::1", 124)));
}

#[test]
fn partial_endpoints_at_numeric_boundaries_fill_without_overflow() {
    for (ipv4, preferred, expected) in [
        (
            true,
            pair("255.255.255.255", "", 31),
            pair("255.255.255.255", "255.255.255.254", 31),
        ),
        (
            true,
            pair("", "255.255.255.254", 31),
            pair("255.255.255.255", "255.255.255.254", 31),
        ),
        (true, pair("0.0.0.1", "", 0), pair("0.0.0.1", "0.0.0.2", 0)),
        (
            false,
            pair("", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", 127),
            pair(
                "ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe",
                "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
                127,
            ),
        ),
        (
            false,
            pair("ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe", "", 127),
            pair(
                "ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe",
                "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
                127,
            ),
        ),
        (false, pair("::", "", 0), pair("::", "::1", 0)),
    ] {
        let result = allocate_tunnel_endpoints_with_options(
            None,
            None,
            &[],
            TunnelEndpointAllocationOptions {
                include_ipv4: ipv4,
                include_ipv6: !ipv4,
                ipv4_prefix_len: preferred.prefix_len,
                ipv6_prefix_len: preferred.prefix_len,
                preferred_ipv4: ipv4.then_some(&preferred),
                preferred_ipv6: (!ipv4).then_some(&preferred),
            },
        )
        .unwrap();
        assert_eq!(
            if ipv4 {
                result.ipv4_tunnel
            } else {
                result.ipv6_tunnel
            },
            Some(expected)
        );
    }
}

#[test]
fn explicit_prefix_validation_and_pool_size_errors_are_family_specific() {
    for (ipv4, prefix) in [(true, 32), (true, 255), (false, 128), (false, 255)] {
        assert_eq!(
            allocate_tunnel_endpoints_with_options(
                Some("0.0.0.0/0"),
                Some("::/0"),
                &[],
                TunnelEndpointAllocationOptions {
                    include_ipv4: ipv4,
                    include_ipv6: !ipv4,
                    ipv4_prefix_len: prefix,
                    ipv6_prefix_len: prefix,
                    ..Default::default()
                },
            ),
            Err(NetworkPlanError::InvalidTunnelPrefix),
        );
    }
    for (ipv4, pool4, pool6) in [
        (true, Some("10.0.0.0/25"), None),
        (false, None, Some("fd00::/125")),
    ] {
        assert_eq!(
            allocate_tunnel_endpoints_with_options(
                pool4,
                pool6,
                &[],
                TunnelEndpointAllocationOptions {
                    include_ipv4: ipv4,
                    include_ipv6: !ipv4,
                    ipv4_prefix_len: 24,
                    ipv6_prefix_len: 124,
                    ..Default::default()
                },
            ),
            Err(NetworkPlanError::AddressPoolTooSmall),
        );
    }
    assert_eq!(
        allocate_tunnel_endpoints(None, None, &[], true, false),
        Err(NetworkPlanError::AddressPoolRequired),
    );
    assert_eq!(
        allocate_tunnel_endpoints(None, None, &[], false, false),
        Err(NetworkPlanError::TunnelAddressRequired),
    );
    assert_eq!(
        allocate_tunnel_endpoints(Some("fd00::/64"), None, &[], true, false),
        Err(NetworkPlanError::InvalidCidr),
    );
    assert_eq!(
        allocate_tunnel_endpoints(
            Some("10.0.0.0/24"),
            None,
            &reservations(&["invalid"]),
            true,
            false
        ),
        Err(NetworkPlanError::InvalidReservedAddress("invalid".into())),
    );
}

#[test]
fn broad_reservations_skip_enormous_ranges_in_one_jump() {
    let allocation = allocate_tunnel_endpoints_with_options(
        Some("0.0.0.0/0"),
        Some("::/0"),
        &reservations(&["0.0.0.0/1", "::/1"]),
        TunnelEndpointAllocationOptions {
            include_ipv6: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        allocation.ipv4_tunnel,
        Some(pair("128.0.0.0", "128.0.0.1", 31))
    );
    assert_eq!(allocation.ipv6_tunnel, Some(pair("8000::", "8000::1", 127)));
}

#[test]
fn full_address_space_subnets_and_numeric_maxima_do_not_overflow() {
    let allocation = allocate_tunnel_endpoints_with_options(
        Some("0.0.0.0/0"),
        Some("::/0"),
        &[],
        TunnelEndpointAllocationOptions {
            include_ipv6: true,
            ipv4_prefix_len: 0,
            ipv6_prefix_len: 0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(allocation.ipv4_tunnel, Some(pair("0.0.0.1", "0.0.0.2", 0)));
    assert_eq!(allocation.ipv6_tunnel, Some(pair("::", "::1", 0)));

    let allocation = allocate_tunnel_endpoints(
        Some("255.255.255.254/31"),
        Some("ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe/127"),
        &[],
        true,
        true,
    )
    .unwrap();
    assert_eq!(
        allocation.ipv4_tunnel,
        Some(pair("255.255.255.254", "255.255.255.255", 31))
    );
    assert_eq!(
        allocation.ipv6_tunnel,
        Some(pair(
            "ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe",
            "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            127,
        ))
    );
    for (ipv4, reserved) in [
        (true, "255.255.255.255"),
        (false, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"),
    ] {
        assert_eq!(
            allocate_tunnel_endpoints(
                Some("255.255.255.254/31"),
                Some("ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe/127"),
                &reservations(&[reserved]),
                ipv4,
                !ipv4,
            ),
            Err(NetworkPlanError::AddressPoolExhausted),
        );
        assert_eq!(
            allocate_tunnel_endpoints_with_options(
                Some("0.0.0.0/0"),
                Some("::/0"),
                &reservations(&[reserved]),
                TunnelEndpointAllocationOptions {
                    include_ipv4: ipv4,
                    include_ipv6: !ipv4,
                    ipv4_prefix_len: 0,
                    ipv6_prefix_len: 0,
                    ..Default::default()
                },
            ),
            Err(NetworkPlanError::AddressPoolExhausted),
        );
    }
}

#[test]
fn containing_overlapping_adjacent_and_outside_ranges_are_handled() {
    let allocation = allocate_tunnel_endpoints(
        Some("10.0.1.0/24"),
        None,
        &reservations(&[
            "10.0.0.0/24",
            "10.0.1.0/26",
            "10.0.1.32/27",
            "10.0.1.64/26",
            "10.0.1.128/27",
            "10.0.2.0/24",
        ]),
        true,
        false,
    )
    .unwrap();
    assert_eq!(
        allocation.ipv4_tunnel,
        Some(pair("10.0.1.160", "10.0.1.161", 31))
    );
    for (ipv4, reserved) in [(true, "10.0.0.0/8"), (false, "fd00::/8")] {
        assert_eq!(
            allocate_tunnel_endpoints(
                Some("10.0.1.0/24"),
                Some("fd00:1::/64"),
                &reservations(&[reserved]),
                ipv4,
                !ipv4,
            ),
            Err(NetworkPlanError::AddressPoolExhausted),
        );
    }
}

#[test]
fn interval_jumps_match_exhaustive_first_fit_for_every_small_occupancy_pattern() {
    // Exhaust every reservation combination in an eight-address pool. Checking
    // /31-, /30-, and /29-sized blocks exercises gaps at every alignment boundary.
    for occupancy in 0_u16..256 {
        let occupied = (0_u128..8)
            .filter(|index| occupancy & (1 << index) != 0)
            .map(|index| (index, index))
            .collect::<Vec<_>>();
        for block_size in [2_u128, 4, 8] {
            let expected = (0_u128..8)
                .step_by(block_size as usize)
                .find(|start| {
                    (*start..*start + block_size).all(|index| occupancy & (1 << index) == 0)
                })
                .ok_or(NetworkPlanError::AddressPoolExhausted);
            assert_eq!(
                first_free_subnet(0, 7, block_size - 1, &occupied),
                expected,
                "occupancy={occupancy:08b}, block_size={block_size}",
            );
        }
    }
}

#[test]
fn global_network_helpers_share_canonical_subnet_and_link_local_rules() {
    let v4 = pair("10.0.0.1", "10.0.0.2", 24);
    let v6 = pair("fd00::11", "fd00::12", 124);
    let mut plan = plan(Some(v4), Some(v6));
    plan.additional_addresses.left.ipv4 = reservations(&["10.0.0.90/24", "192.0.2.8/32"]);
    plan.additional_addresses.right.ipv4 = reservations(&["10.0.0.91/24"]);
    plan.additional_addresses.left.ipv6 = reservations(&["fe80::1/64", "fd00::2f/124"]);
    plan.additional_addresses.right.ipv6 = reservations(&["fe80::2/64"]);
    let networks = tunnel_plan_global_networks(&plan).unwrap();
    assert_eq!(
        networks.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["10.0.0.0/24", "fd00::10/124"],
    );
    assert!(tunnel_networks_overlap(
        &networks[0],
        &"10.0.0.128/25".parse().unwrap()
    ));
    assert!(!tunnel_networks_overlap(
        &networks[0],
        &"10.0.1.0/24".parse().unwrap()
    ));
    assert!(!tunnel_networks_overlap(
        &networks[0],
        &"::/0".parse().unwrap()
    ));

    // Additional addresses are not allocator declarations, even with broad masks.
    plan.additional_addresses.left.ipv6 = reservations(&["fe80::1/0"]);
    assert_eq!(tunnel_plan_global_networks(&plan).unwrap(), networks);
    plan.ipv4_tunnel = None;
    plan.ipv6_tunnel = Some(pair("fe80::1", "fe80::2", 64));
    assert!(tunnel_plan_global_networks(&plan).unwrap().is_empty());
    plan.ipv6_tunnel = Some(pair("fe80::1", "fe80::2", 0));
    assert_eq!(
        tunnel_plan_global_networks(&plan).unwrap(),
        ["::/0".parse::<IpNet>().unwrap()]
    );
    plan.ipv6_tunnel = Some(pair("fe80::1", "fe80::2", 9));
    assert_eq!(
        tunnel_plan_global_networks(&plan).unwrap(),
        ["fe80::/9".parse::<IpNet>().unwrap()]
    );
}

#[test]
fn explicit_link_local_reservations_are_honored_for_fresh_and_retained_endpoints() {
    let allocation = allocate_tunnel_endpoints(
        None,
        Some("fe80::/64"),
        &reservations(&["fe80::", "fe80::1"]),
        false,
        true,
    )
    .unwrap();
    assert_eq!(
        allocation.ipv6_tunnel,
        Some(pair("fe80::2", "fe80::3", 127))
    );
    assert_eq!(
        allocate_tunnel_endpoints(
            None,
            Some("fe80::/64"),
            &reservations(&["fe80::/64"]),
            false,
            true
        ),
        Err(NetworkPlanError::AddressPoolExhausted)
    );
    for preferred in [
        pair("fe80::", "fe80::1", 127),
        pair("fe80::", "", 127),
        pair("", "fe80::1", 127),
    ] {
        assert_eq!(
            allocate_tunnel_endpoints_with_options(
                None,
                None,
                &reservations(&["fe80::1"]),
                TunnelEndpointAllocationOptions {
                    include_ipv4: false,
                    include_ipv6: true,
                    preferred_ipv6: Some(&preferred),
                    ..Default::default()
                }
            ),
            Err(NetworkPlanError::TunnelAddressConflict)
        );
    }
    assert_eq!(
        allocate_tunnel_endpoints(
            None,
            Some("fd00::/64"),
            &reservations(&["fe80::1/0"]),
            false,
            true,
        ),
        Err(NetworkPlanError::AddressPoolExhausted),
    );
}

#[test]
fn saved_link_local_main_pairs_do_not_reserve_other_interfaces() {
    let saved = plan(None, Some(pair("fe80::", "fe80::1", 127)));
    let reserved = tunnel_plan_global_networks(&saved)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let result =
        allocate_tunnel_endpoints(None, Some("fe80::/64"), &reserved, false, true).unwrap();
    assert_eq!(result.ipv6_tunnel, saved.ipv6_tunnel);
}
