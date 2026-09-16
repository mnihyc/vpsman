use super::*;

#[path = "tests_fou.rs"]
mod fou;

const TEST_PLAN_ID: uuid::Uuid = uuid::Uuid::from_u128(0x11111111_1111_4111_8111_111111111111);

const LEFT_RUNTIME_ADAPTER: &str = "11111111-1111-4111-8111-111111111111";
const RIGHT_RUNTIME_ADAPTER: &str = "22222222-2222-4222-8222-222222222222";
const LEFT_ROUTING_ADAPTER: &str = "33333333-3333-4333-8333-333333333333";
const RIGHT_ROUTING_ADAPTER: &str = "44444444-4444-4444-8444-444444444444";

fn ipv4_pair(left: &str, right: &str) -> TunnelAddressPair {
    TunnelAddressPair {
        left: left.to_string(),
        right: right.to_string(),
        prefix_len: 31,
    }
}

fn plan_input(kind: TunnelKind, manager: RuntimeTunnelManager) -> TunnelPlanInput {
    let runtime_control = RuntimeTunnelControl {
        manager,
        left_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
            .then(|| LEFT_RUNTIME_ADAPTER.to_string()),
        right_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
            .then(|| RIGHT_RUNTIME_ADAPTER.to_string()),
        ..RuntimeTunnelControl::default()
    };
    TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "tunab".to_string(),
        kind,
        runtime_control,
        runtime_topology: RuntimeTunnelTopologyIntent::default(),
        left_client_id: "edge-a".to_string(),
        right_client_id: "edge-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.255.0.0/24".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(ipv4_pair("10.255.0.0", "10.255.0.1")),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        additional_addresses: Default::default(),
        manage_link_local: true,
        latency_primary_family: TunnelAddressFamily::Ipv4,
        bandwidth_mbps: 1234,
        dynamic_bandwidth: false,
        left_mtu: (manager == RuntimeTunnelManager::AgentBuiltin)
            .then(|| default_tunnel_mtu(kind))
            .flatten(),
        right_mtu: (manager == RuntimeTunnelManager::AgentBuiltin)
            .then(|| default_tunnel_mtu(kind))
            .flatten(),
        ospf: None,
    }
}

#[test]
fn advanced_defaults_leave_existing_plan_wire_shape_unchanged() {
    let original = plan_input(TunnelKind::Openvpn, RuntimeTunnelManager::AgentBuiltin);
    let original_json = serde_json::to_value(&original).unwrap();
    let mut with_empty_fields = original_json.clone();
    with_empty_fields["runtime_control"] = serde_json::json!({
        "manager": "agent_builtin",
        "hooks": { "left": {}, "right": {} },
        "openvpn": { "left_config_override": " \n\t", "right_config_override": null }
    });
    let restored: TunnelPlanInput = serde_json::from_value(with_empty_fields).unwrap();
    assert_eq!(restored, original);
    assert_eq!(serde_json::to_value(&restored).unwrap(), original_json);
}

#[test]
fn advanced_endpoint_settings_survive_plan_and_endpoint_rendering() {
    let mut input = plan_input(TunnelKind::Openvpn, RuntimeTunnelManager::AgentBuiltin);
    input.runtime_control.openvpn.left_config_override = Some("verb 5\nping 20\n".into());
    input.runtime_control.openvpn.right_config_override = Some("verb 6\n".into());
    let hook = RuntimeTunnelCommand {
        argv: vec!["/usr/bin/logger".into(), "{interface}".into()],
        ..Default::default()
    };
    input.runtime_control.hooks.left.pre_start = Some(hook.clone());
    input.runtime_control.hooks.right.post_shutdown = Some(hook);
    let plan = plan_tunnel(&input).unwrap();
    let roundtrip: TunnelPlan =
        serde_json::from_value(serde_json::to_value(&plan).unwrap()).unwrap();
    assert_eq!(roundtrip, plan);
    for side in [TunnelEndpointSide::Left, TunnelEndpointSide::Right] {
        let endpoint = render_tunnel_endpoint_config(&plan, side).unwrap();
        assert_eq!(endpoint.runtime_control, input.runtime_control);
    }
    let preview = render_tunnel_runtime_preview(&plan, Some(TEST_PLAN_ID)).unwrap();
    assert_eq!(preview.endpoints.len(), 2);
    assert!(preview.endpoints[0].artifacts[0]
        .content
        .contains("verb 5\nping 20\n"));
    assert!(preview.endpoints[1].artifacts[0]
        .content
        .contains("verb 6\n"));
    assert_eq!(
        preview.endpoints[0].commands[0].argv,
        ["/usr/bin/logger", "tunab"]
    );
    assert_eq!(
        preview.endpoints[1].commands.last().unwrap().phase,
        "post_shutdown"
    );
}

#[test]
fn openvpn_overrides_replace_defaults_and_keep_native_option_order() {
    let generated = "dev tun0\nping 10\nverb 3\nauth SHA256\n";
    let text = "# tuning remains native\n--verb 6\nsetenv opt ping 25\nremote-random\nsetenv a first\nsetenv b second\nunknown-future-option anything\n";
    let merged = merge_openvpn_config_override(generated, Some(text)).unwrap();
    assert_eq!(merged, format!("dev tun0\nauth SHA256\n{text}"));
    assert_eq!(
        merge_openvpn_config_override(generated, Some("  \n")).unwrap(),
        generated
    );
    // Invalid native values are not silently changed or rejected by a tuning allowlist.
    assert!(validate_openvpn_config_override("ping not-a-number\ncipher future-cipher\ntun-mtu-extra 32\nlink-mtu 1500\nlog /operator/openvpn.log\nscript-security 2\nmanagement /operator/management.sock unix\n").is_ok());
}

#[test]
fn openvpn_overrides_cannot_replace_native_owned_resources_indirectly() {
    for text in [
        "dev other0",
        "--dev other0",
        "\"dev\" other0",
        "d\\ev other0",
        "setenv opt dev other0",
        "--setenv opt --dev other0",
        "config /tmp/other.conf",
        "<connection>\nremote 192.0.2.1\n</connection>",
        "<ca>\nother certificate\n</ca>",
        "writepid /tmp/other.pid",
        "up /tmp/script",
        "route 10.0.0.0 255.255.255.0",
    ] {
        assert!(
            matches!(
                validate_openvpn_config_override(text),
                Err(NetworkPlanError::OpenvpnDirectiveOwned(_))
            ),
            "{text}"
        );
    }
}

