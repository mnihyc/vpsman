#[tokio::test]
#[ignore = "requires disposable Docker private networking, SYS_ADMIN for nested network namespaces, NET_ADMIN, TUN, WireGuard and OpenVPN"]
async fn builtin_additional_and_link_local_packet_matrix() {
    builtin_packet_matrix(&[
        TunnelKind::Gre,
        TunnelKind::Ipip,
        TunnelKind::Sit,
        TunnelKind::Wireguard,
        TunnelKind::Openvpn,
    ])
    .await;
}

#[tokio::test]
#[ignore = "requires disposable Docker private networking, SYS_ADMIN for nested namespaces, NET_ADMIN and available kernel FOU"]
async fn fou_additional_ipv4_packet_and_address_lifecycle() {
    require_isolated_network();
    // This is deliberately separate from the missing-capability test: success
    // requires a real FOU listener and real IPv4-in-UDP traffic.
    checked_native("/sbin/ip", &["fou", "show"]).await;
    builtin_packet_matrix_with_fou(&[TunnelKind::Fou], vpsman_common::RuntimeTunnelFouKind::Ipip).await;
    builtin_address_policy_matrix_with_fou(&[TunnelKind::Fou], vpsman_common::RuntimeTunnelFouKind::Ipip).await;
}

#[tokio::test]
#[ignore = "requires disposable Docker private networking, SYS_ADMIN for nested namespaces, NET_ADMIN and available kernel FOU/GRE"]
async fn fou_gre_packet_and_address_lifecycle() {
    builtin_packet_matrix_with_fou(&[TunnelKind::Fou], vpsman_common::RuntimeTunnelFouKind::Gre).await;
    builtin_address_policy_matrix_with_fou(&[TunnelKind::Fou], vpsman_common::RuntimeTunnelFouKind::Gre).await;
}

#[tokio::test]
#[ignore = "requires disposable Docker private networking, SYS_ADMIN for nested namespaces, NET_ADMIN and available kernel FOU/SIT"]
async fn fou_sit_packet_and_address_lifecycle() {
    builtin_packet_matrix_with_fou(&[TunnelKind::Fou], vpsman_common::RuntimeTunnelFouKind::Sit).await;
    builtin_address_policy_matrix_with_fou(&[TunnelKind::Fou], vpsman_common::RuntimeTunnelFouKind::Sit).await;
}

async fn create_packet_namespaces() {
    for namespace in ["address-left", "address-right"] {
        checked_native("/sbin/ip", &["netns", "add", namespace]).await;
    }
    checked_native(
        "/sbin/ip",
        &[
            "link",
            "add",
            "under-left",
            "type",
            "veth",
            "peer",
            "name",
            "under-right",
        ],
    )
    .await;
    for (namespace, device, underlay) in [
        ("address-left", "under-left", "192.0.2.1/24"),
        ("address-right", "under-right", "192.0.2.2/24"),
    ] {
        checked_native("/sbin/ip", &["link", "set", device, "netns", namespace]).await;
        checked_native(
            "/sbin/ip",
            &[
                "netns", "exec", namespace, "/sbin/ip", "addr", "add", underlay, "dev", device,
            ],
        )
        .await;
        checked_native(
            "/sbin/ip",
            &[
                "netns", "exec", namespace, "/sbin/ip", "link", "set", device, "up",
            ],
        )
        .await;
        checked_native(
            "/sbin/ip",
            &[
                "netns", "exec", namespace, "/sbin/ip", "link", "set", "lo", "up",
            ],
        )
        .await;
    }
}

