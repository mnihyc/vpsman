use super::{AgentMetrics, ConnectionStat, CpuStat};

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