#[test]
fn lifecycle_hook_templates_preserve_argv_and_do_not_recurse() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.name = "literal-{interface}".into();
    let plan = plan_tunnel(&input).unwrap();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Right).unwrap();
    let command = RuntimeTunnelCommand {
        argv: [
            "/usr/bin/logger",
            "name={plan}",
            "interface {interface}",
            "{unknown}",
            "{local_ipv4}/{prefix_len_ipv4}",
            "{local_ipv6}",
        ]
        .map(str::to_string)
        .to_vec(),
        ..Default::default()
    };
    assert_eq!(
        render_runtime_tunnel_command(&command, &plan, &endpoint, &[]).unwrap(),
        [
            "/usr/bin/logger",
            "name=literal-{interface}",
            "interface tunab",
            "{unknown}",
            "10.255.0.1/31",
            ""
        ]
    );
}

#[test]
fn builtin_advanced_options_cannot_silently_activate_for_other_managers() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::ExternalObserved);
    input.runtime_control.hooks.left.pre_start = Some(RuntimeTunnelCommand {
        argv: vec!["/bin/true".into()],
        ..Default::default()
    });
    assert!(plan_tunnel(&input).is_err());
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.runtime_control.openvpn.left_config_override = Some("verb 5".into());
    assert!(plan_tunnel(&input).is_err());
}

#[test]
fn native_address_commands_attach_prefix_to_peer_for_both_families_and_sides() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.ipv6_tunnel = Some(TunnelAddressPair {
        left: "fd00::".to_string(),
        right: "fd00::1".to_string(),
        prefix_len: 127,
    });
    let plan = plan_tunnel(&input).unwrap();
    let base = vec!["/sbin/ip".to_string()];
    for (side, expected) in [
        (
            TunnelEndpointSide::Left,
            [("10.255.0.0", "10.255.0.1/31"), ("fd00::", "fd00::1/127")],
        ),
        (
            TunnelEndpointSide::Right,
            [("10.255.0.1", "10.255.0.0/31"), ("fd00::1", "fd00::/127")],
        ),
    ] {
        let endpoint = render_tunnel_endpoint_config(&plan, side).unwrap();
        let commands = build_tunnel_address_argv(&base, &plan, &endpoint, Some(TEST_PLAN_ID));
        assert_eq!(commands.len(), expected.len() + 1);
        assert_eq!(
            commands.last().unwrap()[3],
            tunnel_generated_link_local(TEST_PLAN_ID, &endpoint.local_client_id)
        );
        for (argv, (local, peer)) in commands.into_iter().zip(expected) {
            assert_eq!(
                argv,
                ["/sbin/ip", "addr", "replace", local, "peer", peer, "dev", "tunab"]
            );
        }
    }
}

#[test]
fn additional_addresses_are_independent_and_keep_primary_targets_unchanged() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.additional_addresses.left.ipv4 = vec![" 10.20.1.99/24 ".into()];
    input.additional_addresses.left.ipv6 = vec!["FD00:AB::1234/64".into(), "fe80::a/64".into()];
    input.additional_addresses.right.ipv6 = vec!["fe80::b/64".into()];
    let plan = plan_tunnel(&input).unwrap();
    assert_eq!(plan.additional_addresses.left.ipv4, ["10.20.1.99/24"]);
    assert_eq!(
        plan.additional_addresses.left.ipv6,
        ["fd00:ab::1234/64", "fe80::a/64"]
    );
    assert_eq!(plan.left_tunnel_address, "10.255.0.0");
    assert_eq!(plan.right_tunnel_address, "10.255.0.1");
    assert_eq!(plan.latency_primary_family, TunnelAddressFamily::Ipv4);
    assert!(plan.ipv6_tunnel.is_none());
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Right).unwrap();
    assert_eq!(
        endpoint.additional_addresses,
        plan.additional_addresses.right
    );
    assert_eq!(
        endpoint.peer_additional_addresses,
        plan.additional_addresses.left
    );
    assert_eq!(
        tunnel_endpoint_explicit_address_cidrs(&plan, &endpoint),
        ["10.255.0.1/31", "fe80::b/64"]
    );
}

#[test]
fn additional_addresses_reject_wrong_family_and_same_link_duplicates() {
    for (ipv4, entries) in [
        (true, vec!["fd00::1/64"]),
        (false, vec!["10.0.0.1/32"]),
        (true, vec!["10.0.0.1/33"]),
        (false, vec!["fe80::1/129"]),
        (false, vec!["fe80::1"]),
        (true, vec!["10.255.0.0/32"]),
        (false, vec!["fe80::1/64", "FE80:0:0:0:0:0:0:1/128"]),
    ] {
        let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
        let list = if ipv4 {
            &mut input.additional_addresses.left.ipv4
        } else {
            &mut input.additional_addresses.left.ipv6
        };
        *list = entries.iter().map(|entry| (*entry).to_string()).collect();
        assert!(
            matches!(
                plan_tunnel(&input),
                Err(NetworkPlanError::InvalidAdditionalAddress(_))
            ),
            "{entries:?}"
        );
    }
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.additional_addresses.left.ipv6 = vec!["fe80::1/64".into()];
    input.additional_addresses.right.ipv6 = vec!["fe80::1/128".into()];
    assert!(matches!(
        plan_tunnel(&input),
        Err(NetworkPlanError::InvalidAdditionalAddress(_))
    ));
    input.additional_addresses.right.ipv6 = vec!["fe80::2/64".into()];
    input.reserved_addresses = vec!["fe80::1".into()];
    assert!(
        plan_tunnel(&input).is_ok(),
        "another link may reuse a link-local address"
    );
}

#[test]
fn additional_ipv6_mtu_requirement_is_endpoint_scoped() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.additional_addresses.left.ipv6 = vec!["fd00::1/64".into()];
    input.left_mtu = Some(1280);
    input.right_mtu = Some(1000);
    assert!(plan_tunnel(&input).is_ok());
    input.left_mtu = Some(1279);
    assert_eq!(plan_tunnel(&input), Err(NetworkPlanError::InvalidTunnelMtu));
}

#[test]
fn every_builtin_kind_renders_additional_addresses_without_extra_openvpn_ifconfig() {
    for kind in [
        TunnelKind::Gre,
        TunnelKind::Ipip,
        TunnelKind::Sit,
        TunnelKind::Fou,
        TunnelKind::Wireguard,
        TunnelKind::Openvpn,
    ] {
        let mut input = plan_input(kind, RuntimeTunnelManager::AgentBuiltin);
        input.additional_addresses.left.ipv4 = vec!["10.20.1.99/24".into()];
        input.additional_addresses.right.ipv6 = vec!["fd00:ab::1234/64".into()];
        let plan = plan_tunnel(&input).unwrap();
        let preview = render_tunnel_runtime_preview(&plan, Some(TEST_PLAN_ID)).unwrap();
        for (side, expected) in [
            (TunnelEndpointSide::Left, "10.20.1.99/24"),
            (TunnelEndpointSide::Right, "fd00:ab::1234/64"),
        ] {
            let endpoint = render_tunnel_endpoint_config(&plan, side).unwrap();
            let commands =
                build_tunnel_address_argv(&["ip".into()], &plan, &endpoint, Some(TEST_PLAN_ID));
            assert!(
                commands
                    .iter()
                    .any(|command| command == &["ip", "addr", "replace", expected, "dev", "tunab"]),
                "{kind:?} {side:?}"
            );
            let endpoint_preview = preview
                .endpoints
                .iter()
                .find(|entry| entry.side == side)
                .unwrap();
            assert!(endpoint_preview
                .commands
                .iter()
                .any(|command| command.argv.get(3).is_some_and(|value| value == expected)));
            if kind == TunnelKind::Openvpn {
                for artifact in &endpoint_preview.artifacts {
                    if artifact.label.starts_with("OpenVPN") {
                        assert_eq!(
                            artifact
                                .content
                                .lines()
                                .filter(|line| line.starts_with("ifconfig "))
                                .count(),
                            1
                        );
                        assert!(!artifact
                            .content
                            .lines()
                            .any(|line| line.starts_with("ifconfig-ipv6 ")));
                    }
                }
            }
        }
    }
}

