use anyhow::Result;
use tracing::warn;

use crate::{error::ApiError, model::LifecycleOutcomeView};

pub(crate) fn gateway_disconnect_outcome(
    result: Result<(), ApiError>,
    client_id: &str,
    committed_action: &str,
) -> LifecycleOutcomeView {
    match result {
        Ok(()) => completed("gateway_session_disconnect"),
        Err(error) => {
            warn!(
                ?error,
                client_id, committed_action, "post-commit gateway disconnect failed"
            );
            let reason = if error.code == "gateway_control_url_missing" {
                format!(
                    "{committed_action} is saved, but gateway control is not configured, so an existing agent session may remain active. Configure gateway control or disconnect the session from Access > Gateway sessions"
                )
            } else {
                format!(
                    "{committed_action} is saved, but the gateway did not accept the session disconnect ({}), so an existing agent session may remain active. Retry from Access > Gateway sessions and inspect API/gateway logs",
                    error.code.replace('_', " ")
                )
            };
            failed("gateway_session_disconnect", reason)
        }
    }
}

pub(crate) fn terminal_reconciliation_outcome<T>(
    result: Result<T>,
    committed_action: &str,
) -> LifecycleOutcomeView {
    match result {
        Ok(_) => completed("job_terminal_reconciliation"),
        Err(error) => {
            warn!(
                ?error,
                committed_action, "post-commit terminal event reconciliation failed"
            );
            failed(
                "job_terminal_reconciliation",
                format!(
                    "{committed_action} is saved, but related job terminal events were not fully reconciled. Durable job results remain intact; refresh Jobs and inspect API logs"
                ),
            )
        }
    }
}

fn completed(operation: &str) -> LifecycleOutcomeView {
    LifecycleOutcomeView {
        operation: operation.to_string(),
        status: "completed".to_string(),
        error: None,
    }
}

fn failed(operation: &str, error: String) -> LifecycleOutcomeView {
    LifecycleOutcomeView {
        operation: operation.to_string(),
        status: "failed".to_string(),
        error: Some(error),
    }
}

#[cfg(test)]
mod tests {
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
}
