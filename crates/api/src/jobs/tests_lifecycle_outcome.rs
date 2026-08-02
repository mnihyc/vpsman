use anyhow::anyhow;

use super::*;

#[test]
fn gateway_disconnect_failure_explains_committed_state_and_recovery() {
    let outcome = gateway_disconnect_outcome(
        Err(ApiError::conflict("gateway_control_url_missing")),
        "edge-01",
        "VPS key revocation",
    );

    assert_eq!(outcome.status, "failed");
    let message = outcome.error.expect("failure must be operator-visible");
    assert!(message.contains("VPS key revocation is saved"));
    assert!(message.contains("existing agent session may remain active"));
    assert!(message.contains("Access > Gateway sessions"));
}

#[test]
fn terminal_reconciliation_failure_does_not_leak_internal_error() {
    let outcome = terminal_reconciliation_outcome::<()>(
        Err(anyhow!("database password leaked in internal detail")),
        "VPS deletion",
    );

    assert_eq!(outcome.status, "failed");
    let message = outcome.error.expect("failure must be operator-visible");
    assert!(message.contains("VPS deletion is saved"));
    assert!(message.contains("Durable job results remain intact"));
    assert!(!message.contains("database password"));
}