#[test]
fn wireguard_additional_family_permissions_are_peer_host_scoped() {
    let mut input = plan_input(TunnelKind::Wireguard, RuntimeTunnelManager::AgentBuiltin);
    input.additional_addresses.left.ipv6 = vec!["fd00::a/64".into()];
    input.additional_addresses.right.ipv6 = vec!["fd00::b/64".into(), "fe80::b/64".into()];
    let plan = plan_tunnel(&input).unwrap();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let command = build_wireguard_configure_argv(
        &["wg".into()],
        &plan,
        &endpoint,
        std::path::Path::new("key"),
        "peer",
        Some(TEST_PLAN_ID),
    )
    .unwrap();
    assert_eq!(
        command.last().unwrap(),
        &format!(
            "0.0.0.0/0,fd00::b/128,fe80::b/128,{}/128",
            tunnel_generated_link_local(TEST_PLAN_ID, &endpoint.peer_client_id)
                .trim_end_matches("/64")
        )
    );
}

#[test]
fn managed_link_local_is_stable_additive_and_only_uses_explicit_ipv6_intent() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    let plan = plan_tunnel(&input).unwrap();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    assert!(!tunnel_endpoint_manages_link_local(&plan, &endpoint));
    input.additional_addresses.left.ipv6 = vec!["fe80::1234/64".into()];
    let plan = plan_tunnel(&input).unwrap();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    assert!(tunnel_endpoint_manages_link_local(&plan, &endpoint));
    let generated = tunnel_generated_link_local(TEST_PLAN_ID, &endpoint.local_client_id);
    assert_ne!(
        generated,
        tunnel_generated_link_local(uuid::Uuid::from_u128(2), &endpoint.local_client_id),
        "the same endpoint has distinct identities on different tunnel plans"
    );
    let address: std::net::Ipv6Addr = generated.trim_end_matches("/64").parse().unwrap();
    assert!(address.is_unicast_link_local());
    assert_eq!(address.octets()[8] & 0x02, 0);
    assert_ne!(
        generated,
        tunnel_generated_link_local(TEST_PLAN_ID, &endpoint.peer_client_id)
    );
    let commands = build_tunnel_address_argv(&["ip".into()], &plan, &endpoint, Some(TEST_PLAN_ID));
    assert!(commands
        .iter()
        .any(|command| command.get(3) == Some(&generated)));
    assert!(commands
        .iter()
        .any(|command| command.get(3).is_some_and(|value| value == "fe80::1234/64")));
    let mut renamed = plan.clone();
    renamed.name = "new display name".into();
    renamed.interface_name = "new0".into();
    renamed.kind = TunnelKind::Wireguard;
    let renamed_endpoint =
        render_tunnel_endpoint_config(&renamed, TunnelEndpointSide::Left).unwrap();
    assert_eq!(
        generated,
        tunnel_generated_link_local(TEST_PLAN_ID, &renamed_endpoint.local_client_id)
    );
    renamed.manage_link_local = false;
    assert!(!tunnel_endpoint_manages_link_local(
        &renamed,
        &renamed_endpoint
    ));
    let commands = build_tunnel_address_argv(
        &["ip".into()],
        &renamed,
        &renamed_endpoint,
        Some(TEST_PLAN_ID),
    );
    assert!(!commands
        .iter()
        .any(|command| command.get(3) == Some(&generated)));
    assert!(commands
        .iter()
        .any(|command| command.get(3).is_some_and(|value| value == "fe80::1234/64")));
    renamed.manage_link_local = true;
    renamed.runtime_control.manager = RuntimeTunnelManager::ExternalObserved;
    assert!(!tunnel_endpoint_manages_link_local(
        &renamed,
        &renamed_endpoint
    ));
}

#[test]
fn generated_link_local_already_listed_explicitly_is_not_applied_twice() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    let generated = tunnel_generated_link_local(TEST_PLAN_ID, &input.left_client_id);
    input.additional_addresses.left.ipv6 = vec![generated.clone()];
    let plan = plan_tunnel(&input).unwrap();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let commands = build_tunnel_address_argv(&["ip".into()], &plan, &endpoint, Some(TEST_PLAN_ID));
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.get(3) == Some(&generated))
            .count(),
        1
    );
}

#[test]
fn explicit_ipv6_cannot_duplicate_the_peers_active_generated_link_local() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    let generated_left = tunnel_generated_link_local(TEST_PLAN_ID, &input.left_client_id);
    input.additional_addresses.left.ipv6 = vec!["fd00::a/64".into()];
    input.additional_addresses.right.ipv6 = vec![generated_left.clone()];
    let plan = plan_tunnel(&input).unwrap();
    assert!(matches!(
        validate_tunnel_link_local_addresses(TEST_PLAN_ID, &plan),
        Err(NetworkPlanError::InvalidAdditionalAddress(_))
    ));
    assert!(validate_tunnel_link_local_addresses(uuid::Uuid::from_u128(2), &plan).is_ok());
    assert!(
        render_tunnel_runtime_preview(&plan, None).is_ok(),
        "an unsaved draft cannot invent its future plan UUID"
    );
    assert!(render_tunnel_runtime_preview(&plan, Some(TEST_PLAN_ID)).is_err());
    input.additional_addresses.left.ipv6.clear();
    assert!(
        validate_tunnel_link_local_addresses(TEST_PLAN_ID, &plan_tunnel(&input).unwrap()).is_ok(),
        "left has no IPv6 intent, so its automatic address is inactive"
    );
    input.ipv6_tunnel = Some(TunnelAddressPair {
        left: "fe80::a".into(),
        right: generated_left.trim_end_matches("/64").into(),
        prefix_len: 64,
    });
    input.additional_addresses.right.ipv6.clear();
    assert!(matches!(
        validate_tunnel_link_local_addresses(TEST_PLAN_ID, &plan_tunnel(&input).unwrap()),
        Err(NetworkPlanError::InvalidAdditionalAddress(_))
    ));
    input.manage_link_local = false;
    assert!(
        validate_tunnel_link_local_addresses(TEST_PLAN_ID, &plan_tunnel(&input).unwrap()).is_ok(),
        "native mode has no generated address claim"
    );
    input.manage_link_local = true;
    input.ipv6_tunnel.as_mut().unwrap().left = generated_left.trim_end_matches("/64").into();
    input.ipv6_tunnel.as_mut().unwrap().right = "fe80::b".into();
    assert!(
        validate_tunnel_link_local_addresses(TEST_PLAN_ID, &plan_tunnel(&input).unwrap()).is_ok(),
        "explicit own address coalesces with the generated address"
    );
}

