use super::{
    ordinal_admission_mask_has_exact_shape, AgentMetrics, ConnectionStat, CpuStat, DiskStat,
    MemoryStat, DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1,
};

#[test]
fn ordinal_admission_masks_require_exact_length_and_zero_unused_bits() {
    assert!(ordinal_admission_mask_has_exact_shape(&[], 0));
    assert!(ordinal_admission_mask_has_exact_shape(&[0b1111_1111], 8));
    assert!(ordinal_admission_mask_has_exact_shape(&[0b0000_0111], 3));
    assert!(ordinal_admission_mask_has_exact_shape(
        &[0b1111_1111, 0b0000_0001],
        9
    ));

    assert!(!ordinal_admission_mask_has_exact_shape(&[], 1));
    assert!(!ordinal_admission_mask_has_exact_shape(&[0], 9));
    assert!(!ordinal_admission_mask_has_exact_shape(&[0, 0], 8));
    assert!(!ordinal_admission_mask_has_exact_shape(&[0b1000_0111], 3));
}

#[test]
fn cpu_utilization_is_an_optional_additive_wire_field() {
    let without_utilization: CpuStat = serde_json::from_value(serde_json::json!({
        "load": {"one": 0.5, "five": 0.4, "fifteen": 0.3},
        "cores": 2
    }))
    .unwrap();
    assert_eq!(without_utilization.utilization_ratio, None);
    assert!(serde_json::to_value(&without_utilization)
        .unwrap()
        .get("utilization_ratio")
        .is_none());

    let with_utilization: CpuStat = serde_json::from_value(serde_json::json!({
        "load": {"one": 0.5, "five": 0.4, "fifteen": 0.3},
        "cores": 2,
        "utilization_ratio": 0.75
    }))
    .unwrap();
    assert_eq!(with_utilization.utilization_ratio, Some(0.75));
}

#[test]
fn socket_counts_are_atomic_and_missing_is_not_zero() {
    let mut metrics = AgentMetrics::default();
    assert!(serde_json::to_value(&metrics)
        .unwrap()
        .get("connections")
        .is_none());
    metrics.connections = Some(ConnectionStat { tcp: 18, udp: 4 });
    let value = serde_json::to_value(&metrics).unwrap();
    assert_eq!(value["connections"]["tcp"], 18);
    assert_eq!(value["connections"]["udp"], 4);
}

#[test]
fn swap_capacity_is_optional_and_atomic_on_the_wire() {
    let legacy: MemoryStat = serde_json::from_value(serde_json::json!({
        "total_bytes": 4096,
        "available_bytes": 2048
    }))
    .unwrap();
    assert_eq!(legacy.swap_total_bytes, None);
    assert_eq!(legacy.swap_available_bytes, None);
    let legacy_value = serde_json::to_value(&legacy).unwrap();
    assert!(legacy_value.get("swap_total_bytes").is_none());
    assert!(legacy_value.get("swap_available_bytes").is_none());

    let with_swap: MemoryStat = serde_json::from_value(serde_json::json!({
        "total_bytes": 4096,
        "available_bytes": 2048,
        "swap_total_bytes": 1024,
        "swap_available_bytes": 768
    }))
    .unwrap();
    assert_eq!(with_swap.swap_total_bytes, Some(1024));
    assert_eq!(with_swap.swap_available_bytes, Some(768));
}

#[test]
fn disk_collection_presence_requires_explicit_versioned_semantics() {
    let legacy_with_disks: AgentMetrics = serde_json::from_value(serde_json::json!({
        "observed_unix": 1,
        "hostname": "legacy",
        "uptime_secs": 1,
        "cpu": {"load": {"one": 0.0, "five": 0.0, "fifteen": 0.0}, "cores": 1},
        "memory": {"total_bytes": 1, "available_bytes": 1},
        "disks": [{"mountpoint": "/", "total_bytes": 1, "available_bytes": 1}],
        "networks": []
    }))
    .unwrap();
    assert!(!legacy_with_disks.has_persistent_block_filesystem_disk_sample());

    let mut current = AgentMetrics {
        disks: vec![DiskStat {
            mountpoint: "/".to_string(),
            total_bytes: 1,
            available_bytes: 1,
        }],
        disk_collection_available: Some(true),
        disk_semantics: Some(DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1.to_string()),
        ..AgentMetrics::default()
    };
    assert!(current.has_persistent_block_filesystem_disk_sample());

    current.disk_collection_available = Some(false);
    assert!(!current.has_persistent_block_filesystem_disk_sample());
    current.disk_collection_available = Some(true);
    current.disk_semantics = Some("future_semantics".to_string());
    assert!(!current.has_persistent_block_filesystem_disk_sample());
}
