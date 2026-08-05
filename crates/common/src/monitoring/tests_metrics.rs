use super::{AgentMetrics, ConnectionStat, CpuStat, MemoryStat};

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