#[test]
fn draft_link_local_preview_is_placeholder_while_saved_preview_matches_runtime() {
    let mut input = plan_input(TunnelKind::Wireguard, RuntimeTunnelManager::AgentBuiltin);
    input.additional_addresses.left.ipv6 = vec!["fd00::a/64".into()];
    input.additional_addresses.right.ipv6 = vec!["fd00::b/64".into()];
    let plan = plan_tunnel(&input).unwrap();
    let draft = render_tunnel_runtime_preview(&plan, None).unwrap();
    let saved = render_tunnel_runtime_preview(&plan, Some(TEST_PLAN_ID)).unwrap();
    let base = ["{runtime_ip_argv}".to_string()];
    for (draft_endpoint, saved_endpoint) in draft.endpoints.iter().zip(&saved.endpoints) {
        let endpoint = render_tunnel_endpoint_config(&plan, saved_endpoint.side).unwrap();
        assert!(draft_endpoint.commands.iter().any(|command| command
            .argv
            .contains(&"{generated_link_local}/64".to_string())));
        assert!(draft_endpoint.commands.iter().any(|command| command
            .argv
            .iter()
            .any(|arg| arg.contains("{generated_peer_link_local}/128"))));
        assert!(!saved_endpoint
            .commands
            .iter()
            .any(|command| command.argv.iter().any(|arg| arg.contains("{generated_"))));
        for command in build_tunnel_address_argv(&base, &plan, &endpoint, Some(TEST_PLAN_ID)) {
            assert!(saved_endpoint
                .commands
                .iter()
                .any(|rendered| rendered.argv == command));
        }
        let peer = tunnel_generated_link_local(TEST_PLAN_ID, &endpoint.peer_client_id);
        let peer_allowance = format!("{}/128", peer.trim_end_matches("/64"));
        assert!(saved_endpoint
            .commands
            .iter()
            .any(|command| command.argv.iter().any(|arg| arg.contains(&peer_allowance))));
    }
}

#[test]
fn address_management_defaults_preserve_serialized_default_plan_shape() {
    let input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    let value = serde_json::to_value(&input).unwrap();
    assert!(value.get("additional_addresses").is_none());
    assert!(value.get("manage_link_local").is_none());
    let decoded: TunnelPlanInput = serde_json::from_value(value).unwrap();
    assert!(decoded.manage_link_local);
    assert!(decoded.additional_addresses.is_empty());
    let mut plan = plan_tunnel(&decoded).unwrap();
    plan.manage_link_local = false;
    plan.additional_addresses.left.ipv4 = vec!["10.0.0.2/32".into()];
    let roundtrip: TunnelPlan =
        serde_json::from_value(serde_json::to_value(&plan).unwrap()).unwrap();
    assert_eq!(plan, roundtrip);
}

#[test]
fn fou_native_commands_keep_endpoint_intent_with_link_encapsulation() {
    let mut input = plan_input(TunnelKind::Fou, RuntimeTunnelManager::AgentBuiltin);
    input.left_local_underlay = Some("192.0.2.1".into());
    input.runtime_control.fou.peer_port = 15555;
    input.runtime_control.fou.tunnel_kind = RuntimeTunnelFouKind::Ipip;
    let plan = plan_tunnel(&input).unwrap();
    let base = ["/sbin/ip".into()];
    for (side, remote, local) in [
        (TunnelEndpointSide::Left, "198.51.100.10", Some("192.0.2.1")),
        (TunnelEndpointSide::Right, "203.0.113.20", None),
    ] {
        let endpoint = render_tunnel_endpoint_config(&plan, side).unwrap();
        let mut expected = vec![
            "/sbin/ip", "link", "add", "tunab", "type", "ipip", "remote", remote,
        ];
        if let Some(local) = local {
            expected.extend(["local", local]);
        }
        expected.extend([
            "ttl",
            "255",
            "encap",
            "fou",
            "encap-sport",
            "auto",
            "encap-dport",
            "15555",
        ]);
        assert_eq!(
            build_ip_tunnel_argv(&base, "add", &plan, &endpoint).unwrap(),
            expected
        );
    }
}

#[test]
fn preview_and_native_renderers_use_the_same_endpoint_options() {
    for kind in [
        TunnelKind::Gre,
        TunnelKind::Ipip,
        TunnelKind::Sit,
        TunnelKind::Fou,
        TunnelKind::Wireguard,
        TunnelKind::Openvpn,
    ] {
        let plan = plan_tunnel(&plan_input(kind, RuntimeTunnelManager::AgentBuiltin)).unwrap();
        let preview = render_tunnel_runtime_preview(&plan, Some(TEST_PLAN_ID)).unwrap();
        for rendered in preview.endpoints {
            let endpoint = render_tunnel_endpoint_config(&plan, rendered.side).unwrap();
            let base = ["{runtime_ip_argv}".to_string()];
            if matches!(
                kind,
                TunnelKind::Gre | TunnelKind::Ipip | TunnelKind::Sit | TunnelKind::Fou
            ) {
                let native = build_ip_tunnel_argv(&base, "add", &plan, &endpoint).unwrap();
                assert!(rendered
                    .commands
                    .iter()
                    .any(|command| command.argv == native));
            }
            for native in build_tunnel_address_argv(&base, &plan, &endpoint, Some(TEST_PLAN_ID)) {
                assert!(rendered
                    .commands
                    .iter()
                    .any(|command| command.argv == native));
            }
            if kind == TunnelKind::Openvpn {
                assert!(rendered.artifacts[0]
                    .content
                    .contains("ncp-ciphers AES-256-GCM:AES-128-GCM\n"));
                assert!(rendered.artifacts[1]
                    .content
                    .contains("data-ciphers AES-256-GCM:AES-128-GCM\n"));
            }
        }
    }
}

