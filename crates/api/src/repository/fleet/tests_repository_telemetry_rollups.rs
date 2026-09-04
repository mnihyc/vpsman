use super::*;

#[test]
fn selected_network_history_aggregation_preserves_the_existing_card_contract() {
    let point = |interface: &str,
                 sample_count: i32,
                 rx_bytes_avg: i64,
                 tx_bytes_avg: i64,
                 rx_bytes_delta: i64,
                 tx_bytes_delta: i64,
                 rx_bps_avg: f64,
                 tx_bps_avg: f64,
                 latest_observed_at: &str,
                 updated_at: &str| TelemetryNetworkRateView {
        client_id: "vps-1".to_string(),
        interface: interface.to_string(),
        bucket_start: "2026-08-30T10:00:00+00:00".to_string(),
        bucket_secs: 60,
        sample_count,
        rx_bytes_avg,
        tx_bytes_avg,
        latest_observed_at: latest_observed_at.to_string(),
        rx_bytes_delta,
        tx_bytes_delta,
        rx_bps_avg,
        tx_bps_avg,
        updated_at: updated_at.to_string(),
    };
    let rows = aggregate_selected_network_history_oracle(vec![
        point("eth0", 2, 10, 20, 3, 4, 5.0, 6.0, "10:00:30", "10:00:31"),
        point("eth1", 3, 30, 40, 7, 8, 9.0, 10.0, "10:00:40", "10:00:41"),
    ]);

    assert_eq!(rows.len(), 1);
    let aggregate = &rows[0];
    assert!(aggregate.interface.is_empty());
    assert_eq!(aggregate.sample_count, 3);
    assert_eq!(aggregate.rx_bytes_avg, 40);
    assert_eq!(aggregate.tx_bytes_avg, 60);
    assert_eq!(aggregate.rx_bytes_delta, 10);
    assert_eq!(aggregate.tx_bytes_delta, 12);
    assert_eq!(aggregate.rx_bps_avg, 14.0);
    assert_eq!(aggregate.tx_bps_avg, 16.0);
    assert_eq!(aggregate.latest_observed_at, "10:00:40");
    assert_eq!(aggregate.updated_at, "10:00:41");
}
