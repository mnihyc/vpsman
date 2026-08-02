use super::*;

#[test]
fn notification_worker_config_clamps_bounds() {
    assert_eq!(
        AlertNotificationWorkerConfig::new(0, 0, 0, 0),
        AlertNotificationWorkerConfig {
            delivery_limit: 1,
            retention_days: 1,
            retention_prune_limit: 1,
            webhook_timeout_secs: 1,
        }
    );
    assert_eq!(
        AlertNotificationWorkerConfig::new(10_000, 10_000, 20_000, 120),
        AlertNotificationWorkerConfig {
            delivery_limit: 200,
            retention_days: 3_650,
            retention_prune_limit: 10_000,
            webhook_timeout_secs: 60,
        }
    );
}

#[test]
fn delivery_error_is_bounded() {
    let error = "x".repeat(MAX_ERROR_BYTES + 100);
    assert_eq!(truncate_error(&error).len(), MAX_ERROR_BYTES);
}

#[test]
fn delivery_error_keeps_nested_transport_cause() {
    let error = anyhow::anyhow!("connection refused").context("webhook request failed");
    assert_eq!(
        format_delivery_error(&error),
        "webhook request failed: connection refused"
    );
}