#[test]
fn preview_includes_declared_cleanup_routes_and_traffic_policy() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.runtime_topology.stale_interfaces = vec!["retired0".into()];
    input.runtime_topology.stale_routes = vec![RuntimeTunnelRoute {
        destination_cidr: "192.0.2.0/24".into(),
        ..Default::default()
    }];
    input.runtime_topology.routes = vec![RuntimeTunnelRoute {
        destination_cidr: "198.51.100.0/24".into(),
        metric: Some(17),
        ..Default::default()
    }];
    input.runtime_control.traffic_limit = RuntimeTunnelTrafficLimit {
        ingress_kbps: Some(8000),
        egress_kbps: Some(9000),
        burst_kb: Some(64),
    };
    let plan = plan_tunnel(&input).unwrap();
    let preview = render_tunnel_runtime_preview(&plan, Some(TEST_PLAN_ID)).unwrap();
    let ip = ["{runtime_ip_argv}".into()];
    let tc = ["{runtime_tc_argv}".into()];
    for endpoint in preview.endpoints {
        for native in build_tunnel_topology_cleanup_commands(&ip, &plan) {
            assert!(endpoint
                .commands
                .iter()
                .any(|command| command.phase == "cleanup" && command.argv == native.argv));
        }
        for native in build_tunnel_traffic_limit_commands(
            &tc,
            &plan.interface_name,
            &plan.runtime_control.traffic_limit,
        ) {
            assert!(endpoint
                .commands
                .iter()
                .any(|command| command.phase == "configure" && command.argv == native.argv));
        }
        let removal = build_ip_route_argv(
            &ip,
            "del",
            &plan.runtime_topology.routes[0],
            &plan.interface_name,
        );
        assert!(endpoint
            .commands
            .iter()
            .any(|command| command.phase == "shutdown" && command.argv == removal));
    }
    assert!(build_tunnel_traffic_limit_commands(
        &[],
        "tunab",
        &RuntimeTunnelTrafficLimit::default()
    )
    .is_empty());
}

#[test]
fn dynamic_bandwidth_defaults_off_and_does_not_change_the_runtime_plan() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.ospf = Some(ospf_config());
    let mut legacy_input = serde_json::to_value(&input).unwrap();
    legacy_input
        .as_object_mut()
        .unwrap()
        .remove("dynamic_bandwidth");
    let decoded: TunnelPlanInput = serde_json::from_value(legacy_input).unwrap();
    assert!(!decoded.dynamic_bandwidth);

    let static_plan = plan_tunnel(&decoded).unwrap();
    input.dynamic_bandwidth = true;
    let opted_in: TunnelPlanInput =
        serde_json::from_value(serde_json::to_value(&input).unwrap()).unwrap();
    assert!(opted_in.dynamic_bandwidth);
    let dynamic_plan = plan_tunnel(&opted_in).unwrap();
    assert_eq!(static_plan, dynamic_plan);
    assert_eq!(
        serde_json::to_vec(&static_plan).unwrap(),
        serde_json::to_vec(&dynamic_plan).unwrap(),
    );
    let plan_id = uuid::Uuid::nil();
    assert_eq!(
        tunnel_runtime_evidence_identity_hash(plan_id, &static_plan, None),
        tunnel_runtime_evidence_identity_hash(plan_id, &dynamic_plan, None),
    );
}

#[test]
fn tunnel_mtu_defaults_are_kind_aware_1500_underlay_baselines() {
    assert_eq!(default_tunnel_mtu(TunnelKind::Gre), Some(1476));
    assert_eq!(default_tunnel_mtu(TunnelKind::Ipip), Some(1480));
    assert_eq!(default_tunnel_mtu(TunnelKind::Sit), Some(1480));
    assert_eq!(default_tunnel_mtu(TunnelKind::Fou), Some(1468));
    assert_eq!(default_tunnel_mtu(TunnelKind::Wireguard), Some(1420));
    assert_eq!(default_tunnel_mtu(TunnelKind::Openvpn), Some(1500));
    assert_eq!(default_tunnel_mtu(TunnelKind::TunTap), None);
    assert_eq!(default_tunnel_mtu(TunnelKind::Custom), None);
}

#[test]
fn tunnel_plan_name_input_boundary_accepts_128_bytes_and_rejects_129() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.name = "p".repeat(128);
    assert!(plan_tunnel(&input).is_ok());

    // The total persisted identity is bounded, not only its trimmed view.
    // This 129-byte value previously passed because validation measured the
    // 128-byte trimmed suffix.
    input.name = format!(" {}", "p".repeat(128));
    assert_eq!(
        plan_tunnel(&input).unwrap_err(),
        NetworkPlanError::InvalidPlanIdentity
    );
}

#[test]
fn openvpn_listener_bind_matches_the_initiators_destination_family() {
    let mut input = plan_input(TunnelKind::Openvpn, RuntimeTunnelManager::AgentBuiltin);
    input.runtime_control.openvpn.listener_side = TunnelEndpointSide::Left;
    input.left_remote_underlay = "198.51.100.20".to_string();
    input.right_remote_underlay = "2001:db8::10".to_string();
    input.left_local_underlay = Some("2001:db8::10".to_string());
    input.right_local_underlay = Some("2001:db8::20".to_string());
    assert!(plan_tunnel(&input).is_ok());

    input.left_local_underlay = Some("198.51.100.10".to_string());
    assert_eq!(
        plan_tunnel(&input).unwrap_err(),
        NetworkPlanError::InvalidUnderlayAddress
    );

    input.left_local_underlay = Some("  ".to_string());
    input.right_local_underlay = Some(String::new());
    assert!(plan_tunnel(&input).is_ok());
}

fn ospf_config() -> TunnelOspfConfig {
    TunnelOspfConfig {
        mode: OspfControlMode::Reviewed,
        planned_latency_ms: 20.0,
        planned_packet_loss_ratio: 0.01,
        preference: 1.0,
        policy: OspfCostPolicy::default(),
        min_cost_delta: 5,
        healthy_windows: 2,
        left_adapter_definition_id: Some(LEFT_ROUTING_ADAPTER.to_string()),
        right_adapter_definition_id: Some(RIGHT_ROUTING_ADAPTER.to_string()),
    }
}

#[test]
fn routing_cost_privilege_payload_freezes_both_endpoint_snapshots() {
    let payload = routing_cost_update_privilege_payload(
        "00000000-0000-0000-0000-000000000001".parse().unwrap(),
        7,
        " recommendation-1 ",
        Some(14),
        None,
        22,
        &"a".repeat(64),
        &"b".repeat(64),
    );
    assert_eq!(
        payload,
        format!(
            "v3|00000000-0000-0000-0000-000000000001|7|recommendation-1|14|none|22|{}|{}",
            "a".repeat(64),
            "b".repeat(64)
        )
    );
}

#[test]
fn ospf_cost_is_monotonic_across_the_full_operator_bandwidth_range() {
    let policy = OspfCostPolicy::default();
    let mut previous = u16::MAX;
    for bandwidth_mbps in MIN_TUNNEL_BANDWIDTH_MBPS..=MAX_TUNNEL_BANDWIDTH_MBPS {
        let cost = ospf_cost(
            policy,
            TunnelObservation {
                latency_ms: 20.0,
                packet_loss_ratio: 0.0,
                bandwidth_mbps,
                preference: 1.0,
            },
        );
        assert!(cost <= previous, "cost increased at {bandwidth_mbps} Mbps");
        previous = cost;
    }
}

