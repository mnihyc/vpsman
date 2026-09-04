use super::*;

use uuid::Uuid;
use vpsman_common::TerminalControlAction;

#[test]
fn terminal_control_input_validation_accepts_exact_terminal_bytes() {
    let session_id = Uuid::new_v4();
    let terminal_bytes = [0x03, b'\r', 0x1b, b'[', b'A', 0x7f];
    validate_terminal_control_action(
        session_id,
        &TerminalControlAction::Input {
            data_base64: BASE64_STANDARD.encode(terminal_bytes),
        },
    )
    .unwrap();

    for (data_base64, expected_code) in [
        (String::new(), "terminal_input_size_invalid"),
        ("not base64".to_string(), "terminal_input_base64_invalid"),
        (
            BASE64_STANDARD.encode(vec![0_u8; MAX_TERMINAL_INPUT_BYTES + 1]),
            "terminal_input_size_invalid",
        ),
    ] {
        let error = validate_terminal_control_action(
            session_id,
            &TerminalControlAction::Input { data_base64 },
        )
        .unwrap_err();
        assert_eq!(error.code, expected_code);
    }
}

#[test]
fn terminal_socket_frames_use_the_session_bound_contract() {
    let request_id = Uuid::new_v4();
    let frame = serde_json::from_value::<TerminalSocketClientFrame>(serde_json::json!({
        "type": "input",
        "request_id": request_id,
        "data_base64": "Aw=="
    }))
    .unwrap();
    let (parsed_request_id, action) = frame.into_control();
    assert_eq!(parsed_request_id, request_id);
    assert_eq!(
        action,
        TerminalControlAction::Input {
            data_base64: "Aw==".to_string()
        }
    );

    let legacy_http_shape =
        serde_json::from_value::<TerminalSocketClientFrame>(serde_json::json!({
            "request_id": Uuid::new_v4(),
            "action": {"type": "close", "reason": "done"}
        }));
    assert!(legacy_http_shape.is_err());

    let unknown_field = serde_json::from_value::<TerminalSocketClientFrame>(serde_json::json!({
        "type": "resize",
        "request_id": Uuid::new_v4(),
        "cols": 80,
        "rows": 24,
        "confirmed": true
    }));
    assert!(unknown_field.is_err());

    let ack = TerminalControlAck {
        request_id,
        session_id: Uuid::new_v4(),
        action: "input".to_string(),
        accepted: true,
        status: "accepted".to_string(),
        message: "accepted".to_string(),
        input_seq: Some(1),
        written_bytes: Some(1),
        cols: None,
        rows: None,
    };
    let encoded = serde_json::to_value(TerminalSocketServerFrame::ControlAck { ack }).unwrap();
    assert_eq!(encoded["type"], "control_ack");
    assert_eq!(encoded["ack"]["request_id"], request_id.to_string());
}

#[test]
fn terminal_rest_control_request_preserves_the_vpsctl_contract() {
    let request_id = Uuid::new_v4();
    let request = serde_json::from_value::<TerminalControlSubmitRequest>(serde_json::json!({
        "request_id": request_id,
        "action": {
            "type": "resize",
            "cols": 80,
            "rows": 24
        }
    }))
    .unwrap();
    assert_eq!(request.request_id, request_id);
    assert_eq!(
        request.action,
        TerminalControlAction::Resize { cols: 80, rows: 24 }
    );
    assert!(
        serde_json::from_value::<TerminalControlSubmitRequest>(serde_json::json!({
            "request_id": request_id,
            "text": "uptime\n"
        }))
        .is_err()
    );
}

#[test]
fn terminal_control_resize_and_close_reuse_session_validation() {
    let session_id = Uuid::new_v4();
    validate_terminal_control_action(
        session_id,
        &TerminalControlAction::Resize { cols: 80, rows: 24 },
    )
    .unwrap();
    validate_terminal_control_action(
        session_id,
        &TerminalControlAction::Close {
            reason: Some("operator finished".to_string()),
        },
    )
    .unwrap();

    let invalid_resize = validate_terminal_control_action(
        session_id,
        &TerminalControlAction::Resize { cols: 19, rows: 24 },
    )
    .unwrap_err();
    assert_eq!(invalid_resize.code, "terminal_cols_out_of_range");

    let invalid_close = validate_terminal_control_action(
        session_id,
        &TerminalControlAction::Close {
            reason: Some("bad\u{0007}reason".to_string()),
        },
    )
    .unwrap_err();
    assert_eq!(invalid_close.code, "terminal_close_reason_invalid");
}

#[test]
fn terminal_socket_retries_transient_server_failures_and_releases_rejected_close() {
    assert!(terminal_socket_error_recoverable(
        StatusCode::INTERNAL_SERVER_ERROR
    ));
    assert!(terminal_socket_error_recoverable(
        StatusCode::SERVICE_UNAVAILABLE
    ));
    assert!(!terminal_socket_error_recoverable(StatusCode::UNAUTHORIZED));
    assert!(should_clear_terminal_close_queue(true, false));
    assert!(!should_clear_terminal_close_queue(true, true));
    assert!(!should_clear_terminal_close_queue(false, false));
}

#[test]
fn terminal_socket_ignores_redundant_streaming_status_notifications() {
    assert!(!terminal_event_requires_replay(None, false));
    assert!(terminal_event_requires_replay(Some(7), false));
    assert!(terminal_event_requires_replay(None, true));
    assert!(terminal_event_requires_replay(Some(7), true));
}

#[test]
fn terminal_gateway_failures_map_to_operator_facing_states() {
    for (message, expected_status, expected_code) in [
        (
            "agent_not_online",
            StatusCode::CONFLICT,
            "terminal_agent_not_online",
        ),
        (
            "agent_incarnation_mismatch",
            StatusCode::CONFLICT,
            "terminal_agent_reconnected",
        ),
        (
            "terminal queue_full",
            StatusCode::SERVICE_UNAVAILABLE,
            "terminal_control_busy",
        ),
        (
            "terminal control timed out",
            StatusCode::GATEWAY_TIMEOUT,
            "terminal_control_timeout",
        ),
        (
            "transport closed",
            StatusCode::BAD_GATEWAY,
            "terminal_control_delivery_failed",
        ),
    ] {
        let error = map_terminal_gateway_error(anyhow::anyhow!(message));
        assert_eq!(error.status, expected_status);
        assert_eq!(error.code, expected_code);
    }
}
