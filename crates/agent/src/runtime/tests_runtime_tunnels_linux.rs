use super::*;

// Exercise the full config-sync entry point with real kernel interfaces. The
// existing command-stub fixtures cannot detect iproute2 output/version effects.
#[tokio::test]
#[ignore = "requires disposable Docker private networking, NET_ADMIN and VPSMAN_TEST_ISOLATED_NETWORK=1"]
async fn runtime_config_sync_add_and_ospf_edit_preserve_existing_gre() {
    config_sync_tunnel_workflow(vpsman_common::TunnelKind::Gre).await;
}

#[tokio::test]
#[ignore = "requires disposable Docker private networking, NET_ADMIN and kernel ip6_gre"]
async fn runtime_config_sync_add_and_ospf_edit_preserve_existing_gre6() {
    config_sync_tunnel_workflow(vpsman_common::TunnelKind::Gre6).await;
}

#[tokio::test]
#[ignore = "requires disposable Docker private networking, NET_ADMIN and kernel fou6"]
async fn runtime_config_sync_add_and_ospf_edit_preserve_existing_fou_gre6() {
    config_sync_tunnel_workflow(vpsman_common::TunnelKind::Fou).await;
}

async fn config_sync_tunnel_workflow(kind: vpsman_common::TunnelKind) {
    assert_eq!(
        std::env::var("VPSMAN_TEST_ISOLATED_NETWORK").as_deref(),
        Ok("1"),
        "run only in a disposable private-network container"
    );
    assert!(std::path::Path::new("/.dockerenv").exists());
    assert_eq!(unsafe { libc::geteuid() }, 0);

    let mut config = AgentConfig {
        client_id: "left-a".into(),
        ..runtime_sync_test_base()
    };
    config.network.apply_enabled = true;
    config.network.runtime_reconcile_enabled = true;
    config.network.root_dir = "/".into();

    let mut first = runtime_sync_test_telemetry_plan(runtime_sync_test_plan(
        "203.0.113.20",
        "10.255.0.0",
        "10.255.0.1",
    ));
    first.plan_id = Some(uuid::Uuid::new_v4().to_string());
    first.plan.kind = kind;
    if kind != vpsman_common::TunnelKind::Gre {
        first.plan.left_local_underlay = Some("fd00:abcd::1".into());
        first.plan.left_remote_underlay = "fd00:abcd::2".into();
        first.plan.right_local_underlay = Some("fd00:abcd::2".into());
        first.plan.right_remote_underlay = "fd00:abcd::1".into();
        if kind == vpsman_common::TunnelKind::Fou {
            first.plan.runtime_control.fou.tunnel_kind = vpsman_common::RuntimeTunnelFouKind::Gre6;
        }
        first.plan.left_mtu = Some(if kind == vpsman_common::TunnelKind::Fou {
            1440
        } else {
            1448
        });
        first.plan.right_mtu = first.plan.left_mtu;
    }
    first.plan.manage_link_local = true;
    first.plan.ipv6_tunnel = Some(vpsman_common::TunnelAddressPair {
        left: "fd00:ffff::".into(),
        right: "fd00:ffff::1".into(),
        prefix_len: 127,
    });
    first.plan.ospf = Some(
        serde_json::from_value(serde_json::json!({
            "planned_latency_ms": 10.0, "planned_packet_loss_ratio": 0.0, "preference": 1.0
        }))
        .unwrap(),
    );
    let mut second = first.clone();
    second.plan_id = Some(uuid::Uuid::new_v4().to_string());
    second.plan.name = "second-link".into();
    second.plan.interface_name = "tunsecond".into();
    // Exercise both endpoint orientations on this VPS, with distinct remotes.
    second.endpoint_side = vpsman_common::TunnelEndpointSide::Right;
    second.plan.right_client_id = config.client_id.clone();
    second.plan.left_client_id = "peer-c".into();
    second.plan.left_tunnel_address = "10.255.0.2".into();
    second.plan.right_tunnel_address = "10.255.0.3".into();
    second.plan.ipv4_tunnel = Some(vpsman_common::TunnelAddressPair {
        left: "10.255.0.2".into(),
        right: "10.255.0.3".into(),
        prefix_len: 31,
    });
    second.plan.ipv6_tunnel = Some(vpsman_common::TunnelAddressPair {
        left: "fd00:ffff::2".into(),
        right: "fd00:ffff::3".into(),
        prefix_len: 127,
    });
    if kind != vpsman_common::TunnelKind::Gre {
        second.plan.left_local_underlay = Some("fd00:abcd::3".into());
        second.plan.left_remote_underlay = "fd00:abcd::1".into();
        second.plan.right_local_underlay = Some("fd00:abcd::1".into());
        second.plan.right_remote_underlay = "fd00:abcd::3".into();
    }
    if kind == vpsman_common::TunnelKind::Fou {
        // Plans still reserve distinct UDP listener ports per VPS.
        second.plan.runtime_control.fou.port = 5556;
        second.plan.runtime_control.fou.peer_port = 5556;
    }

    let mut desired = AgentRuntimeConfig::from_agent_config(1, &config);
    desired.network.runtime_status_telemetry_plans = vec![first.clone()];
    config = apply(&config, desired, "tunnel_plan_saved_enabled", 0).await;
    let first_before = snapshot(&first.plan.interface_name).await;
    assert_eq!(first_before["addresses"].as_array().unwrap().len(), 3);

    let mut desired = AgentRuntimeConfig::from_agent_config(2, &config);
    desired
        .network
        .runtime_status_telemetry_plans
        .push(second.clone());
    config = apply(&config, desired, "tunnel_plan_saved_enabled", 0).await;
    assert_eq!(snapshot(&first.plan.interface_name).await, first_before);
    let second_before = snapshot(&second.plan.interface_name).await;
    assert_eq!(second_before["addresses"].as_array().unwrap().len(), 3);

    let mut desired = AgentRuntimeConfig::from_agent_config(3, &config);
    desired.network.runtime_status_telemetry_plans[0]
        .plan
        .ospf
        .as_mut()
        .unwrap()
        .preference = 2.0;
    config = apply(&config, desired, "tunnel_plan_updated", 0).await;
    assert_eq!(snapshot(&first.plan.interface_name).await, first_before);
    assert_eq!(snapshot(&second.plan.interface_name).await, second_before);

    // Reapplying the accepted document must remain idempotent, too.
    let desired = AgentRuntimeConfig::from_agent_config(4, &config);
    config = apply(&config, desired, "tunnel_plan_updated", 0).await;
    assert_eq!(snapshot(&first.plan.interface_name).await, first_before);
    assert_eq!(snapshot(&second.plan.interface_name).await, second_before);

    if kind != vpsman_common::TunnelKind::Gre {
        // An IPv6 underlay edit replaces only the edited endpoint.
        let mut desired = AgentRuntimeConfig::from_agent_config(5, &config);
        desired.network.runtime_status_telemetry_plans[0]
            .plan
            .left_remote_underlay = "fd00:abcd::9".into();
        config = apply(&config, desired, "tunnel_plan_updated", 1).await;
        assert_ne!(
            snapshot(&first.plan.interface_name).await["ifindex"],
            first_before["ifindex"]
        );
        assert_eq!(snapshot(&second.plan.interface_name).await, second_before);
    }
    if kind == vpsman_common::TunnelKind::Fou {
        // Changing outer family retires the IPv6 listener before creating the
        // IPv4 listener on the same declared port; the other plan stays intact.
        let mut desired = AgentRuntimeConfig::from_agent_config(6, &config);
        let changed = &mut desired.network.runtime_status_telemetry_plans[0].plan;
        changed.runtime_control.fou.tunnel_kind = vpsman_common::RuntimeTunnelFouKind::Gre;
        changed.left_local_underlay = Some("192.0.2.1".into());
        changed.left_remote_underlay = "192.0.2.2".into();
        changed.right_local_underlay = Some("192.0.2.2".into());
        changed.right_remote_underlay = "192.0.2.1".into();
        config = apply(&config, desired, "tunnel_plan_updated", 1).await;
        assert_eq!(snapshot(&second.plan.interface_name).await, second_before);
    }
    let mut desired = AgentRuntimeConfig::from_agent_config(7, &config);
    desired.network.runtime_status_telemetry_plans.clear();
    apply(&config, desired, "tunnel_plan_disabled", 2).await;
    for interface in [&first.plan.interface_name, &second.plan.interface_name] {
        assert!(!std::path::Path::new("/sys/class/net")
            .join(interface)
            .exists());
    }
    if kind == vpsman_common::TunnelKind::Fou {
        let output = tokio::process::Command::new("/sbin/ip")
            .args(["-j", "fou", "show"])
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
            serde_json::json!([])
        );
    }
}