#[test]
fn arbitrary_bandwidth_values_use_the_smooth_cost_curve() {
    let cost = |bandwidth_mbps| {
        ospf_cost(
            OspfCostPolicy::default(),
            TunnelObservation {
                latency_ms: 20.0,
                packet_loss_ratio: 0.0,
                bandwidth_mbps,
                preference: 1.0,
            },
        )
    };
    assert_eq!(cost(10), 52);
    assert_eq!(cost(123), 29);
    assert_eq!(cost(1234), 23);
    assert_eq!(cost(9876), 21);
    assert!(cost(99).abs_diff(cost(101)) <= 1);
}

#[test]
fn latency_loss_and_preference_remain_primary_cost_inputs() {
    let policy = OspfCostPolicy::default();
    let healthy = ospf_cost(
        policy,
        TunnelObservation {
            latency_ms: 20.0,
            packet_loss_ratio: 0.0,
            bandwidth_mbps: 100,
            preference: 1.0,
        },
    );
    let unhealthy = ospf_cost(
        policy,
        TunnelObservation {
            latency_ms: 120.0,
            packet_loss_ratio: 0.1,
            bandwidth_mbps: 10_000,
            preference: 1.0,
        },
    );
    let preferred = ospf_cost(
        policy,
        TunnelObservation {
            latency_ms: 20.0,
            packet_loss_ratio: 0.0,
            bandwidth_mbps: 100,
            preference: 2.0,
        },
    );
    assert!(unhealthy > healthy);
    assert!(preferred < healthy);
}

#[test]
fn planned_ospf_cost_is_optional_and_computed_next_to_operator_inputs() {
    let without_ospf = plan_tunnel(&plan_input(
        TunnelKind::Gre,
        RuntimeTunnelManager::AgentBuiltin,
    ))
    .unwrap();
    assert_eq!(without_ospf.recommended_ospf_cost, None);

    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.ospf = Some(ospf_config());
    let expected = ospf_cost(
        OspfCostPolicy::default(),
        TunnelObservation {
            latency_ms: 20.0,
            packet_loss_ratio: 0.01,
            bandwidth_mbps: 1234,
            preference: 1.0,
        },
    );
    assert_eq!(
        plan_tunnel(&input).unwrap().recommended_ospf_cost,
        Some(expected)
    );
}

#[test]
fn evidence_identities_ignore_display_name_while_command_templates_use_it() {
    let plan_id = uuid::Uuid::new_v4();
    let mut plan = plan_tunnel(&plan_input(
        TunnelKind::Gre,
        RuntimeTunnelManager::AgentBuiltin,
    ))
    .unwrap();
    let hook = RuntimeTunnelCommand {
        argv: vec!["/bin/echo".to_string(), "{plan}".to_string()],
        ..Default::default()
    };
    plan.runtime_control.hooks.left.pre_start = Some(hook.clone());
    let topology_identity = tunnel_topology_identity_hash(plan_id, &plan);
    let runtime_identity = tunnel_runtime_evidence_identity_hash(plan_id, &plan, Some(1));
    plan.name = "renamed tunnel".to_string();

    assert_eq!(
        tunnel_topology_identity_hash(plan_id, &plan),
        topology_identity
    );
    assert_eq!(
        tunnel_runtime_evidence_identity_hash(plan_id, &plan, Some(1)),
        runtime_identity
    );
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    assert_eq!(
        render_runtime_tunnel_command(&hook, &plan, &endpoint, &[]).unwrap(),
        ["/bin/echo", "renamed tunnel"]
    );
    assert_ne!(
        tunnel_topology_identity_hash(uuid::Uuid::new_v4(), &plan),
        topology_identity
    );
    plan.kind = TunnelKind::Ipip;
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &plan),
        topology_identity
    );
    assert_ne!(
        tunnel_runtime_evidence_identity_hash(plan_id, &plan, Some(1)),
        runtime_identity
    );
}

#[test]
fn topology_identity_ignores_policy_only_plan_edits() {
    let plan_id = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let mut plan = plan_tunnel(&plan_input(
        TunnelKind::Gre,
        RuntimeTunnelManager::AgentBuiltin,
    ))
    .unwrap();
    let identity = tunnel_topology_identity_hash(plan_id, &plan);

    plan.bandwidth_mbps = 9_999;
    plan.left_mtu = Some(1_400);
    plan.right_mtu = Some(1_400);
    plan.ospf = Some(ospf_config());
    plan.recommended_ospf_cost = Some(42);

    assert_eq!(tunnel_topology_identity_hash(plan_id, &plan), identity);
}

#[test]
fn runtime_evidence_identity_tracks_runtime_policy_and_credential_generation() {
    let plan_id = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let mut plan = plan_tunnel(&plan_input(
        TunnelKind::Wireguard,
        RuntimeTunnelManager::AgentBuiltin,
    ))
    .unwrap();
    let identity = tunnel_runtime_evidence_identity_hash(plan_id, &plan, Some(1));

    plan.bandwidth_mbps = 9_999;
    assert_ne!(
        tunnel_runtime_evidence_identity_hash(plan_id, &plan, Some(1)),
        identity,
        "runtime policy edits must invalidate old adapter/traffic evidence"
    );

    let generation_one = tunnel_runtime_evidence_identity_hash(plan_id, &plan, Some(1));
    assert_ne!(
        tunnel_runtime_evidence_identity_hash(plan_id, &plan, Some(2)),
        generation_one,
        "credential rotation must invalidate old runtime evidence"
    );
}

#[test]
fn topology_identity_changes_with_endpoint_underlay_or_primary_family() {
    let plan_id = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let plan = plan_tunnel(&plan_input(
        TunnelKind::Gre,
        RuntimeTunnelManager::AgentBuiltin,
    ))
    .unwrap();
    let identity = tunnel_topology_identity_hash(plan_id, &plan);

    let mut changed_endpoint = plan.clone();
    changed_endpoint.right_tunnel_address = "10.255.0.3".to_string();
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_endpoint),
        identity
    );

    let mut changed_underlay = plan.clone();
    changed_underlay.left_remote_underlay = "198.51.100.11".to_string();
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_underlay),
        identity,
        "left remote underlay edit must detach prior path evidence"
    );

    let mut changed_underlay = plan.clone();
    changed_underlay.left_local_underlay = Some("198.51.100.12".to_string());
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_underlay),
        identity,
        "left local underlay edit must detach prior path evidence"
    );

    let mut changed_underlay = plan.clone();
    changed_underlay.right_remote_underlay = "203.0.113.21".to_string();
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_underlay),
        identity,
        "right remote underlay edit must detach prior path evidence"
    );

    let mut changed_underlay = plan.clone();
    changed_underlay.right_local_underlay = Some("203.0.113.22".to_string());
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_underlay),
        identity,
        "right local underlay edit must detach prior path evidence"
    );

    let mut changed_family = plan;
    changed_family.latency_primary_family = TunnelAddressFamily::Ipv6;
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_family),
        identity
    );
}

