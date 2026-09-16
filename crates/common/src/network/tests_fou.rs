use super::*;

fn fou_input(kind: RuntimeTunnelFouKind) -> TunnelPlanInput {
    let mut input = plan_input(TunnelKind::Fou, RuntimeTunnelManager::AgentBuiltin);
    input.runtime_control.fou.tunnel_kind = kind;
    input.left_mtu = Some(kind.default_mtu());
    input.right_mtu = Some(kind.default_mtu());
    if kind == RuntimeTunnelFouKind::Sit {
        input.ipv4_tunnel = None;
        input.ipv6_tunnel = Some(TunnelAddressPair {
            left: "fd00::".into(),
            right: "fd00::1".into(),
            prefix_len: 127,
        });
        input.latency_primary_family = TunnelAddressFamily::Ipv6;
    }
    input
}

#[test]
fn fou_type_owns_receive_protocol_transmit_device_and_preview() {
    for (kind, name, protocol, mtu) in [
        (RuntimeTunnelFouKind::Gre, "gre", 47, 1468),
        (RuntimeTunnelFouKind::Ipip, "ipip", 4, 1472),
        (RuntimeTunnelFouKind::Sit, "sit", 41, 1472),
    ] {
        let mut input = fou_input(kind);
        input.runtime_control.fou.port = 15555;
        input.runtime_control.fou.peer_port = 15556;
        let plan = plan_tunnel(&input).unwrap();
        assert_eq!(kind.ip_protocol(), protocol);
        assert_eq!(kind.default_mtu(), mtu);
        assert_eq!(tunnel_iproute2_mode(&plan).unwrap(), name);
        let preview = render_tunnel_runtime_preview(&plan, Some(TEST_PLAN_ID)).unwrap();
        for (index, side) in [TunnelEndpointSide::Left, TunnelEndpointSide::Right]
            .into_iter()
            .enumerate()
        {
            let endpoint = render_tunnel_endpoint_config(&plan, side).unwrap();
            let argv = build_ip_tunnel_argv(&["{runtime_ip_argv}".into()], "add", &plan, &endpoint)
                .unwrap();
            assert!(argv.windows(2).any(|pair| pair == ["type", name]));
            assert!(argv.windows(2).any(|pair| pair == ["encap-dport", "15556"]));
            assert!(preview.endpoints[index]
                .commands
                .iter()
                .any(|command| command.argv == argv));
            assert!(preview.endpoints[index].commands.iter().any(|command| {
                command
                    .argv
                    .windows(2)
                    .any(|pair| pair == ["ipproto", &protocol.to_string()])
                    && command
                        .argv
                        .windows(2)
                        .any(|pair| pair == ["port", "15555"])
            }));
        }
        let serialized = serde_json::to_value(&input.runtime_control.fou).unwrap();
        assert_eq!(serialized["tunnel_kind"], name);
        assert!(serialized.get("ipproto").is_none());
        let roundtrip: TunnelPlanInput =
            serde_json::from_value(serde_json::to_value(&input).unwrap()).unwrap();
        assert_eq!(roundtrip, input);
    }
}

#[test]
fn fou_default_is_gre_and_legacy_numeric_input_is_rejected() {
    assert_eq!(
        RuntimeTunnelFouOptions::default().tunnel_kind,
        RuntimeTunnelFouKind::Gre
    );
    assert_eq!(
        serde_json::from_str::<RuntimeTunnelFouOptions>("{}").unwrap(),
        RuntimeTunnelFouOptions::default()
    );
    for old in [
        r#"{"ipproto":4}"#,
        r#"{"ipproto":47}"#,
        r#"{"tunnel_kind":"gre","ipproto":4}"#,
    ] {
        assert!(serde_json::from_str::<RuntimeTunnelFouOptions>(old)
            .unwrap_err()
            .to_string()
            .contains("ipproto"));
    }
    for kind in ["gretap", "ip6gre", "openvpn", "fou", "47"] {
        assert!(serde_json::from_value::<RuntimeTunnelFouOptions>(
            serde_json::json!({"tunnel_kind":kind})
        )
        .is_err());
    }
    let plan = plan_tunnel(&fou_input(RuntimeTunnelFouKind::Gre)).unwrap();
    assert_eq!(tunnel_iproute2_mode(&plan).unwrap(), "gre");
}

