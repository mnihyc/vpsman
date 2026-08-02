use super::*;

#[test]
fn socket_tables_count_tcp_and_udp_entries_across_ip_families() {
    let root = std::env::temp_dir().join(format!("vpsman-sockets-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("net")).unwrap();
    let table = |rows: &[&str]| format!("sl local_address rem_address st\n{}\n", rows.join("\n"));
    std::fs::write(root.join("net/tcp"), table(&["0: tcp-a", "1: tcp-b"])).unwrap();
    std::fs::write(root.join("net/tcp6"), table(&["0: tcp6-a"])).unwrap();
    std::fs::write(root.join("net/udp"), table(&["0: udp-a"])).unwrap();
    std::fs::write(root.join("net/udp6"), table(&["0: udp6-a", "1: udp6-b"])).unwrap();

    let counts = connection_stats(&root).unwrap();
    assert_eq!(counts.tcp, 3);
    assert_eq!(counts.udp, 3);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn missing_ipv6_socket_tables_are_not_invented_or_required() {
    let root = std::env::temp_dir().join(format!("vpsman-sockets-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("net")).unwrap();
    std::fs::write(root.join("net/tcp"), "header\n0: tcp\n").unwrap();
    std::fs::write(root.join("net/udp"), "header\n").unwrap();

    let counts = connection_stats(&root).unwrap();
    assert_eq!(counts.tcp, 1);
    assert_eq!(counts.udp, 0);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn ipv6_only_socket_tables_are_counted_without_assuming_ipv4() {
    let root = std::env::temp_dir().join(format!("vpsman-sockets-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("net")).unwrap();
    std::fs::write(root.join("net/tcp6"), "header\n0: tcp6\n").unwrap();
    std::fs::write(root.join("net/udp6"), "header\n0: udp6-a\n1: udp6-b\n").unwrap();

    let counts = connection_stats(&root).unwrap();
    assert_eq!(counts.tcp, 1);
    assert_eq!(counts.udp, 2);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn missing_or_headerless_socket_tables_remain_unavailable() {
    let root = std::env::temp_dir().join(format!("vpsman-sockets-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("net")).unwrap();
    assert!(connection_stats(&root)
        .unwrap_err()
        .to_string()
        .contains("socket tables are missing"));
    std::fs::write(root.join("net/tcp"), "\n").unwrap();
    std::fs::write(root.join("net/udp"), "header\n").unwrap();
    assert!(connection_stats(&root)
        .unwrap_err()
        .to_string()
        .contains("no socket-table header"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn configured_hostname_read_failure_is_not_replaced_by_a_default() {
    let default_called = std::cell::Cell::new(false);
    let error = resolve_hostname(
        Some("/configured/hostname"),
        |_| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
        || {
            default_called.set(true);
            Ok("fallback-hostname".to_string())
        },
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("failed to read configured hostname file /configured/hostname"));
    assert!(!default_called.get());
}

#[test]
fn configured_hostname_must_not_be_empty() {
    let error = resolve_hostname(
        Some("/configured/hostname"),
        |_| Ok(" \n".to_string()),
        || Ok("fallback-hostname".to_string()),
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "configured hostname file /configured/hostname is empty"
    );
}

#[test]
fn missing_default_hostname_is_an_error_instead_of_an_invented_identity() {
    let error = resolve_hostname(
        None,
        |_| unreachable!("no configured hostname file should be read"),
        || anyhow::bail!("system source unavailable"),
    )
    .unwrap_err();

    assert!(format!("{error:#}").contains("system source unavailable"));
    assert!(!format!("{error:#}").contains("unknown"));
}

#[test]
fn default_hostname_uses_the_operating_system_value() {
    let hostname = resolve_hostname(
        None,
        |_| unreachable!("no configured hostname file should be read"),
        || Ok("node-from-os\n".to_string()),
    )
    .unwrap();

    assert_eq!(hostname, "node-from-os");
}

#[test]
fn cpu_utilization_requires_two_valid_samples_and_uses_busy_time() {
    let mut previous = None;
    assert_eq!(
        update_cpu_utilization_ratio(
            &mut previous,
            "cpu 100 20 30 400 10 5 5 0 50 10\ncpu0 1 1 1 1 1 1 1 1\n",
        ),
        None
    );

    let ratio = update_cpu_utilization_ratio(
        &mut previous,
        // The large guest deltas are deliberately ignored because Linux
        // already includes guest time in user and nice.
        "cpu 130 20 40 450 20 5 5 0 500 200\n",
    )
    .unwrap();
    assert!((ratio - 0.4).abs() < f64::EPSILON);
}

#[test]
fn cpu_utilization_reset_and_invalid_input_restart_the_baseline() {
    let mut previous = None;
    assert_eq!(
        update_cpu_utilization_ratio(&mut previous, "cpu 100 20 30 400 10 5 5 0\n",),
        None
    );
    assert_eq!(
        update_cpu_utilization_ratio(&mut previous, "cpu 10 20 30 400 10 5 5 0\n"),
        None
    );
    let after_reset =
        update_cpu_utilization_ratio(&mut previous, "cpu 20 20 30 410 10 5 5 0\n").unwrap();
    assert!((after_reset - 0.5).abs() < f64::EPSILON);

    assert_eq!(
        update_cpu_utilization_ratio(&mut previous, "cpu invalid\n"),
        None
    );
    assert!(previous.is_none());
    assert_eq!(
        update_cpu_utilization_ratio(&mut previous, "cpu 30 20 30 420 10 5 5 0\n",),
        None
    );
}

#[test]
fn cpu_utilization_zero_delta_is_unknown_and_results_stay_bounded() {
    let mut previous = None;
    let baseline = "cpu 0 0 0 0 0 0 0 0\n";
    assert_eq!(update_cpu_utilization_ratio(&mut previous, baseline), None);
    assert_eq!(update_cpu_utilization_ratio(&mut previous, baseline), None);
    assert_eq!(
        update_cpu_utilization_ratio(&mut previous, "cpu 10 0 0 0 0 0 0 0\n"),
        Some(1.0)
    );
    assert_eq!(
        update_cpu_utilization_ratio(&mut previous, "cpu 10 0 0 10 0 0 0 0\n"),
        Some(0.0)
    );
}

#[test]
fn parses_linux_network_counters_without_classifying_tunnels() {
    let stats = network_stats_from_proc_net_dev(
        "Inter-| Receive | Transmit\n\
         face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n\
         eth0: 10 1 0 0 0 0 0 0 20 2 0 0 0 0 0 0\n\
         wg0: 30 3 0 0 0 0 0 0 40 4 0 0 0 0 0 0\n",
    )
    .unwrap();
    assert_eq!(stats.len(), 2);
    assert_eq!(stats[0].interface, "eth0");
    assert_eq!(stats[1].interface, "wg0");
    assert_eq!(stats[1].rx_bytes, 30);
    assert_eq!(stats[1].tx_bytes, 40);
}

#[test]
fn linux_network_collection_stays_within_the_ingest_cardinality_limit() {
    let mut contents = String::from(
        "Inter-| Receive | Transmit\n\
         face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n",
    );
    for index in 0..(MAX_TELEMETRY_NETWORKS + 1) {
        contents.push_str(&format!(
            "veth{index}: {index} 1 0 0 0 0 0 0 {index} 1 0 0 0 0 0 0\n"
        ));
    }

    let stats = network_stats_from_proc_net_dev(&contents).unwrap();
    assert_eq!(stats.len(), MAX_TELEMETRY_NETWORKS);
    assert_eq!(stats.last().unwrap().interface, "veth511");
}

#[test]
fn malformed_linux_network_counters_fail_instead_of_becoming_zero() {
    let error = network_stats_from_proc_net_dev(
        "Inter-| Receive | Transmit\n\
         face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n\
         eth0: broken 1 0 0 0 0 0 0 20 2 0 0 0 0 0 0\n",
    )
    .unwrap_err();
    assert!(error.to_string().contains("RX counter"));
}

#[test]
fn incomplete_meminfo_fails_instead_of_becoming_zero() {
    let root = std::env::temp_dir().join(format!("vpsman-meminfo-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("meminfo"), "MemTotal: 1024 kB\n").unwrap();
    let error = memory_stat(&root).unwrap_err();
    assert!(error.to_string().contains("MemAvailable"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn malformed_mount_inventory_fails_instead_of_disappearing() {
    let root = std::env::temp_dir().join(format!("vpsman-mounts-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("mounts"), "incomplete-row\n").unwrap();
    let error = disk_stats(&root).unwrap_err();
    assert!(error.to_string().contains("row 1 is incomplete"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn decodes_proc_mount_escapes_before_inspection() {
    assert_eq!(
        decode_proc_mount_field("/srv/space\\040and\\011tab\\134dir"),
        "/srv/space and\ttab\\dir"
    );
}

#[test]
fn parses_latency_probe_output_as_observation_only() {
    let parsed = parse_latency_ping_output(
        "3 packets transmitted, 2 received, 33.333% packet loss\n\
         rtt min/avg/max/mdev = 10.0/12.5/15.0/1.0 ms\n",
    );
    assert!(parsed.healthy);
    assert_eq!(parsed.latency_avg_ms, Some(12.5));
    assert!(parsed.packet_loss_ratio.unwrap() > 0.33);
}

#[test]
fn runtime_labels_do_not_imply_discovery_or_routing_mutation() {
    assert_eq!(
        runtime_manager_label(RuntimeTunnelManager::AgentIproute2Managed),
        "agent_iproute2_managed"
    );
    assert_eq!(
        runtime_manager_label(RuntimeTunnelManager::ExternalObserved),
        "external_observed"
    );
    assert_eq!(
        runtime_manager_label(RuntimeTunnelManager::ExternalManagedAdapter),
        "external_managed_adapter"
    );
}
