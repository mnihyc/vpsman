use super::*;
use crate::state::{GatewaySession, GatewaySessionCloseRequest, SESSION_COMMAND_QUEUE_CAPACITY};
use vpsman_common::{JobCommand, JobRequest};

#[test]
fn internal_control_auth_checks_bearer_token_when_configured() {
    let headers = vec![(
        "authorization".to_string(),
        "Bearer expected-token".to_string(),
    )];

    assert!(!authorized_internal_request(&headers, None));
    assert!(authorized_internal_request(
        &headers,
        Some("expected-token")
    ));
    assert!(!authorized_internal_request(&headers, Some("wrong-token")));
    assert!(!authorized_internal_request(&[], Some("expected-token")));
}

#[test]
fn http_header_end_detects_complete_header_block() {
    assert_eq!(find_header_end(b"POST / HTTP/1.1\r\n\r\nbody"), Some(15));
    assert_eq!(find_header_end(b"POST / HTTP/1.1\r\n"), None);
}

#[tokio::test]
async fn full_session_command_queue_returns_busy_error() {
    let state = GatewayState::default();
    let (sender, _receiver) = tokio::sync::mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    for _ in 0..SESSION_COMMAND_QUEUE_CAPACITY {
        let (response, _response_rx) = tokio::sync::oneshot::channel();
        sender
            .try_send(GatewaySessionMessage::Command(Box::new(GatewayCommand {
                request: test_job_request(),
                payload_hash: "test-payload-hash".to_string(),
                response,
            })))
            .unwrap();
    }
    let (close_tx, _close_rx) = tokio::sync::watch::channel(None::<GatewaySessionCloseRequest>);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: uuid::Uuid::new_v4(),
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender,
            close_tx,
        },
    );

    let error = dispatch_gateway_command(
        &state,
        GatewayCommandDispatch {
            client_id: "client-a".to_string(),
            request: test_job_request(),
            expected_process_incarnation_id: state
                .sessions
                .read()
                .await
                .get("client-a")
                .unwrap()
                .process_incarnation_id,
            payload_hash: "test-payload-hash".to_string(),
        },
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("agent_session_command_queue_full:client-a"));
}

#[tokio::test]
async fn disconnect_bypasses_full_session_command_queue() {
    let state = GatewayState::default();
    let (sender, _receiver) = tokio::sync::mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    for _ in 0..SESSION_COMMAND_QUEUE_CAPACITY {
        let (response, _response_rx) = tokio::sync::oneshot::channel();
        sender
            .try_send(GatewaySessionMessage::Command(Box::new(GatewayCommand {
                request: test_job_request(),
                payload_hash: "test-payload-hash".to_string(),
                response,
            })))
            .unwrap();
    }
    let (close_tx, mut close_rx) = tokio::sync::watch::channel(None::<GatewaySessionCloseRequest>);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: uuid::Uuid::new_v4(),
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender,
            close_tx,
        },
    );

    let result = disconnect_gateway_session(
        &state,
        GatewaySessionDisconnect {
            client_id: "client-a".to_string(),
            reason: "client_key_revoked".to_string(),
        },
    )
    .await
    .unwrap();

    assert!(result.accepted);
    assert!(result.disconnected);
    assert!(!state.sessions.read().await.contains_key("client-a"));
    close_rx.changed().await.unwrap();
    assert_eq!(
        close_rx.borrow().as_ref(),
        Some(&GatewaySessionCloseRequest::Graceful(
            "client_key_revoked".to_string()
        ))
    );
}

fn test_job_request() -> JobRequest {
    JobRequest {
        job_id: uuid::Uuid::new_v4(),
        command_version: 1,
        command: JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        },
        max_timeout_secs: 30,
    }
}
