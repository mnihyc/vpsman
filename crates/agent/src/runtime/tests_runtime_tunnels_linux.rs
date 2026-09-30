use super::*;

// Exercise the full config-sync entry point with real kernel interfaces. The
// existing command-stub fixtures cannot detect iproute2 output/version effects.
#[tokio::test]
#[ignore = "requires disposable Docker private networking, NET_ADMIN and VPSMAN_TEST_ISOLATED_NETWORK=1"]
async fn runtime_config_sync_add_and_ospf_edit_preserve_existing_gre() {
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

    let mut desired = AgentRuntimeConfig::from_agent_config(1, &config);
    desired.network.runtime_status_telemetry_plans = vec![first.clone()];
    config = apply(&config, desired, "tunnel_plan_saved_enabled").await;
    let first_before = snapshot(&first.plan.interface_name).await;
    assert_eq!(first_before["addresses"].as_array().unwrap().len(), 3);

    let mut desired = AgentRuntimeConfig::from_agent_config(2, &config);
    desired
        .network
        .runtime_status_telemetry_plans
        .push(second.clone());
    config = apply(&config, desired, "tunnel_plan_saved_enabled").await;
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
    config = apply(&config, desired, "tunnel_plan_updated").await;
    assert_eq!(snapshot(&first.plan.interface_name).await, first_before);
    assert_eq!(snapshot(&second.plan.interface_name).await, second_before);

    // Reapplying the accepted document must remain idempotent, too.
    let desired = AgentRuntimeConfig::from_agent_config(4, &config);
    apply(&config, desired, "tunnel_plan_updated").await;
    assert_eq!(snapshot(&first.plan.interface_name).await, first_before);
    assert_eq!(snapshot(&second.plan.interface_name).await, second_before);
}

async fn apply(config: &AgentConfig, desired: AgentRuntimeConfig, reason: &str) -> AgentConfig {
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
    assert_eq!(report["removed_tunnel_count"], 0, "{report}");
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