#[test]
fn tunnel_plan_rejects_ambiguous_identity_and_underlay() {
    let mut same_endpoint = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    same_endpoint.right_client_id = same_endpoint.left_client_id.clone();
    assert_eq!(
        plan_tunnel(&same_endpoint),
        Err(NetworkPlanError::InvalidTunnelEndpoints)
    );

    let mut malformed_underlay = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    malformed_underlay.right_remote_underlay = "not-an-address".to_string();
    assert_eq!(
        plan_tunnel(&malformed_underlay),
        Err(NetworkPlanError::InvalidUnderlayAddress)
    );

    let mut mixed_underlay = plan_input(
        TunnelKind::Wireguard,
        RuntimeTunnelManager::ExternalObserved,
    );
    mixed_underlay.right_remote_underlay = "2001:db8::20".to_string();
    mixed_underlay.right_local_underlay = Some("10.0.1.20".to_string());
    assert_eq!(
        plan_tunnel(&mixed_underlay),
        Err(NetworkPlanError::InvalidUnderlayAddress)
    );

    let mut native_ipv6_underlay = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    native_ipv6_underlay.left_remote_underlay = "2001:db8::10".to_string();
    native_ipv6_underlay.right_remote_underlay = "2001:db8::20".to_string();
    assert_eq!(
        plan_tunnel(&native_ipv6_underlay),
        Err(NetworkPlanError::InvalidUnderlayAddress)
    );

    let mut observed_ipv6_underlay = plan_input(
        TunnelKind::Wireguard,
        RuntimeTunnelManager::ExternalObserved,
    );
    observed_ipv6_underlay.left_remote_underlay = "2001:db8::10".to_string();
    observed_ipv6_underlay.right_remote_underlay = "2001:db8::20".to_string();
    assert!(plan_tunnel(&observed_ipv6_underlay).is_ok());
}

#[test]
fn tunnel_plan_keeps_nat_remote_destinations_independent_from_local_sources() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.left_remote_underlay = "203.0.113.20".to_string();
    input.left_local_underlay = Some("10.0.0.10".to_string());
    input.right_remote_underlay = "198.51.100.10".to_string();
    input.right_local_underlay = Some("10.0.1.20".to_string());

    let plan = plan_tunnel(&input).unwrap();
    let left = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let right = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Right).unwrap();

    assert_eq!(left.remote_underlay, "203.0.113.20");
    assert_eq!(left.local_underlay.as_deref(), Some("10.0.0.10"));
    assert_eq!(right.remote_underlay, "198.51.100.10");
    assert_eq!(right.local_underlay.as_deref(), Some("10.0.1.20"));
}

#[test]
fn tunnel_plan_rejects_duplicate_or_disconnected_endpoint_addresses() {
    let mut duplicate = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    duplicate.ipv4_tunnel = Some(ipv4_pair("10.255.0.1", "10.255.0.1"));
    assert_eq!(plan_tunnel(&duplicate), Err(NetworkPlanError::InvalidCidr));

    let mut disconnected = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    disconnected.ipv4_tunnel = Some(ipv4_pair("10.255.0.0", "10.255.0.2"));
    assert_eq!(
        plan_tunnel(&disconnected),
        Err(NetworkPlanError::InvalidCidr)
    );
}

#[test]
fn agent_builtin_accepts_its_supported_tunnel_kinds() {
    for kind in [
        TunnelKind::Gre,
        TunnelKind::Ipip,
        TunnelKind::Sit,
        TunnelKind::Fou,
        TunnelKind::Openvpn,
        TunnelKind::Wireguard,
    ] {
        assert!(plan_tunnel(&plan_input(kind, RuntimeTunnelManager::AgentBuiltin)).is_ok());
    }
    for kind in [TunnelKind::TunTap, TunnelKind::Custom] {
        assert_eq!(
            plan_tunnel(&plan_input(kind, RuntimeTunnelManager::AgentBuiltin)),
            Err(NetworkPlanError::UnsupportedRuntimeManagerTunnelKind)
        );
    }
}

#[test]
fn custom_adapter_plans_require_both_endpoint_definition_ids() {
    let valid = plan_input(TunnelKind::Wireguard, RuntimeTunnelManager::CustomAdapter);
    assert!(plan_tunnel(&valid).is_ok());

    let mut missing = valid.clone();
    missing.runtime_control.right_adapter_definition_id = None;
    assert_eq!(
        plan_tunnel(&missing),
        Err(NetworkPlanError::RuntimeTunnelAdapterCommandRequired)
    );
}

#[test]
fn custom_adapter_has_one_canonical_wire_name() {
    assert_eq!(
        serde_json::to_string(&RuntimeTunnelManager::CustomAdapter).unwrap(),
        "\"custom_adapter\""
    );
    assert_eq!(
        serde_json::from_str::<RuntimeTunnelManager>("\"custom_adapter\"").unwrap(),
        RuntimeTunnelManager::CustomAdapter
    );
    assert!(serde_json::from_str::<RuntimeTunnelManager>("\"external_managed_adapter\"").is_err());
}

#[test]
fn external_observed_plans_are_explicit_and_cannot_mutate() {
    let observed = plan_input(
        TunnelKind::Wireguard,
        RuntimeTunnelManager::ExternalObserved,
    );
    assert!(plan_tunnel(&observed).is_ok());

    let mut mutating = observed;
    mutating.runtime_control.traffic_limit.ingress_kbps = Some(1000);
    assert_eq!(
        plan_tunnel(&mutating),
        Err(NetworkPlanError::RuntimeTunnelObservedCannotMutate)
    );

    let mut topology_mutation = plan_input(
        TunnelKind::Wireguard,
        RuntimeTunnelManager::ExternalObserved,
    );
    topology_mutation.runtime_topology.stale_interfaces = vec!["wg-old".to_string()];
    assert_eq!(
        plan_tunnel(&topology_mutation),
        Err(NetworkPlanError::RuntimeTunnelTopologyRequiresAgentBuiltin)
    );
}

#[test]
fn custom_adapter_plans_delegate_topology_mutation_to_the_adapter() {
    let mut input = plan_input(TunnelKind::Wireguard, RuntimeTunnelManager::CustomAdapter);
    input.runtime_topology.routes.push(RuntimeTunnelRoute {
        destination_cidr: "10.60.0.0/16".to_string(),
        ..RuntimeTunnelRoute::default()
    });
    assert_eq!(
        plan_tunnel(&input),
        Err(NetworkPlanError::RuntimeTunnelTopologyRequiresAgentBuiltin)
    );
}

