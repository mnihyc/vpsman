use super::*;

#[test]
fn renders_custom_metrics_placeholders() {
    let config = AgentConfig {
        client_id: "edge-a".to_string(),
        ..AgentConfig::default()
    };
    let argv = render_custom_metrics_argv(
        &config,
        &RuntimeTunnelCommand {
            argv: vec!["/opt/vpsman/metrics".to_string(), "{client_id}".to_string()],
            ..RuntimeTunnelCommand::default()
        },
    )
    .unwrap();

    assert_eq!(
        argv,
        vec!["/opt/vpsman/metrics".to_string(), "edge-a".to_string(),]
    );
}

#[test]
fn rejects_removed_custom_metrics_identity_placeholders() {
    for placeholder in ["{display_name}", "{tags_csv}"] {
        let error = render_custom_metrics_argv(
            &AgentConfig::default(),
            &RuntimeTunnelCommand {
                argv: vec!["/opt/vpsman/metrics".to_string(), placeholder.to_string()],
                ..RuntimeTunnelCommand::default()
            },
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("removed server identity placeholder"));
    }
}

#[test]
fn custom_patch_rejects_empty_and_invalid_overlay_values() {
    assert!(
        validate_custom_metrics_patch(&CustomMetricsPatch::default())
            .unwrap_err()
            .to_string()
            .contains("patch is empty")
    );

    let hostname_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        hostname: Some(" \t ".to_string()),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(hostname_error.contains("invalid hostname"));

    let cores_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        cpu: Some(CpuPatch {
            cores: Some(0),
            ..CpuPatch::default()
        }),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(cores_error.contains("invalid cpu.cores"));

    let load_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        cpu: Some(CpuPatch {
            load: Some(LoadAverage {
                one: -0.1,
                five: 0.0,
                fifteen: 0.0,
            }),
            cores: None,
            utilization_ratio: None,
        }),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(load_error.contains("invalid cpu.load"));

    let memory_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        memory: Some(MemoryStat {
            total_bytes: 100,
            available_bytes: 101,
            swap_total_bytes: None,
            swap_available_bytes: None,
        }),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(memory_error.contains("invalid memory"));

    let partial_swap_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        memory: Some(MemoryStat {
            total_bytes: 100,
            available_bytes: 50,
            swap_total_bytes: Some(20),
            swap_available_bytes: None,
        }),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(partial_swap_error.contains("invalid memory"));
}

#[test]
fn custom_patch_rejects_over_cardinality_arrays() {
    let disks_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        disks: Some(vec![DiskStat::default(); MAX_TELEMETRY_DISKS + 1]),
        disk_collection_available: Some(true),
        disk_semantics: Some(DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1.to_string()),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(disks_error.contains("too many disks"));

    let networks_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        networks: Some(vec![NetworkStat::default(); MAX_TELEMETRY_NETWORKS + 1]),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(networks_error.contains("too many networks"));

    let tunnels_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        tunnels: Some(vec![
            RuntimeTunnelStat::default();
            MAX_TELEMETRY_TUNNELS + 1
        ]),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(tunnels_error.contains("too many tunnels"));
}

#[test]
fn custom_disk_replacement_marks_the_trusted_persistent_filesystem_contract() {
    let mut metrics = AgentMetrics::default();
    apply_patch(
        &mut metrics,
        CustomMetricsPatch {
            disks: Some(Vec::new()),
            disk_collection_available: Some(true),
            disk_semantics: Some(DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1.to_string()),
            ..CustomMetricsPatch::default()
        },
    );

    assert_eq!(metrics.disk_collection_available, Some(true));
    assert_eq!(
        metrics.disk_semantics.as_deref(),
        Some(DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1)
    );
    assert!(metrics.has_persistent_block_filesystem_disk_sample());
}