async fn apply(
    config: &AgentConfig,
    desired: AgentRuntimeConfig,
    reason: &str,
    expected_removals: usize,
) -> AgentConfig {
    let result = apply_runtime_config_sync(
        uuid::Uuid::new_v4(),
        config,
        &desired,
        desired.version,
        reason,
        CommandCancelToken::default(),
    )
    .await
    .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
    assert_eq!(report["status"], "applied", "{report}");
    assert_eq!(
        report["removed_tunnel_count"], expected_removals,
        "{report}"
    );
    assert_eq!(
        report["reconcile"]["converged"],
        desired.network.runtime_status_telemetry_plans.len()
    );
    result.applied_config.unwrap()
}

async fn snapshot(interface: &str) -> serde_json::Value {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new("/sbin/ip")
            .args(["-d", "-j", "addr", "show", "dev", interface])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
    let link = records
        .iter()
        .find(|link| link["ifname"] == interface)
        .unwrap();
    let addresses = link["addr_info"]
        .as_array()
        .unwrap()
        .iter()
        .map(|addr| {
            serde_json::json!([
                addr["family"],
                addr["local"],
                addr["address"],
                addr["prefixlen"]
            ])
            .to_string()
        })
        .collect::<std::collections::BTreeSet<_>>();
    serde_json::json!({
        "ifindex": link["ifindex"], "mtu": link["mtu"], "flags": link["flags"],
        "underlay": link["linkinfo"], "addresses": addresses,
    })
}
