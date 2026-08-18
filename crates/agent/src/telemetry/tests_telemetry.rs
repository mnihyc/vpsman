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
fn linux_memory_collection_reports_swap_as_optional_capacity() {
    let root = std::env::temp_dir().join(format!("vpsman-meminfo-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("meminfo"),
        "MemTotal: 4096 kB\nMemAvailable: 1024 kB\nSwapTotal: 2048 kB\nSwapFree: 1536 kB\n",
    )
    .unwrap();

    let memory = memory_stat(&root).unwrap();
    assert_eq!(memory.total_bytes, 4096 * 1024);
    assert_eq!(memory.available_bytes, 1024 * 1024);
    assert_eq!(memory.swap_total_bytes, Some(2048 * 1024));
    assert_eq!(memory.swap_available_bytes, Some(1536 * 1024));

    std::fs::write(
        root.join("meminfo"),
        "MemTotal: 4096 kB\nMemAvailable: 1024 kB\n",
    )
    .unwrap();
    let without_swap = memory_stat(&root).unwrap();
    assert_eq!(without_swap.swap_total_bytes, None);
    assert_eq!(without_swap.swap_available_bytes, None);

    std::fs::write(
        root.join("meminfo"),
        "MemTotal: 4096 kB\nMemAvailable: 1024 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n",
    )
    .unwrap();
    let without_swap_capacity = memory_stat(&root).unwrap();
    assert_eq!(without_swap_capacity.swap_total_bytes, Some(0));
    assert_eq!(without_swap_capacity.swap_available_bytes, Some(0));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn linux_memory_collection_keeps_core_memory_when_swap_evidence_is_invalid() {
    let root = std::env::temp_dir().join(format!("vpsman-meminfo-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let overflow_kib = u64::MAX / 1024 + 1;
    let invalid_swap_cases = vec![
        "SwapTotal: 2048 kB\n".to_string(),
        "SwapFree: 1536 kB\n".to_string(),
        "SwapTotal: 1024 kB\nSwapFree: 2048 kB\n".to_string(),
        "SwapTotal: invalid kB\nSwapFree: 1536 kB\n".to_string(),
        "SwapTotal: 2048 bytes\nSwapFree: 1536 kB\n".to_string(),
        format!("SwapTotal: {overflow_kib} kB\nSwapFree: 1536 kB\n"),
    ];

    for swap in invalid_swap_cases {
        std::fs::write(
            root.join("meminfo"),
            format!("MemTotal: 4096 kB\nMemAvailable: 1024 kB\n{swap}"),
        )
        .unwrap();
        let memory = memory_stat(&root).unwrap();
        assert_eq!(memory.total_bytes, 4096 * 1024);
        assert_eq!(memory.available_bytes, 1024 * 1024);
        assert_eq!(memory.swap_total_bytes, None);
        assert_eq!(memory.swap_available_bytes, None);
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn connection_host_facts_use_configured_proc_and_sys_evidence() {
    let root = std::env::temp_dir().join(format!("vpsman-host-facts-{}", uuid::Uuid::new_v4()));
    let proc_root = root.join("proc");
    let sys_root = root.join("sys");
    std::fs::create_dir_all(proc_root.join("sys/kernel")).unwrap();
    std::fs::create_dir_all(sys_root.join("class/net")).unwrap();
    std::fs::create_dir_all(sys_root.join("class/dmi/id")).unwrap();
    std::fs::write(
        proc_root.join("cpuinfo"),
        "processor : 0\nmodel name : Example   Cloud CPU  3.20GHz\n",
    )
    .unwrap();
    std::fs::write(proc_root.join("sys/kernel/osrelease"), "6.12.3-test\n").unwrap();
    std::fs::write(sys_root.join("class/dmi/id/sys_vendor"), "QEMU\n").unwrap();
    std::fs::write(sys_root.join("class/dmi/id/product_name"), "KVM\n").unwrap();
    let mut config = AgentConfig::default();
    config.telemetry.proc_root = proc_root.to_string_lossy().into_owned();
    config.telemetry.sys_class_net_dir = sys_root.join("class/net").to_string_lossy().into_owned();

    let facts = collect_connection_host_facts(&config);
    assert_eq!(
        facts.cpu_model.as_deref(),
        Some("Example Cloud CPU 3.20GHz")
    );
    assert_eq!(facts.kernel_release.as_deref(), Some("6.12.3-test"));
    assert_eq!(facts.virtualization.as_deref(), Some("kvm"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn host_fact_parsing_does_not_invent_cpu_or_virtualization_labels() {
    assert_eq!(parse_cpu_model("processor : 0\nprocessor : 1\n"), None);
    assert_eq!(
        parse_cpu_model("Hardware : ARM Neoverse-N1\n"),
        Some("ARM Neoverse-N1".to_string())
    );
    assert_eq!(classify_virtualization("Acme Bare Metal Server"), None);
    assert_eq!(classify_virtualization("Amazon EC2 OpenStack Nova"), None);
    assert_eq!(
        classify_virtualization("Microsoft Corporation Virtual Machine"),
        Some("hyper-v")
    );
}

#[test]
fn malformed_mount_row_does_not_discard_available_storage() {
    let root = std::env::temp_dir().join(format!("vpsman-mounts-{}", uuid::Uuid::new_v4()));
    let storage = root.join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    std::fs::write(
        root.join("mounts"),
        format!(
            "incomplete-row\n/dev/root {} ext4 rw 0 0\n",
            storage.display()
        ),
    )
    .unwrap();

    let collection = disk_stats(&root).unwrap();
    assert_eq!(collection.disks.len(), 1);
    assert_eq!(collection.disks[0].mountpoint, storage.to_string_lossy());
    assert_eq!(collection.failure_count, 1);
    assert!(collection
        .first_error
        .as_deref()
        .is_some_and(|error| error.contains("row 1 is incomplete")));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn container_namespace_and_overlay_mounts_are_not_storage() {
    let root = std::env::temp_dir().join(format!("vpsman-nsfs-{}", uuid::Uuid::new_v4()));
    let storage = root.join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    std::fs::write(
        root.join("mounts"),
        format!(
            "nsfs /run/docker/netns/808d0d1218c8 nsfs rw 0 0\n\
             fuse-overlayfs /var/lib/docker/fuse-overlayfs/layer fuse-overlayfs rw 0 0\n\
             /dev/fuse /var/lib/docker/fuse-overlayfs/other fuse.fuse-overlayfs rw 0 0\n\
             /dev/root {} ext4 rw 0 0\n",
            storage.display()
        ),
    )
    .unwrap();

    let collection = disk_stats(&root).unwrap();
    assert_eq!(collection.disks.len(), 1);
    assert_eq!(collection.disks[0].mountpoint, storage.to_string_lossy());
    assert_eq!(collection.failure_count, 0);
    assert_eq!(collection.first_error, None);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn inaccessible_storage_mount_does_not_discard_available_storage() {
    let root = std::env::temp_dir().join(format!("vpsman-partial-disks-{}", uuid::Uuid::new_v4()));
    let storage = root.join("storage");
    let missing = root.join("missing");
    std::fs::create_dir_all(&storage).unwrap();
    std::fs::write(
        root.join("mounts"),
        format!(
            "/dev/root {} ext4 rw 0 0\n/dev/root {} ext4 rw 0 0\n",
            missing.display(),
            storage.display()
        ),
    )
    .unwrap();

    let collection = disk_stats(&root).unwrap();
    assert_eq!(collection.disks.len(), 1);
    assert_eq!(collection.disks[0].mountpoint, storage.to_string_lossy());
    assert_eq!(collection.failure_count, 1);
    assert!(collection
        .first_error
        .as_deref()
        .is_some_and(|error| error.contains(missing.to_string_lossy().as_ref())));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn unprivileged_permission_denied_mounts_do_not_raise_warning_state() {
    let permission_error = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EACCES))
        .context("failed to inspect telemetry mount /restricted");
    assert!(error_is_permission_denied(&permission_error));
    let missing_error = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENOENT))
        .context("failed to inspect telemetry mount /vanished");
    assert!(!error_is_permission_denied(&missing_error));

    let mut collection = DiskCollection::default();
    collection.record_failure(format!("{permission_error:#}"), true);
    assert_eq!(collection.warning_summary(1_000), None);
    let root_warning = collection.warning_summary(0).unwrap();
    assert_eq!(root_warning.0, 1);
    assert!(root_warning.1.contains("/restricted"));

    collection.record_failure(format!("{missing_error:#}"), false);
    let unprivileged_warning = collection.warning_summary(1_000).unwrap();
    assert_eq!(unprivileged_warning.0, 1);
    assert!(unprivileged_warning.1.contains("/vanished"));
}

#[test]
fn unavailable_disk_inventory_keeps_other_linux_telemetry_available() {
    let root =
        std::env::temp_dir().join(format!("vpsman-partial-telemetry-{}", uuid::Uuid::new_v4()));
    let proc_root = root.join("proc");
    let hostname_path = root.join("hostname");
    std::fs::create_dir_all(proc_root.join("net")).unwrap();
    std::fs::write(&hostname_path, "partial-host\n").unwrap();
    std::fs::write(proc_root.join("uptime"), "123.50 50.00\n").unwrap();
    std::fs::write(proc_root.join("loadavg"), "0.25 0.50 0.75 1/10 1\n").unwrap();
    std::fs::write(
        proc_root.join("meminfo"),
        "MemTotal: 4096 kB\nMemAvailable: 1024 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n",
    )
    .unwrap();
    std::fs::write(
        proc_root.join("net/dev"),
        "Inter-| Receive | Transmit\n\
         face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n\
         eth0: 10 1 0 0 0 0 0 0 20 2 0 0 0 0 0 0\n",
    )
    .unwrap();

    let mut config = AgentConfig::default();
    config.telemetry.proc_root = proc_root.to_string_lossy().into_owned();
    config.telemetry.hostname_file = Some(hostname_path.to_string_lossy().into_owned());
    let mut runtime_state = TelemetryRuntimeState::default();

    let metrics = collect_linux_metrics(&config, &mut runtime_state).unwrap();
    assert_eq!(metrics.hostname, "partial-host");
    assert_eq!(metrics.uptime_secs, 123);
    assert_eq!(metrics.cpu.load.one, 0.25);
    assert_eq!(metrics.memory.total_bytes, 4096 * 1024);
    assert_eq!(metrics.networks.len(), 1);
    assert!(metrics.disks.is_empty());
    assert!(runtime_state.disk_collection_failed);

    let storage = root.join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    std::fs::write(
        proc_root.join("mounts"),
        format!("/dev/root {} ext4 rw 0 0\n", storage.display()),
    )
    .unwrap();
    let recovered = collect_linux_metrics(&config, &mut runtime_state).unwrap();
    assert_eq!(recovered.disks.len(), 1);
    assert!(!runtime_state.disk_collection_failed);
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
    let parsed = parse_ping_measurement(
        "3 packets transmitted, 2 received, 33.333% packet loss\n\
         rtt min/avg/max/mdev = 10.0/12.5/15.0/1.0 ms\n",
    );
    assert!(parsed.healthy);
    assert_eq!(parsed.latency_avg_ms, Some(12.5));
    assert!(parsed.packet_loss_ratio > 0.33);
}

#[test]
fn failed_fallback_preserves_one_coherent_primary_evidence_tuple() {
    let primary = LatencyProbeResult {
        family: TunnelAddressFamily::Ipv4,
        target: "10.0.0.2".to_string(),
        healthy: false,
        transmitted: 3,
        received: 1,
        latency_min_ms: Some(10.0),
        latency_avg_ms: Some(10.0),
        latency_max_ms: Some(10.0),
        latency_mdev_ms: Some(0.0),
        packet_loss_ratio: 2.0 / 3.0,
        reason: Some("latency_probe_exit:Some(1):configured".to_string()),
    };
    let fallback = LatencyProbeResult {
        family: TunnelAddressFamily::Ipv6,
        target: "fd00::2".to_string(),
        healthy: false,
        transmitted: 3,
        received: 0,
        latency_min_ms: None,
        latency_avg_ms: None,
        latency_max_ms: None,
        latency_mdev_ms: None,
        packet_loss_ratio: 1.0,
        reason: Some("latency_probe_exit:Some(1):configured".to_string()),
    };

    let retained_after_error =
        retain_primary_after_fallback_error(primary.clone(), TunnelAddressFamily::Ipv6);
    let retained = retain_primary_after_unhealthy_fallback(primary, fallback);
    for evidence in [&retained, &retained_after_error] {
        assert_eq!(evidence.family, TunnelAddressFamily::Ipv4);
        assert_eq!(evidence.target, "10.0.0.2");
        assert_eq!(evidence.transmitted, 3);
        assert_eq!(evidence.received, 1);
        assert_eq!(evidence.packet_loss_ratio, 2.0 / 3.0);
        assert_eq!(evidence.latency_avg_ms, Some(10.0));
    }
    assert_eq!(
        retained.reason.as_deref(),
        Some("primary_ipv4_and_fallback_ipv6_unhealthy")
    );
    assert_eq!(
        retained_after_error.reason.as_deref(),
        Some("primary_ipv4_unhealthy_and_fallback_ipv6_probe_failed")
    );
}

#[test]
fn runtime_status_and_latency_checks_keep_independent_cadences() {
    let key = "plan:topology:left";
    let mut state = TelemetryRuntimeState::default();
    state.last_adapter_check_unix.insert(key.to_string(), 100);
    state.last_latency_check_unix.insert(key.to_string(), 100);
    state.cached_adapter_tunnels.insert(
        key.to_string(),
        RuntimeTunnelStat {
            latency_monitoring_enabled: Some(true),
            ..RuntimeTunnelStat::default()
        },
    );

    assert_eq!(
        runtime_status_checks_due(&state, key, 115, 15, 3_600, true),
        (true, false),
        "a fast status cadence must not accelerate the Ping cadence"
    );
    assert_eq!(
        runtime_status_checks_due(&state, key, 115, 3_600, 15, true),
        (false, true),
        "a fast Ping cadence must not accelerate adapter status checks"
    );
    assert_eq!(
        runtime_status_checks_due(&state, key, 115, 3_600, 3_600, false),
        (false, true),
        "disabling monitoring must clear a cached enabled state immediately"
    );
}

#[test]
fn runtime_status_merge_retains_both_plan_evidence_identities() {
    let mut metrics = AgentMetrics {
        tunnels: vec![RuntimeTunnelStat {
            interface: "guard0".to_string(),
            source: "interface_inventory".to_string(),
            ..RuntimeTunnelStat::default()
        }],
        ..AgentMetrics::default()
    };
    merge_runtime_status_tunnel(
        &mut metrics,
        RuntimeTunnelStat {
            interface: "guard0".to_string(),
            source: "runtime_status".to_string(),
            plan_id: Some("00000000-0000-4000-8000-000000000001".to_string()),
            topology_identity_hash: Some("a".repeat(64)),
            runtime_evidence_identity_hash: Some("b".repeat(64)),
            ..RuntimeTunnelStat::default()
        },
    );

    assert_eq!(metrics.tunnels.len(), 1);
    assert_eq!(
        metrics.tunnels[0].topology_identity_hash.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(
        metrics.tunnels[0].runtime_evidence_identity_hash.as_deref(),
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
    );
}

#[test]
fn removed_runtime_plan_cannot_reuse_cached_state_when_readded() {
    let key = "plan:topology:left";
    let mut state = TelemetryRuntimeState::default();
    state.last_adapter_check_unix.insert(key.to_string(), 100);
    state.last_latency_check_unix.insert(key.to_string(), 100);
    state
        .cached_adapter_tunnels
        .insert(key.to_string(), RuntimeTunnelStat::default());
    state
        .latency_monitors
        .insert(key.to_string(), LatencyMonitorState::default());

    retain_runtime_status_state_for_plans(&mut state, &[]);

    assert!(state.last_adapter_check_unix.is_empty());
    assert!(state.last_latency_check_unix.is_empty());
    assert!(state.cached_adapter_tunnels.is_empty());
    assert!(state.latency_monitors.is_empty());
    assert_eq!(
        runtime_status_checks_due(&state, key, 101, 3_600, 3_600, true),
        (true, true),
        "readding a removed plan must perform fresh status and latency checks"
    );
}

#[test]
fn runtime_status_cache_key_is_scoped_to_topology_and_endpoint() {
    let plan = vpsman_common::plan_tunnel(&vpsman_common::TunnelPlanInput {
        name: "left-right".to_string(),
        interface_name: "tunlr".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        left_mtu: Some(1476),
        right_mtu: Some(1476),
        ospf: None,
    })
    .unwrap();
    let base = AgentRuntimeStatusTelemetryPlan {
        plan_id: Some("00000000-0000-4000-8000-000000000001".to_string()),
        topology_identity_hash: "a".repeat(64),
        runtime_evidence_identity_hash: "c".repeat(64),
        endpoint_side: TunnelEndpointSide::Left,
        plan,
        builtin_credentials: None,
        runtime_adapter: None,
        latency_monitoring_enabled: true,
    };
    let mut changed_topology = base.clone();
    changed_topology.topology_identity_hash = "b".repeat(64);
    let mut changed_runtime = base.clone();
    changed_runtime.runtime_evidence_identity_hash = "d".repeat(64);
    let mut other_side = base.clone();
    other_side.endpoint_side = TunnelEndpointSide::Right;

    assert_ne!(
        runtime_status_telemetry_key(&base),
        runtime_status_telemetry_key(&changed_topology)
    );
    assert_ne!(
        runtime_status_telemetry_key(&base),
        runtime_status_telemetry_key(&changed_runtime)
    );
    assert_ne!(
        runtime_status_telemetry_key(&base),
        runtime_status_telemetry_key(&other_side)
    );
}

#[test]
fn runtime_labels_do_not_imply_discovery_or_routing_mutation() {
    assert_eq!(
        runtime_manager_label(RuntimeTunnelManager::AgentBuiltin),
        "agent_builtin"
    );
    assert_eq!(
        runtime_manager_label(RuntimeTunnelManager::ExternalObserved),
        "external_observed"
    );
    assert_eq!(
        runtime_manager_label(RuntimeTunnelManager::CustomAdapter),
        "custom_adapter"
    );
}
