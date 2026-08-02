use super::*;

#[test]
fn every_error_has_operator_safe_reason_and_recovery() {
    assert_eq!(
        default_public_reason(StatusCode::BAD_REQUEST, "invalid_selector_expression"),
        "The request was rejected: invalid selector expression."
    );
    assert!(
        public_recovery(StatusCode::BAD_REQUEST, "invalid_selector_expression")
            .contains("Correct the submitted values")
    );

    let internal = default_public_reason(
        StatusCode::INTERNAL_SERVER_ERROR,
        "private_database_failure",
    );
    assert!(internal.contains("could not complete"));
    assert!(!internal.contains("database"));
    assert!(
        public_recovery(StatusCode::INTERNAL_SERVER_ERROR, "internal_server_error")
            .contains("inspect API logs")
    );
}

#[test]
fn stale_and_capability_errors_have_specific_recovery() {
    assert!(
        public_recovery(StatusCode::CONFLICT, "confirmation_snapshot_stale")
            .contains("review the action again")
    );
    assert!(
        public_recovery(StatusCode::CONFLICT, "port_forward_capability_unsupported")
            .contains("agent status")
    );
}
