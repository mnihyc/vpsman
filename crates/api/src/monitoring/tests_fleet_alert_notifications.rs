use super::*;

#[test]
fn severity_threshold_matches_more_severe_alerts() {
    assert!(severity_rank("critical") <= severity_rank("warning"));
    assert!(severity_rank("warning") <= severity_rank("warning"));
    assert!(severity_rank("info") > severity_rank("warning"));
}

#[test]
fn delivery_error_keeps_nested_transport_cause_and_is_bounded() {
    let error = anyhow::anyhow!("connection refused").context("webhook request failed");
    assert_eq!(
        format_delivery_error(&error),
        "webhook request failed: connection refused"
    );
    let long = anyhow::anyhow!("x".repeat(MAX_NOTIFICATION_ERROR_BYTES + 100));
    assert_eq!(
        format_delivery_error(&long).len(),
        MAX_NOTIFICATION_ERROR_BYTES
    );
}

#[test]
fn process_lease_covers_every_serial_delivery_timeout() {
    assert_eq!(notification_delivery_lease_secs(0), 60);
    assert_eq!(notification_delivery_lease_secs(50), 310);
    assert_eq!(notification_delivery_lease_secs(200), 1_060);
}
