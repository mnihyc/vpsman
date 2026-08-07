use super::*;

#[test]
fn tunnel_traffic_uses_live_interface_counters() {
    let traffic = traffic_accumulation_for_interface(
        "wg0",
        Some(NetworkStat {
            interface: "wg0".to_string(),
            rx_bytes: 1234,
            tx_bytes: 5678,
        }),
    );

    assert_eq!(traffic.rx_bytes, 1234);
    assert_eq!(traffic.tx_bytes, 5678);
    assert_eq!(traffic.source, "interface_counters");
    assert_eq!(traffic.status, "ok");
    assert_eq!(traffic.reason, None);
}

#[test]
fn missing_managed_interface_is_reported_without_external_accounting() {
    let traffic = traffic_accumulation_for_interface("wg0", None);

    assert_eq!(traffic.rx_bytes, 0);
    assert_eq!(traffic.tx_bytes, 0);
    assert_eq!(traffic.source, "interface_counters");
    assert_eq!(traffic.status, "missing");
    assert_eq!(traffic.reason.as_deref(), Some("wg0_not_found"));
}