#[test]
fn custom_patch_rejects_invalid_collection_rows() {
    let disk_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        disks: Some(vec![DiskStat {
            mountpoint: "/".to_string(),
            total_bytes: 100,
            available_bytes: 101,
        }]),
        disk_collection_available: Some(true),
        disk_semantics: Some(DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1.to_string()),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(disk_error.contains("invalid disk"));

    let duplicate_disk_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        disks: Some(vec![
            DiskStat {
                mountpoint: "/data".to_string(),
                total_bytes: 100,
                available_bytes: 50,
            },
            DiskStat {
                mountpoint: "/data".to_string(),
                total_bytes: 200,
                available_bytes: 100,
            },
        ]),
        disk_collection_available: Some(true),
        disk_semantics: Some(DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1.to_string()),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(duplicate_disk_error.contains("duplicate disk"));

    let network = NetworkStat {
        interface: "eth0".to_string(),
        rx_bytes: 1,
        tx_bytes: 2,
    };
    let network_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        networks: Some(vec![network.clone(), network]),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(network_error.contains("duplicate network"));

    let tunnel_error = validate_custom_metrics_patch(&CustomMetricsPatch {
        tunnels: Some(vec![RuntimeTunnelStat {
            interface: "wg0".to_string(),
            packet_loss_ratio: Some(1.1),
            ..RuntimeTunnelStat::default()
        }]),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(tunnel_error.contains("invalid tunnel"));
}

#[test]
fn custom_disk_contract_requires_explicit_versioned_presence() {
    let legacy = CustomMetricsPatch {
        disks: Some(vec![DiskStat {
            mountpoint: "/".to_string(),
            total_bytes: 100,
            available_bytes: 50,
        }]),
        ..CustomMetricsPatch::default()
    };
    validate_custom_metrics_patch(&legacy).unwrap();
    let mut legacy_metrics = AgentMetrics::default();
    apply_patch(&mut legacy_metrics, legacy);
    assert_eq!(legacy_metrics.disk_collection_available, None);
    assert_eq!(legacy_metrics.disk_semantics, None);
    assert!(!legacy_metrics.has_persistent_block_filesystem_disk_sample());

    let unsupported = validate_custom_metrics_patch(&CustomMetricsPatch {
        disks: Some(Vec::new()),
        disk_collection_available: Some(true),
        disk_semantics: Some("legacy_mount_scan".to_string()),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(unsupported.contains("unsupported disk_semantics"));

    let incoherent = validate_custom_metrics_patch(&CustomMetricsPatch {
        disks: Some(vec![DiskStat {
            mountpoint: "/".to_string(),
            total_bytes: 100,
            available_bytes: 50,
        }]),
        disk_collection_available: Some(false),
        disk_semantics: Some(DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1.to_string()),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(incoherent.contains("cannot contain disks"));

    let mut metrics = AgentMetrics::default();
    apply_patch(
        &mut metrics,
        CustomMetricsPatch {
            disks: Some(Vec::new()),
            disk_collection_available: Some(false),
            disk_semantics: Some(DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1.to_string()),
            ..CustomMetricsPatch::default()
        },
    );
    assert!(metrics.disks.is_empty());
    assert_eq!(metrics.disk_collection_available, Some(false));
    assert_eq!(
        metrics.disk_semantics.as_deref(),
        Some(DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1)
    );
}

#[test]
fn custom_overlay_accepts_valid_partial_metrics() {
    validate_custom_metrics_patch(&CustomMetricsPatch {
        cpu: Some(CpuPatch {
            load: Some(LoadAverage {
                one: 0.5,
                five: 0.4,
                fifteen: 0.3,
            }),
            cores: None,
            utilization_ratio: Some(0.25),
        }),
        networks: Some(Vec::new()),
        ..CustomMetricsPatch::default()
    })
    .unwrap();
}

#[test]
fn custom_cpu_utilization_is_optional_but_must_be_a_ratio() {
    let snapshot = empty_custom_metrics_snapshot(1);
    assert_eq!(snapshot.cpu.utilization_ratio, None);

    let mut metrics = snapshot;
    apply_patch(
        &mut metrics,
        CustomMetricsPatch {
            cpu: Some(CpuPatch {
                utilization_ratio: Some(0.75),
                ..CpuPatch::default()
            }),
            ..CustomMetricsPatch::default()
        },
    );
    assert_eq!(metrics.cpu.utilization_ratio, Some(0.75));

    let error = validate_custom_metrics_patch(&CustomMetricsPatch {
        cpu: Some(CpuPatch {
            utilization_ratio: Some(1.01),
            ..CpuPatch::default()
        }),
        ..CustomMetricsPatch::default()
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("invalid cpu.utilization_ratio"));
}

#[test]
fn custom_patch_rejects_unknown_fields() {
    let error = serde_json::from_str::<CustomMetricsPatch>(
        r#"{"hostname":"edge-a","unexpected_metric":1}"#,
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn custom_replacement_adds_completeness_to_shared_validation() {
    let patch = CustomMetricsPatch {
        hostname: Some("edge-a".to_string()),
        ..CustomMetricsPatch::default()
    };
    validate_custom_metrics_patch(&patch).unwrap();
    let error = validate_complete_custom_metrics_patch(&patch)
        .unwrap_err()
        .to_string();
    assert!(error.contains("uptime_secs"));
}