#[test]
fn topology_intent_accepts_only_declared_interfaces_and_routes() {
    let valid = RuntimeTunnelTopologyIntent {
        version: Some("v1".to_string()),
        desired_interfaces: vec!["tunab".to_string(), "tunab-peer".to_string()],
        stale_interfaces: vec!["tunab-old".to_string()],
        routes: vec![RuntimeTunnelRoute {
            destination_cidr: "10.60.0.0/16".to_string(),
            interface_name: Some("tunab".to_string()),
            metric: Some(20),
            ..RuntimeTunnelRoute::default()
        }],
        stale_routes: Vec::new(),
    };
    assert!(validate_runtime_topology_intent(&valid, "tunab").is_ok());

    let mut invalid = valid;
    invalid.desired_interfaces.push("../host".to_string());
    assert_eq!(
        validate_runtime_topology_intent(&invalid, "tunab"),
        Err(NetworkPlanError::InvalidRuntimeTunnelTopology)
    );
}

#[test]
fn endpoint_rendering_is_side_specific_without_generating_daemon_files() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.left_mtu = Some(1400);
    input.right_mtu = Some(1450);
    let plan = plan_tunnel(&input).unwrap();
    let left = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let right = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Right).unwrap();
    assert_eq!(left.local_client_id, "edge-a");
    assert_eq!(left.local_tunnel_address, "10.255.0.0");
    assert_eq!(left.local_mtu, Some(1400));
    assert_eq!(right.local_client_id, "edge-b");
    assert_eq!(right.local_tunnel_address, "10.255.0.1");
    assert_eq!(right.local_mtu, Some(1450));
}

#[test]
fn endpoint_allocator_returns_non_overlapping_dual_stack_pairs() {
    let allocation = allocate_tunnel_endpoints(
        Some("10.255.0.0/29"),
        Some("fd00::/125"),
        &["10.255.0.0".to_string(), "fd00::".to_string()],
        true,
        true,
    )
    .unwrap();
    let ipv4 = allocation.ipv4_tunnel.unwrap();
    let ipv6 = allocation.ipv6_tunnel.unwrap();
    assert_ne!(ipv4.left, "10.255.0.0");
    assert_ne!(ipv4.left, ipv4.right);
    assert_ne!(ipv6.left, "fd00::");
    assert_ne!(ipv6.left, ipv6.right);
}

#[test]
fn plan_rejects_out_of_range_bandwidth_and_invalid_ospf_binding() {
    let mut bandwidth = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    bandwidth.bandwidth_mbps = 10_001;
    assert_eq!(
        plan_tunnel(&bandwidth),
        Err(NetworkPlanError::InvalidBandwidthMbps)
    );

    let mut ospf = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    ospf.ospf = Some(ospf_config());
    ospf.ospf.as_mut().unwrap().left_adapter_definition_id = Some(String::new());
    assert_eq!(plan_tunnel(&ospf), Err(NetworkPlanError::InvalidOspfConfig));
}

#[test]
fn plan_rejects_invalid_endpoint_mtu_and_enforces_the_ipv6_minimum() {
    let mut too_small = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    too_small.left_mtu = Some(MIN_TUNNEL_MTU - 1);
    assert_eq!(
        plan_tunnel(&too_small),
        Err(NetworkPlanError::InvalidTunnelMtu)
    );

    let mut ipv6 = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    ipv6.ipv6_tunnel = Some(TunnelAddressPair {
        left: "fd00::".to_string(),
        right: "fd00::1".to_string(),
        prefix_len: 127,
    });
    ipv6.right_mtu = Some(MIN_IPV6_TUNNEL_MTU - 1);
    assert_eq!(plan_tunnel(&ipv6), Err(NetworkPlanError::InvalidTunnelMtu));
    ipv6.right_mtu = Some(MIN_IPV6_TUNNEL_MTU);
    assert!(plan_tunnel(&ipv6).is_ok());

    let mut sit = plan_input(TunnelKind::Sit, RuntimeTunnelManager::AgentBuiltin);
    sit.left_mtu = Some(MIN_IPV6_TUNNEL_MTU - 1);
    assert_eq!(plan_tunnel(&sit), Err(NetworkPlanError::InvalidTunnelMtu));

    let mut missing = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    missing.right_mtu = None;
    assert_eq!(
        plan_tunnel(&missing),
        Err(NetworkPlanError::TunnelMtuRequired)
    );

    let mut custom_adapter = plan_input(TunnelKind::Wireguard, RuntimeTunnelManager::CustomAdapter);
    custom_adapter.left_mtu = Some(1420);
    assert_eq!(
        plan_tunnel(&custom_adapter),
        Err(NetworkPlanError::TunnelMtuExternallyOwned)
    );
}

#[test]
fn published_network_wire_names_remain_stable() {
    let ospf = ospf_config();
    let encoded_ospf = serde_json::to_value(&ospf).unwrap();
    assert_eq!(
        encoded_ospf["left_adapter_template_id"],
        LEFT_ROUTING_ADAPTER
    );
    assert_eq!(
        encoded_ospf["right_adapter_template_id"],
        RIGHT_ROUTING_ADAPTER
    );
    assert!(encoded_ospf.get("left_adapter_definition_id").is_none());
    assert!(encoded_ospf.get("right_adapter_definition_id").is_none());

    let command = RuntimeTunnelCommand {
        argv: vec!["/opt/vpsman/routing-cost".to_string()],
        ..RuntimeTunnelCommand::default()
    };
    let adapter = RoutingCostAdapterCommands {
        source: RoutingCostCommandSource::ConfigurationPreset,
        definition_id: LEFT_ROUTING_ADAPTER.to_string(),
        definition_name: "FRR updater".to_string(),
        definition_hash: "a".repeat(64),
        status: command.clone(),
        update: command,
    };
    let encoded_adapter = serde_json::to_value(&adapter).unwrap();
    assert_eq!(encoded_adapter["template_id"], LEFT_ROUTING_ADAPTER);
    assert_eq!(encoded_adapter["template_name"], "FRR updater");
    assert_eq!(encoded_adapter["source"], "configuration_preset");
    assert!(encoded_adapter.get("definition_id").is_none());
    assert!(encoded_adapter.get("definition_name").is_none());

    let result = RoutingCostAdapterJobResult {
        contract_version: ROUTING_COST_ADAPTER_CONTRACT_VERSION,
        operation: RoutingCostAdapterOperation::Status,
        plan_id: "00000000-0000-4000-8000-000000000099".to_string(),
        endpoint_side: TunnelEndpointSide::Left,
        client_id: "edge-a".to_string(),
        adapter_definition_id: LEFT_ROUTING_ADAPTER.to_string(),
        adapter_definition_hash: "a".repeat(64),
        previous_cost: None,
        current_cost: 20,
        message: None,
    };
    let encoded_result = serde_json::to_value(result).unwrap();
    assert_eq!(encoded_result["adapter_template_id"], LEFT_ROUTING_ADAPTER);
    assert!(encoded_result.get("adapter_definition_id").is_none());
}
