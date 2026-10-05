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

// OpenVPN's TUN/PID readiness is local convergence, not completion of TLS.
// Keep this separation in the packet fixture; do not change production startup.
#[tokio::test]
#[ignore = "requires disposable Docker private networking, SYS_ADMIN for nested namespaces, NET_ADMIN, TUN and OpenVPN"]
async fn openvpn_additional_and_link_local_packets_after_tls_ready() {
    builtin_packet_matrix(&[TunnelKind::Openvpn]).await;
}

#[tokio::test]
#[ignore = "requires disposable Docker private networking, SYS_ADMIN for nested namespaces, NET_ADMIN, TUN and OpenVPN"]
async fn openvpn_legacy_generated_config_upgrade_removes_duplicate_addresses() {
    builtin_packet_matrix_with_openvpn_upgrade(
        &[TunnelKind::Openvpn],
        vpsman_common::RuntimeTunnelFouKind::default(),
        true,
    )
    .await;
}

fn packet_openvpn_path(plan_id: &str, side: &str, name: &str) -> PathBuf {
    crate::state_dir::agent_state_dir()
        .unwrap()
        .join("network-tunnels")
        .join(plan_id)
        .join(side)
        .join(name)
}

fn packet_openvpn_log_ready(log: &str) -> std::result::Result<bool, String> {
    // These fresh, unique per-run logs must show a successful first connection.
    // Do not accept a previous success followed by failure/restart, or "With Errors".
    for line in log.lines() {
        if [
            "VERIFY ERROR:",
            "TLS Error:",
            "AUTH_FAILED",
            "Options error:",
            "OPTIONS ERROR:",
            "Exiting due to fatal error",
            "Initialization Sequence Completed With Errors",
            "SIGUSR1[",
            "SIGTERM[",
            "SIGHUP[",
        ]
        .iter()
        .any(|marker| line.contains(marker))
        {
            return Err(line.to_string());
        }
    }
    Ok(log.lines().any(|line| {
        line.trim_end().ends_with("Initialization Sequence Completed")
    }))
}

#[test]
fn openvpn_packet_readiness_requires_successful_tls_initialization() {
    for log in [
        "",
        "TUN/TAP device vpspkt opened\n",
        "Peer Connection Initiated with [AF_INET]192.0.2.2:1194\n",
    ] {
        assert_eq!(packet_openvpn_log_ready(log), Ok(false));
    }
    let ready = "2026-09-17 00:00:00 Initialization Sequence Completed\n";
    assert_eq!(packet_openvpn_log_ready(ready), Ok(true));
    for error in [
        "Initialization Sequence Completed With Errors",
        "VERIFY ERROR: depth=0, error=certificate has expired",
        "TLS Error: TLS handshake failed",
        "AUTH_FAILED",
        "Options error: unsupported option",
        "OPTIONS ERROR: failed to import crypto options",
        "Exiting due to fatal error",
        "SIGUSR1[soft,tls-error] received, process restarting",
        "SIGTERM[hard,] received, process exiting",
        "SIGHUP[hard,] received, process restarting",
    ] {
        assert!(packet_openvpn_log_ready(error).is_err(), "{error}");
        assert!(packet_openvpn_log_ready(&format!("{ready}{error}\n")).is_err());
    }
}