#[test]
fn fou_family_validation_covers_primary_and_additional_addresses_before_runtime() {
    for kind in RuntimeTunnelFouKind::ALL {
        let valid = fou_input(kind);
        assert!(plan_tunnel(&valid).is_ok());
        for family in [TunnelAddressFamily::Ipv4, TunnelAddressFamily::Ipv6] {
            let mut input = valid.clone();
            match family {
                TunnelAddressFamily::Ipv4 => input
                    .additional_addresses
                    .right
                    .ipv4
                    .push("10.254.0.2/32".into()),
                TunnelAddressFamily::Ipv6 => input
                    .additional_addresses
                    .left
                    .ipv6
                    .push("fd00:1::1/128".into()),
            }
            assert_eq!(plan_tunnel(&input).is_ok(), kind.supports_family(family));
            let mut plan = plan_tunnel(&valid).unwrap();
            plan.additional_addresses = input.additional_addresses;
            // Agent commands can carry resolved plans directly; the endpoint
            // renderer enforces the same family contract before mutation.
            assert_eq!(
                render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).is_ok(),
                kind.supports_family(family)
            );
        }
    }
    let mut input = fou_input(RuntimeTunnelFouKind::Ipip);
    input.ipv6_tunnel = fou_input(RuntimeTunnelFouKind::Sit).ipv6_tunnel;
    assert!(matches!(
        plan_tunnel(&input),
        Err(NetworkPlanError::InvalidFouAddressFamily(_))
    ));
    input.runtime_control.fou.tunnel_kind = RuntimeTunnelFouKind::Gre;
    assert!(plan_tunnel(&input).is_ok());
    input.runtime_control.fou.tunnel_kind = RuntimeTunnelFouKind::Sit;
    assert!(matches!(
        plan_tunnel(&input),
        Err(NetworkPlanError::InvalidFouAddressFamily(_))
    ));
}

#[test]
fn fou_type_and_udp_path_detach_evidence_but_display_edits_do_not() {
    let plan = plan_tunnel(&fou_input(RuntimeTunnelFouKind::Gre)).unwrap();
    let identity = tunnel_topology_identity_hash(TEST_PLAN_ID, &plan);
    let mut changed = plan.clone();
    changed.runtime_control.fou.tunnel_kind = RuntimeTunnelFouKind::Ipip;
    assert_ne!(
        identity,
        tunnel_topology_identity_hash(TEST_PLAN_ID, &changed)
    );
    changed = plan.clone();
    changed.runtime_control.fou.port += 1;
    assert_ne!(
        identity,
        tunnel_topology_identity_hash(TEST_PLAN_ID, &changed)
    );
    changed = plan.clone();
    changed.runtime_control.fou.peer_port += 1;
    assert_ne!(
        identity,
        tunnel_topology_identity_hash(TEST_PLAN_ID, &changed)
    );
    changed = plan.clone();
    changed.name.push_str("-renamed");
    assert_eq!(
        identity,
        tunnel_topology_identity_hash(TEST_PLAN_ID, &changed)
    );
}

#[test]
fn fou_model_change_leaves_canonical_non_fou_json_and_identity_unchanged() {
    for kind in [
        TunnelKind::Gre,
        TunnelKind::Ipip,
        TunnelKind::Sit,
        TunnelKind::Wireguard,
        TunnelKind::Openvpn,
    ] {
        let input = plan_input(kind, RuntimeTunnelManager::AgentBuiltin);
        let value = serde_json::to_value(&input).unwrap();
        assert!(
            value.get("runtime_control").is_none(),
            "default FOU options must not appear in unrelated stored plans"
        );
        assert_eq!(
            serde_json::from_value::<TunnelPlanInput>(value.clone()).unwrap(),
            input
        );
        let plan = plan_tunnel(&input).unwrap();
        // This is the exact pre-change topology identity payload. FOU alone
        // adds a typed-path coordinate; existing non-FOU evidence stays attached.
        let old_payload = serde_json::to_vec(&serde_json::json!({
            "plan_id": TEST_PLAN_ID.to_string(), "kind": format!("{:?}", plan.kind),
            "left_client_id": plan.left_client_id, "right_client_id": plan.right_client_id,
            "interface_name": plan.interface_name, "left_remote_underlay": plan.left_remote_underlay,
            "left_local_underlay": plan.left_local_underlay, "right_remote_underlay": plan.right_remote_underlay,
            "right_local_underlay": plan.right_local_underlay, "left_tunnel_address": plan.left_tunnel_address,
            "right_tunnel_address": plan.right_tunnel_address, "ipv4_tunnel": plan.ipv4_tunnel,
            "ipv6_tunnel": plan.ipv6_tunnel, "latency_primary_family": format!("{:?}", plan.latency_primary_family),
        })).unwrap();
        assert_eq!(
            tunnel_topology_identity_hash(TEST_PLAN_ID, &plan),
            crate::payload_hash(&old_payload)
        );
    }
}