async fn reconcile_packet_endpoint(
    namespace: &str,
    id: &str,
    plan: &TunnelPlan,
    side: TunnelEndpointSide,
    credentials: Option<&TunnelEndpointBuiltinCredentials>,
) {
    let fixture = serde_json::json!({"id":id, "plan":plan, "side":side, "credentials":credentials});
    let output = tokio::process::Command::new("/sbin/ip")
        .args(["netns", "exec", namespace]).arg(std::env::current_exe().unwrap())
        .args(["network_runtime::tests::isolated_linux_failures::builtin_additional_and_link_local_packet_matrix", "--exact", "--ignored", "--nocapture"])
        .env("VPSMAN_ADDRESS_TEST_ENDPOINT", fixture.to_string()).output().await.unwrap();
    assert!(
        output.status.success(),
        "{:?}/{namespace}: {} {}",
        plan.kind,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "requires disposable Docker private networking, SYS_ADMIN for nested namespaces, NET_ADMIN and WireGuard"]
async fn parallel_plans_keep_distinct_stable_link_local_addresses_and_packets() {
    require_isolated_network();
    create_packet_namespaces().await;
    let mut plans = Vec::new();
    for index in 0..2u16 {
        let id = uuid::Uuid::new_v4();
        let mut plan = isolated_plan(TunnelKind::Wireguard);
        plan.interface_name = format!("vpsparallel{index}");
        plan.left_local_underlay = Some("192.0.2.1".into());
        plan.right_local_underlay = Some("192.0.2.2".into());
        plan.left_remote_underlay = "192.0.2.2".into();
        plan.right_remote_underlay = "192.0.2.1".into();
        plan.runtime_control.wireguard.left_listen_port = 51820 + index * 2;
        plan.runtime_control.wireguard.right_listen_port = 51821 + index * 2;
        let pair = plan.ipv4_tunnel.as_mut().unwrap();
        pair.left = format!("10.255.0.{}", index * 2);
        pair.right = format!("10.255.0.{}", index * 2 + 1);
        plan.left_tunnel_address = pair.left.clone();
        plan.right_tunnel_address = pair.right.clone();
        plan.additional_addresses.left.ipv6 = vec![format!("fd00:{}::1/64", 124 + index)];
        plan.additional_addresses.right.ipv6 = vec![format!("fd00:{}::2/64", 124 + index)];
        let mut left = wireguard_credentials().await;
        let mut right = wireguard_credentials().await;
        if let (
            TunnelEndpointBuiltinCredentials::Wireguard {
                local_public_key_base64: left_public,
                peer_public_key_base64: left_peer,
                ..
            },
            TunnelEndpointBuiltinCredentials::Wireguard {
                local_public_key_base64: right_public,
                peer_public_key_base64: right_peer,
                ..
            },
        ) = (&mut left, &mut right)
        {
            *left_peer = right_public.clone();
            *right_peer = left_public.clone();
        }
        for (namespace, side, credentials) in [
            ("address-left", TunnelEndpointSide::Left, &left),
            ("address-right", TunnelEndpointSide::Right, &right),
        ] {
            reconcile_packet_endpoint(namespace, &id.to_string(), &plan, side, Some(credentials))
                .await;
        }
        plans.push((id, plan, left, right));
    }
    for client in ["edge-a", "edge-b"] {
        assert_ne!(
            vpsman_common::tunnel_generated_link_local(plans[0].0, client),
            vpsman_common::tunnel_generated_link_local(plans[1].0, client)
        );
    }
    // Reapply both live plans after a display-name change. Neither the shared
    // endpoint identity nor the other simultaneously active plan may change.
    for (id, plan, left, right) in &mut plans {
        plan.name.push_str("-renamed");
        for (namespace, side, client, credentials) in [
            (
                "address-left",
                TunnelEndpointSide::Left,
                &plan.left_client_id,
                left,
            ),
            (
                "address-right",
                TunnelEndpointSide::Right,
                &plan.right_client_id,
                right,
            ),
        ] {
            let before = checked_native(
                "/sbin/ip",
                &[
                    "-n",
                    namespace,
                    "-j",
                    "addr",
                    "show",
                    "dev",
                    &plan.interface_name,
                ],
            )
            .await
            .stdout;
            let before: serde_json::Value = serde_json::from_slice(&before).unwrap();
            reconcile_packet_endpoint(namespace, &id.to_string(), plan, side, Some(credentials))
                .await;
            let after = checked_native(
                "/sbin/ip",
                &[
                    "-n",
                    namespace,
                    "-j",
                    "addr",
                    "show",
                    "dev",
                    &plan.interface_name,
                ],
            )
            .await
            .stdout;
            let after: serde_json::Value = serde_json::from_slice(&after).unwrap();
            assert_eq!(before[0]["ifindex"], after[0]["ifindex"]);
            let expected = vpsman_common::tunnel_generated_link_local(*id, client)
                .split('/')
                .next()
                .unwrap()
                .to_string();
            let local_addresses = after[0]["addr_info"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|address| address["local"].as_str())
                .filter(|address| address.starts_with("fe80:"))
                .collect::<Vec<_>>();
            assert_eq!(local_addresses, [expected.as_str()]);
        }
        let target = vpsman_common::tunnel_generated_link_local(*id, &plan.right_client_id)
            .split('/')
            .next()
            .unwrap()
            .to_string();
        checked_native(
            "/sbin/ip",
            &[
                "netns",
                "exec",
                "address-left",
                "/usr/bin/ping",
                "-n",
                "-c",
                "3",
                "-i",
                "0.5",
                "-W",
                "2",
                "-I",
                &plan.interface_name,
                &target,
            ],
        )
        .await;
    }
    for namespace in ["address-left", "address-right"] {
        checked_native("/sbin/ip", &["netns", "del", namespace]).await;
    }
}

async fn builtin_packet_matrix(kinds: &[TunnelKind]) {
    builtin_packet_matrix_with_fou(kinds, vpsman_common::RuntimeTunnelFouKind::default()).await;
}

async fn builtin_packet_matrix_with_fou(kinds: &[TunnelKind], fou_kind: vpsman_common::RuntimeTunnelFouKind) {
    require_isolated_network();
    // Re-enter this test in each isolated endpoint namespace. The child executes
    // the production reconciler; the parent verifies real packets.
    if let Ok(raw) = std::env::var("VPSMAN_ADDRESS_TEST_ENDPOINT") {
        let fixture: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let plan: TunnelPlan = serde_json::from_value(fixture["plan"].clone()).unwrap();
        let side: TunnelEndpointSide = serde_json::from_value(fixture["side"].clone()).unwrap();
        let credentials: Option<TunnelEndpointBuiltinCredentials> =
            serde_json::from_value(fixture["credentials"].clone()).unwrap();
        let mut config = config();
        config.client_id = if side == TunnelEndpointSide::Left {
            plan.left_client_id.clone()
        } else {
            plan.right_client_id.clone()
        };
        let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
            config: &config,
            plan_id: fixture["id"].as_str(),
            plan: &plan,
            previous_plan: None,
            builtin_credentials: credentials.as_ref(),
            runtime_adapter: None,
            side,
            max_timeout_secs: 30,
            effective_uid_override: None,
        })
        .await
        .unwrap();
        assert_eq!(report["status"], "converged", "{report}");
        return;
    }
    for &kind in kinds {
        let id = uuid::Uuid::new_v4().to_string();
        create_packet_namespaces().await;
        let mut plan = isolated_plan(kind);
        set_fou_test_kind(&mut plan, fou_kind);
        let carries_ipv4 = packet_test_supports_family(&plan, TunnelAddressFamily::Ipv4);
        let carries_ipv6 = packet_test_supports_family(&plan, TunnelAddressFamily::Ipv6);
        plan.interface_name = "vpspkt".into();
        plan.left_local_underlay = Some("192.0.2.1".into());
        plan.right_local_underlay = Some("192.0.2.2".into());
        plan.left_remote_underlay = "192.0.2.2".into();
        plan.right_remote_underlay = "192.0.2.1".into();
        if carries_ipv4 {
            plan.additional_addresses.left.ipv4 = vec!["10.254.20.1/30".into()];
            plan.additional_addresses.right.ipv4 = vec!["10.254.20.2/30".into()];
        }
        if carries_ipv6 {
            plan.additional_addresses.left.ipv6 =
                vec!["fd00:123::1/64".into(), "fe80::111/64".into()];
            plan.additional_addresses.right.ipv6 =
                vec!["fd00:123::2/64".into(), "fe80::222/64".into()];
        }
        let (mut left_credentials, mut right_credentials) = match kind {
            TunnelKind::Wireguard => (
                Some(wireguard_credentials().await),
                Some(wireguard_credentials().await),
            ),
            TunnelKind::Openvpn => (
                Some(generated_credentials(&format!("{id}-left")).await),
                Some(generated_credentials(&format!("{id}-right")).await),
            ),
            _ => (None, None),
        };
        match (&mut left_credentials, &mut right_credentials) {
            (
                Some(TunnelEndpointBuiltinCredentials::Wireguard {
                    local_public_key_base64: left_public,
                    peer_public_key_base64: left_peer,
                    ..
                }),
                Some(TunnelEndpointBuiltinCredentials::Wireguard {
                    local_public_key_base64: right_public,
                    peer_public_key_base64: right_peer,
                    ..
                }),
            ) => {
                *left_peer = right_public.clone();
                *right_peer = left_public.clone();
            }
            (
                Some(TunnelEndpointBuiltinCredentials::Openvpn {
                    local_certificate_pem: left_cert,
                    peer_issuer_certificate_pem: left_ca,
                    ..
                }),
                Some(TunnelEndpointBuiltinCredentials::Openvpn {
                    local_certificate_pem: right_cert,
                    peer_issuer_certificate_pem: right_ca,
                    ..
                }),
            ) => {
                *left_ca = right_cert.clone();
                *right_ca = left_cert.clone();
            }
            _ => {}
        }
        for (namespace, side, credentials) in [
            ("address-left", TunnelEndpointSide::Left, left_credentials),
            (
                "address-right",
                TunnelEndpointSide::Right,
                right_credentials,
            ),
        ] {
            reconcile_packet_endpoint(namespace, &id, &plan, side, credentials.as_ref()).await;
        }
        let mut targets = Vec::new();
        if carries_ipv4 {
            targets.push("10.254.20.2".to_string());
        }
        if carries_ipv6 {
            targets.push("fd00:123::2".to_string());
            targets.push("fe80::222".to_string());
            targets.push(
                vpsman_common::tunnel_generated_link_local(
                    uuid::Uuid::parse_str(&id).unwrap(),
                    &plan.right_client_id,
                )
                .split('/')
                .next()
                .unwrap()
                .to_string(),
            );
        }
        for target in targets {
            let output = native(
                "/sbin/ip",
                &[
                    "netns",
                    "exec",
                    "address-left",
                    "/usr/bin/ping",
                    "-n",
                    "-c",
                    "3",
                    "-i",
                    "0.5",
                    "-W",
                    "2",
                    "-I",
                    &plan.interface_name,
                    &target,
                ],
            )
            .await;
            assert!(
                output.status.success(),
                "{kind:?} could not reach {target}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        for namespace in ["address-left", "address-right"] {
            let pids = checked_native("/sbin/ip", &["netns", "pids", namespace])
                .await
                .stdout;
            for pid in String::from_utf8(pids).unwrap().split_whitespace() {
                checked_native("/bin/kill", &["-TERM", pid]).await;
            }
            checked_native("/sbin/ip", &["netns", "del", namespace]).await;
        }
    }
}