async fn packet_file_tail(path: &Path) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    // Match the fixture's 16 KiB command-report budget. Never read credentials
    // or entire logs into a panic; only these explicit log/status files.
    const MAX_BYTES: u64 = 16 * 1024;
    let mut file = tokio::fs::File::open(path).await?;
    let start = file.metadata().await?.len().saturating_sub(MAX_BYTES);
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES).read_to_end(&mut bytes).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn dump_packet_openvpn_diagnostics(plan_id: &str, interface: &str) {
    eprintln!("=== OpenVPN packet diagnostics (disposable fixture only) ===");
    for (namespace, side) in [("address-left", "left"), ("address-right", "right")] {
        for name in ["packet-openvpn.log", "openvpn.status", "openvpn.pid"] {
            let path = packet_openvpn_path(plan_id, side, name);
            match packet_file_tail(&path).await {
                Ok(text) => eprintln!("--- {namespace}/{name} ---\n{text}"),
                Err(error) => eprintln!("--- {namespace}/{name}: {error} ---"),
            }
        }
    }
    // Read-only namespace state; neither keys, PEM files, process environments
    // nor generated configurations are dumped. Each command has the existing
    // native() ten-second helper bound.
    for namespace in ["address-left", "address-right"] {
        for args in [
            vec!["-n", namespace, "-s", "-d", "link", "show", "dev", interface],
            vec!["-n", namespace, "addr", "show", "dev", interface],
            vec!["-n", namespace, "route", "show", "table", "all"],
            vec!["-n", namespace, "-6", "route", "show", "table", "all"],
        ] {
            let output = native("/sbin/ip", &args).await;
            eprintln!(
                "--- ip {args:?} ({}) ---\n{}{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

async fn wait_for_packet_openvpn_tls(plan_id: &str, interface: &str) {
    let started = tokio::time::Instant::now();
    // Test setup only: one default OpenVPN handshake window, not an arbitrary
    // sleep, ping retry, daemon restart, or change to production timeouts.
    let result = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let mut ready = true;
            for side in ["left", "right"] {
                let path = packet_openvpn_path(plan_id, side, "packet-openvpn.log");
                let log = match packet_file_tail(&path).await {
                    Ok(log) => log,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                    Err(error) => return Err(format!("{side}: cannot read daemon log: {error}")),
                };
                ready &= packet_openvpn_log_ready(&log)
                    .map_err(|error| format!("{side}: {error}"))?;
            }
            if ready {
                return Ok::<(), String>(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    let failure = match result {
        Ok(Ok(())) => {
            eprintln!(
                "OpenVPN packet fixture: both TLS sessions ready after {:?}; measuring packets now",
                started.elapsed()
            );
            return;
        }
        Ok(Err(error)) => error,
        Err(_) => "both endpoints did not complete TLS initialization within 60 seconds".into(),
    };
    dump_packet_openvpn_diagnostics(plan_id, interface).await;
    panic!("OpenVPN packet setup failed before measurement: {failure}");
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
#[ignore = "requires disposable Docker private networking, SYS_ADMIN for nested namespaces, NET_ADMIN and kernel ip6_gre"]
async fn gre6_packet_and_address_lifecycle() {
    builtin_packet_matrix(&[TunnelKind::Gre6]).await;
    builtin_address_policy_matrix(&[TunnelKind::Gre6]).await;
}

#[tokio::test]
#[ignore = "requires disposable Docker private networking, SYS_ADMIN for nested namespaces, NET_ADMIN and kernel fou6"]
async fn fou_gre6_packet_and_address_lifecycle() {
    require_isolated_network();
    // An unrelated IPv4 listener on the same port must survive IPv6 ownership
    // checks and teardown. The product's conservative port reservations remain.
    checked_native("/sbin/ip", &["fou", "add", "port", "5555", "ipproto", "47"]).await;
    builtin_packet_matrix_with_fou(&[TunnelKind::Fou], vpsman_common::RuntimeTunnelFouKind::Gre6).await;
    builtin_address_policy_matrix_with_fou(&[TunnelKind::Fou], vpsman_common::RuntimeTunnelFouKind::Gre6).await;
    let listeners: serde_json::Value = serde_json::from_slice(&checked_native("/sbin/ip", &["-j", "fou", "show"]).await.stdout).unwrap();
    assert!(listeners.as_array().unwrap().iter().any(|listener| listener["port"] == 5555 && listener["ipproto"] == 47 && matches!(listener["family"].as_str(), None | Some("inet"))), "unrelated IPv4 listener was removed: {listeners}");
    checked_native("/sbin/ip", &["fou", "del", "port", "5555"]).await;
}

#[tokio::test]
#[ignore = "requires disposable Docker private networking, SYS_ADMIN for nested namespaces, NET_ADMIN and available kernel FOU/SIT"]
async fn fou_sit_packet_and_address_lifecycle() {
    builtin_packet_matrix_with_fou(&[TunnelKind::Fou], vpsman_common::RuntimeTunnelFouKind::Sit).await;
    builtin_address_policy_matrix_with_fou(&[TunnelKind::Fou], vpsman_common::RuntimeTunnelFouKind::Sit).await;
}

async fn create_packet_namespaces(ipv6_underlay: bool) {
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
    if ipv6_underlay {
        for (namespace, device, address) in [("address-left", "under-left", "fd00:abcd::1/64"), ("address-right", "under-right", "fd00:abcd::2/64")] {
            checked_native("/sbin/ip", &["-n", namespace, "-6", "addr", "add", address, "dev", device, "nodad"]).await;
        }
    }
}

async fn reconcile_packet_endpoint(
    namespace: &str,
    id: &str,
    plan: &TunnelPlan,
    side: TunnelEndpointSide,
    credentials: Option<&TunnelEndpointBuiltinCredentials>,
) {
    reconcile_packet_endpoint_fixture(namespace, id, plan, side, credentials, false, None).await;
}

async fn reconcile_packet_endpoint_fixture(
    namespace: &str,
    id: &str,
    plan: &TunnelPlan,
    side: TunnelEndpointSide,
    credentials: Option<&TunnelEndpointBuiltinCredentials>,
    legacy_openvpn: bool,
    expect_restart: Option<bool>,
) {
    let fixture = serde_json::json!({
        "id":id, "plan":plan, "side":side, "credentials":credentials,
        "legacy_openvpn":legacy_openvpn, "expect_restart":expect_restart,
    });
    let output = tokio::process::Command::new("/sbin/ip")
        .args(["netns", "exec", namespace]).arg(std::env::current_exe().unwrap())
        .args(["network_runtime::tests::isolated_linux_failures::builtin_additional_and_link_local_packet_matrix", "--exact", "--ignored", "--nocapture"])
        .env("VPSMAN_ADDRESS_TEST_ENDPOINT", fixture.to_string()).output().await.unwrap();
    if !output.status.success() && plan.kind == TunnelKind::Openvpn {
        dump_packet_openvpn_diagnostics(id, &plan.interface_name).await;
    }
    assert!(
        output.status.success(),
        "{:?}/{namespace}: {} {}",
        plan.kind,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn packet_openvpn_address_state(namespace: &str, interface: &str) -> serde_json::Value {
    let output = checked_native(
        "/sbin/ip", &["-n", namespace, "-j", "addr", "show", "dev", interface],
    ).await;
    let links: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // iproute2 4.15 can prepend unnamed records to a device-filtered response.
    let matching = links.as_array().unwrap().iter().filter(|link| link["ifname"] == interface).collect::<Vec<_>>();
    assert_eq!(matching.len(), 1);
    matching[0].clone()
}

fn assert_packet_openvpn_address_tuples(
    state: &serde_json::Value,
    id: &str,
    plan: &TunnelPlan,
    side: TunnelEndpointSide,
) {
    // Compare the complete multiset, not CIDR membership: /32 + /31 for the
    // same IPv4 local address must fail, as must a peerless IPv6 primary.
    let canonical = |address: &str| address.parse::<std::net::IpAddr>().unwrap().to_string();
    let mut actual = state["addr_info"].as_array().unwrap().iter().map(|address| (
        address["family"].as_str().unwrap().to_string(),
        canonical(address["local"].as_str().unwrap()),
        address["prefixlen"].as_u64().unwrap(),
        address.get("address").or_else(|| address.get("peer"))
            .and_then(serde_json::Value::as_str).map(canonical),
    )).collect::<Vec<_>>();
    let mut expected = Vec::new();
    for (family, pair) in [("inet", &plan.ipv4_tunnel), ("inet6", &plan.ipv6_tunnel)] {
        let pair = pair.as_ref().unwrap();
        let (local, peer) = if side == TunnelEndpointSide::Left {
            (&pair.left, &pair.right)
        } else {
            (&pair.right, &pair.left)
        };
        expected.push((family.to_string(), canonical(local), u64::from(pair.prefix_len), Some(canonical(peer))));
    }
    let (extra, client) = if side == TunnelEndpointSide::Left {
        (&plan.additional_addresses.left, &plan.left_client_id)
    } else {
        (&plan.additional_addresses.right, &plan.right_client_id)
    };
    let generated = vpsman_common::tunnel_generated_link_local(uuid::Uuid::parse_str(id).unwrap(), client);
    for cidr in extra.ipv4.iter().chain(&extra.ipv6).chain(std::iter::once(&generated)) {
        let (local, prefix) = cidr.split_once('/').unwrap();
        let family = if local.parse::<std::net::IpAddr>().unwrap().is_ipv4() { "inet" } else { "inet6" };
        expected.push((family.to_string(), canonical(local), prefix.parse().unwrap(), None));
    }
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected, "{side:?}: exact local/peer/prefix multiplicity");
}

async fn packet_openvpn_identity(id: &str, side: &str, state: &serde_json::Value) -> (u32, u64) {
    let pid = tokio::fs::read_to_string(packet_openvpn_path(id, side, "openvpn.pid"))
        .await.unwrap().trim().parse().unwrap();
    (pid, state["ifindex"].as_u64().unwrap())
}

#[tokio::test]
#[ignore = "requires disposable Docker private networking, SYS_ADMIN for nested namespaces, NET_ADMIN and WireGuard"]
async fn parallel_plans_keep_distinct_stable_link_local_addresses_and_packets() {
    require_isolated_network();
    create_packet_namespaces(false).await;
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
        // Only IPv4 is configured. Generated link-local addresses and the
        // WireGuard peer /128 must still work without permitting all IPv6.
        assert!(plan.ipv6_tunnel.is_none());
        assert!(plan.additional_addresses.is_empty());
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
    builtin_packet_matrix_with_openvpn_upgrade(kinds, fou_kind, false).await;
}

async fn builtin_packet_matrix_with_openvpn_upgrade(
    kinds: &[TunnelKind],
    fou_kind: vpsman_common::RuntimeTunnelFouKind,
    legacy_openvpn: bool,
) {
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
        let legacy_openvpn = fixture["legacy_openvpn"].as_bool().unwrap_or(false);
        if legacy_openvpn {
            assert_eq!(plan.kind, TunnelKind::Openvpn);
            // Launch the actual old generated configuration with the real
            // daemon. exec retains the normal executable/config ownership
            // evidence; only this disposable first-start fixture is altered.
            config.network.runtime_openvpn_argv = vec![
                "/bin/sh".into(), "-c".into(),
                "if [ \"$1\" = --config ]; then sed -i '/^ifconfig-noexec$/d' \"$2\"; fi\nexec /usr/sbin/openvpn \"$@\"".into(),
                "legacy-openvpn-fixture".into(),
            ];
        }
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
        if let Some(restart) = fixture["expect_restart"].as_bool() {
            assert_eq!(report["existing_link_validation"]["config_hash_matches"], !restart, "{report}");
            assert_eq!(report["commands"].as_array().unwrap().iter()
                .any(|step| step["label"] == "runtime_openvpn_start"), restart, "{report}");
        }
        if legacy_openvpn {
            let side_name = if side == TunnelEndpointSide::Left { "left" } else { "right" };
            // An explicit stale applied marker models the pre-upgrade hash.
            // This tests the existing mismatch/restart path, not a duplicate
            // implementation of the production config+credentials hash.
            tokio::fs::write(
                packet_openvpn_path(fixture["id"].as_str().unwrap(), side_name, "applied.sha256"),
                "legacy-generated-config\n",
            ).await.unwrap();
        }
        return;
    }
    for &kind in kinds {
        let id = uuid::Uuid::new_v4().to_string();
        let gre6 = kind.iproute2_underlay_family(fou_kind) == Some(TunnelAddressFamily::Ipv6);
        create_packet_namespaces(gre6).await;
        let mut plan = isolated_plan(kind);
        set_fou_test_kind(&mut plan, fou_kind);
        let carries_ipv4 = packet_test_supports_family(&plan, TunnelAddressFamily::Ipv4);
        let carries_ipv6 = packet_test_supports_family(&plan, TunnelAddressFamily::Ipv6);
        plan.interface_name = "vpspkt".into();
        if gre6 {
            // ip-link also accepts short names that resemble its options.
            plan.interface_name = "t".into();
        }
        plan.left_local_underlay = Some("192.0.2.1".into());
        plan.right_local_underlay = Some("192.0.2.2".into());
        plan.left_remote_underlay = "192.0.2.2".into();
        plan.right_remote_underlay = "192.0.2.1".into();
        if gre6 {
            set_gre6_test_underlay(&mut plan);
        }
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
        if kind == TunnelKind::Openvpn {
            // OpenVPN accepts IPv6 prefixes through /124; exercise a valid
            // dual-stack primary independently of the additional IPv6 aliases.
            plan.ipv6_tunnel = Some(TunnelAddressPair {
                left: "fd00:ffff::".into(), right: "fd00:ffff::1".into(), prefix_len: 124,
            });
            // Daemon output normally goes to syslog, which this image does not
            // collect. Only add endpoint-local logging through the existing
            // override renderer; leave TLS, DCO, MTUs and all addresses intact.
            for (side, overrides) in [
                ("left", &mut plan.runtime_control.openvpn.left_config_override),
                ("right", &mut plan.runtime_control.openvpn.right_config_override),
            ] {
                assert!(overrides.is_none(), "packet fixture already has native overrides");
                let path = packet_openvpn_path(&id, side, "packet-openvpn.log");
                *overrides = Some(format!(
                    "log {}\n",
                    vpsman_common::openvpn_config_path_value(&path).unwrap()
                ));
            }
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
            ("address-left", TunnelEndpointSide::Left, &left_credentials),
            (
                "address-right",
                TunnelEndpointSide::Right,
                &right_credentials,
            ),
        ] {
            reconcile_packet_endpoint_fixture(namespace, &id, &plan, side, credentials.as_ref(), legacy_openvpn, None).await;
        }
        if kind == TunnelKind::Openvpn {
            wait_for_packet_openvpn_tls(&id, &plan.interface_name).await;
            for (namespace, side, side_name, credentials) in [
                ("address-left", TunnelEndpointSide::Left, "left", &left_credentials),
                ("address-right", TunnelEndpointSide::Right, "right", &right_credentials),
            ] {
                let before = packet_openvpn_address_state(namespace, &plan.interface_name).await;
                let identity = packet_openvpn_identity(&id, side_name, &before).await;
                if legacy_openvpn {
                    let local = if side == TunnelEndpointSide::Left {
                        &plan.ipv4_tunnel.as_ref().unwrap().left
                    } else {
                        &plan.ipv4_tunnel.as_ref().unwrap().right
                    };
                    let mut prefixes = before["addr_info"].as_array().unwrap().iter()
                        .filter(|address| address["family"] == "inet" && address["local"] == local.as_str())
                        .map(|address| address["prefixlen"].as_u64().unwrap()).collect::<Vec<_>>();
                    prefixes.sort();
                    assert_eq!(prefixes, [31, 32], "legacy daemon must reproduce the real duplicate");
                } else {
                    assert_packet_openvpn_address_tuples(&before, &id, &plan, side);
                }
                reconcile_packet_endpoint_fixture(namespace, &id, &plan, side, credentials.as_ref(), false, Some(legacy_openvpn)).await;
                let after = packet_openvpn_address_state(namespace, &plan.interface_name).await;
                let after_identity = packet_openvpn_identity(&id, side_name, &after).await;
                if legacy_openvpn {
                    assert_ne!(identity.0, after_identity.0, "upgrade must restart the owned daemon");
                    assert_ne!(identity.1, after_identity.1, "upgrade must replace the old TUN link");
                } else {
                    assert_eq!(identity, after_identity, "unchanged reapply must preserve daemon and link");
                }
                assert_packet_openvpn_address_tuples(&after, &id, &plan, side);
            }
            if !legacy_openvpn {
                // A single dual-stack prefix edit must remove both old primary
                // tuples while retaining extras and the generated link-local.
                plan.ipv4_tunnel.as_mut().unwrap().prefix_len = 30;
                plan.tunnel_prefix_len = 30;
                plan.ipv6_tunnel.as_mut().unwrap().prefix_len = 120;
                for (namespace, side, side_name, credentials) in [
                    ("address-left", TunnelEndpointSide::Left, "left", &left_credentials),
                    ("address-right", TunnelEndpointSide::Right, "right", &right_credentials),
                ] {
                    let before = packet_openvpn_address_state(namespace, &plan.interface_name).await;
                    let identity = packet_openvpn_identity(&id, side_name, &before).await;
                    reconcile_packet_endpoint_fixture(namespace, &id, &plan, side, credentials.as_ref(), false, Some(true)).await;
                    let after = packet_openvpn_address_state(namespace, &plan.interface_name).await;
                    let after_identity = packet_openvpn_identity(&id, side_name, &after).await;
                    assert_ne!(identity.0, after_identity.0, "native config edit must restart the daemon");
                    assert_packet_openvpn_address_tuples(&after, &id, &plan, side);
                }
            }
            wait_for_packet_openvpn_tls(&id, &plan.interface_name).await;
        }
        if gre6 {
            // Reapply real ip6gre JSON and adopt the IPv6 FOU listener without
            // replacing the link or changing its configured addresses.
            for (namespace, side) in [("address-left", TunnelEndpointSide::Left), ("address-right", TunnelEndpointSide::Right)] {
                let before = packet_openvpn_address_state(namespace, &plan.interface_name).await;
                reconcile_packet_endpoint(namespace, &id, &plan, side, None).await;
                let after = packet_openvpn_address_state(namespace, &plan.interface_name).await;
                assert_eq!(before["ifindex"], after["ifindex"]);
                assert_eq!(before["addr_info"], after["addr_info"]);
            }
            // Include default encapsulation-limit overhead at the MTU boundary.
            for (target, header) in [("10.254.20.2", 28), ("fd00:123::2", 48)] {
                checked_native("/sbin/ip", &["netns", "exec", "address-left", "/usr/bin/ping", "-n", "-c", "1", "-W", "2", "-M", "do", "-s", &(plan.left_mtu.unwrap() - header).to_string(), target]).await;
            }
        }
        let mut targets = Vec::new();
        if kind == TunnelKind::Openvpn {
            targets.push(plan.ipv4_tunnel.as_ref().unwrap().right.clone());
            targets.push(plan.ipv6_tunnel.as_ref().unwrap().right.clone());
        }
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
            if !output.status.success() && kind == TunnelKind::Openvpn {
                dump_packet_openvpn_diagnostics(&id, &plan.interface_name).await;
            }
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
