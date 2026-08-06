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

#[test]
fn contextual_internal_error_keeps_public_context_and_private_cause_separate() {
    let error = ApiError::internal(
        "topology_graph_agents_unavailable",
        "Topology could not load the VPS inventory.",
        anyhow::anyhow!("private database host and query"),
    );

    assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error.code, "topology_graph_agents_unavailable");
    assert_eq!(
        error.public_message.as_deref(),
        Some("Topology could not load the VPS inventory.")
    );
    assert!(error.error.to_string().contains("private database"));
    assert!(!error.public_message.unwrap().contains("database"));
}

#[test]
fn internal_mapper_accepts_typed_causes_without_exposing_them() {
    let error = ApiError::internal_mapper(
        "inventory_unavailable",
        "The inventory could not be loaded.",
    )(std::io::Error::other("private storage path"));

    assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error.code, "inventory_unavailable");
    assert_eq!(
        error.public_message.as_deref(),
        Some("The inventory could not be loaded.")
    );
    assert!(error.error.to_string().contains("private storage path"));
    assert!(!error.public_message.unwrap().contains("storage path"));
}

#[tokio::test]
async fn contextual_internal_error_serializes_only_operator_safe_context() {
    let response = ApiError::internal(
        "job_output_artifact_load_failed",
        "The job-output artifact could not be loaded.",
        anyhow::anyhow!("private object-store bucket and key"),
    )
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("error response body");
    let body = String::from_utf8(body.to_vec()).expect("UTF-8 error response");

    assert!(body.contains("job_output_artifact_load_failed"));
    assert!(body.contains("The job-output artifact could not be loaded."));
    assert!(!body.contains("private object-store"));
    assert!(!body.contains("bucket and key"));
}
