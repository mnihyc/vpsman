use super::*;

#[test]
fn submitted_job_request_preserves_exact_input_and_existing_credential_redaction() {
    let operation = json!({
        "type": "shell",
        "argv": ["printf", "%s\n", "", "a b", "\"quoted\"", "back\\slash"],
        "pty": true
    });
    assert_eq!(
        job_submitted_request_view(Some(operation.clone())).operation,
        Some(operation)
    );
    assert!(job_submitted_request_view(None).operation.is_none());
    assert_eq!(
        job_submitted_request_view(Some(json!({
            "type": "network_apply",
            "plan": {"name": "fixture", "builtin_credentials": {"key": "private"}}
        })))
        .operation,
        Some(json!({"type": "network_apply", "plan": {"name": "fixture"}}))
    );
}

#[test]
fn exclusive_operation_types_follow_shared_command_safety() {
    let exclusive = exclusive_operation_types();
    assert!(exclusive.contains(&"runtime_config_sync"));
    assert!(exclusive.contains(&"agent_update"));
    assert!(exclusive.contains(&"agent_update_activate"));
    assert!(exclusive.contains(&"agent_update_rollback"));
    assert!(exclusive.contains(&"agent_update_check"));
    assert!(!exclusive.contains(&"backup"));
    assert!(!exclusive.contains(&"shell"));
    assert!(!exclusive.contains(&"network_speed_test"));
    assert!(!exclusive.contains(&"network_status"));
}

#[test]
fn persisted_job_operation_decode_is_explicit_for_null_and_invalid_shapes() {
    assert_eq!(
        decode_persisted_job_operation(None).unwrap_err(),
        "operation is null"
    );
    assert!(
        decode_persisted_job_operation(Some(sqlx::types::Json(json!({
            "type": "removed_legacy_operation"
        }))))
        .unwrap_err()
        .contains("unknown variant")
    );
    assert!(matches!(
        decode_persisted_job_operation(Some(sqlx::types::Json(json!({
            "type": "shell",
            "argv": ["/bin/true"],
            "pty": false
        }))))
        .unwrap(),
        JobCommand::Shell { .. }
    ));
}
