use super::*;

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{AgentView, JobHistoryView, JobTargetView, OperatorPreferences, OperatorRecord},
    model_terminal::TerminalSessionView,
    repository::{MemoryState, Repository},
};
use axum::http::{header::AUTHORIZATION, HeaderMap};
use uuid::Uuid;
use vpsman_common::{AgentCapabilitySnapshot, TerminalControlAction};

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

#[tokio::test]
async fn terminal_socket_auth_requires_both_scopes_and_session_ownership() {
    let (state, memory, session_id, job_id) = route_test_state("open").await;
    let (missing_scope_token, _) = issue_auth(&state, &memory, &["terminal:read"]).await;
    let missing_scope =
        authenticate_terminal_socket(&state, &missing_scope_token, "edge-a", session_id, false)
            .await
            .unwrap_err();
    assert_eq!(missing_scope.status, StatusCode::FORBIDDEN);
    assert_eq!(missing_scope.code, "operator_scope_insufficient");

    let (owner_token, owner_id) =
        issue_auth(&state, &memory, &["jobs:write", "terminal:read"]).await;
    seed_terminal_open_job(&memory, job_id, Uuid::nil()).await;
    let not_owned = authenticate_terminal_socket(&state, &owner_token, "edge-a", session_id, false)
        .await
        .unwrap_err();
    assert_eq!(not_owned.status, StatusCode::FORBIDDEN);
    assert_eq!(not_owned.code, "terminal_session_not_owned");
    assert_ne!(owner_id, memory.jobs.read().await[0].actor_id.unwrap());
    assert!(memory.audits.read().await.is_empty());
}

#[tokio::test]
async fn terminal_socket_controls_update_evidence_without_packet_audits_or_reconciliation() {
    let (state, memory, session_id, job_id) = route_test_state("open").await;
    let (token, owner_id) = issue_auth(&state, &memory, &["jobs:write", "terminal:read"]).await;
    seed_terminal_open_job(&memory, job_id, owner_id).await;
    let authority = authenticate_terminal_socket(&state, &token, "edge-a", session_id, false)
        .await
        .unwrap();

    let input = dispatch_bound_terminal_control(
        &state,
        "edge-a",
        session_id,
        &authority,
        TerminalSocketControlWork {
            request_id: Uuid::new_v4(),
            action: TerminalControlAction::Input {
                data_base64: BASE64_STANDARD.encode(b"uptime\r"),
            },
            pending_input_bytes: 7,
        },
    )
    .await;
    assert!(input.error.is_none());
    assert_eq!(input.ack.as_ref().unwrap().action, "input");
    assert!(memory.audits.read().await.is_empty());
    assert_eq!(memory.jobs.read().await[0].status, "running");

    let resize = dispatch_bound_terminal_control(
        &state,
        "edge-a",
        session_id,
        &authority,
        TerminalSocketControlWork {
            request_id: Uuid::new_v4(),
            action: TerminalControlAction::Resize {
                cols: 132,
                rows: 43,
            },
            pending_input_bytes: 0,
        },
    )
    .await;
    assert!(resize.error.is_none());
    assert_eq!(resize.ack.as_ref().unwrap().action, "resize");
    let resized = memory.terminal_sessions.read().await[0].clone();
    assert_eq!((resized.cols, resized.rows), (Some(132), Some(43)));
    assert_eq!(resized.state, "open");
    assert_eq!(resized.last_event, "terminal_resize");
    assert!(memory.audits.read().await.is_empty());
    assert_eq!(memory.jobs.read().await[0].status, "running");
    assert_eq!(memory.job_targets.read().await[0].status, "running");

    let close = dispatch_bound_terminal_control(
        &state,
        "edge-a",
        session_id,
        &authority,
        TerminalSocketControlWork {
            request_id: Uuid::new_v4(),
            action: TerminalControlAction::Close {
                reason: Some("operator finished".to_string()),
            },
            pending_input_bytes: 0,
        },
    )
    .await;
    assert!(close.error.is_none());
    assert!(close.terminal);
    let closed = memory.terminal_sessions.read().await[0].clone();
    assert_eq!(closed.state, "closed");
    assert_eq!(closed.close_reason.as_deref(), Some("operator finished"));
    assert_eq!(memory.jobs.read().await[0].status, "completed");
    assert_eq!(memory.job_targets.read().await[0].status, "completed");
    let audits = memory.audits.read().await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].action, "terminal.close");
}

#[tokio::test]
async fn terminal_socket_controls_reuse_attach_authority_until_revalidation() {
    let (state, memory, session_id, job_id) = route_test_state("open").await;
    let (token, owner_id) = issue_auth(&state, &memory, &["jobs:write", "terminal:read"]).await;
    seed_terminal_open_job(&memory, job_id, owner_id).await;
    let authority = authenticate_terminal_socket(&state, &token, "edge-a", session_id, false)
        .await
        .unwrap();

    memory.jobs.write().await[0].actor_id = Some(Uuid::new_v4());
    let result = dispatch_bound_terminal_control(
        &state,
        "edge-a",
        session_id,
        &authority,
        TerminalSocketControlWork {
            request_id: Uuid::new_v4(),
            action: TerminalControlAction::Input {
                data_base64: BASE64_STANDARD.encode(b"x"),
            },
            pending_input_bytes: 1,
        },
    )
    .await;
    assert!(result.error.is_none());
    assert_eq!(result.ack.as_ref().unwrap().action, "input");
    assert!(result.session.is_none());
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

#[tokio::test]
async fn terminal_rest_control_reuses_the_same_evidence_path() {
    let (state, memory, session_id, job_id) = route_test_state("open").await;
    let (token, owner_id) = issue_auth(&state, &memory, &["jobs:write", "terminal:read"]).await;
    seed_terminal_open_job(&memory, job_id, owner_id).await;
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());

    let Json(ack) = control_terminal_session(
        State(state),
        headers,
        Path(("edge-a".to_string(), session_id)),
        Json(TerminalControlSubmitRequest {
            request_id: Uuid::new_v4(),
            action: TerminalControlAction::Resize {
                cols: 100,
                rows: 30,
            },
        }),
    )
    .await
    .unwrap();
    assert_eq!(ack.action, "resize");
    assert_eq!(
        (
            memory.terminal_sessions.read().await[0].cols,
            memory.terminal_sessions.read().await[0].rows
        ),
        (Some(100), Some(30))
    );
    assert!(memory.audits.read().await.is_empty());
}

#[tokio::test]
async fn terminal_socket_attach_lazily_fails_an_old_agent_incarnation() {
    let (state, memory, session_id, job_id) = route_test_state("open").await;
    let (token, owner_id) = issue_auth(&state, &memory, &["jobs:write", "terminal:read"]).await;
    seed_terminal_open_job(&memory, job_id, owner_id).await;
    memory.agents.write().await[0].process_incarnation_id = Some(Uuid::new_v4());

    let error = authenticate_terminal_socket(&state, &token, "edge-a", session_id, true)
        .await
        .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "terminal_session_not_open");
    let session = memory.terminal_sessions.read().await[0].clone();
    assert_eq!(session.state, "missing");
    assert_eq!(
        session.close_reason.as_deref(),
        Some("agent_process_restarted")
    );
    assert_eq!(memory.jobs.read().await[0].status, "failed");
    assert_eq!(memory.job_targets.read().await[0].status, "failed");
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

async fn route_test_state(state_name: &str) -> (AppState, MemoryState, Uuid, Uuid) {
    let memory = MemoryState::default();
    let session_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    memory.agents.write().await.push(AgentView {
        id: "edge-a".to_string(),
        display_name: "edge-a".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: Some("2026-06-21T00:00:00Z".to_string()),
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: Some(Uuid::new_v4()),
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot::default(),
    });
    memory
        .terminal_sessions
        .write()
        .await
        .push(test_terminal_session(session_id, job_id, state_name));
    let repo = Repository::Memory(memory.clone());
    let (events, _) = crate::state::WsEventBus::new(1);
    let state = AppState {
        repo,
        events,
        internal_token: None,
        gateway: GatewayDispatchClient::new(
            Some("http://127.0.0.1:1".to_string()),
            Some("internal-test-token".to_string()),
        )
        .with_test_privilege_auto_approve(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    };
    (state, memory, session_id, job_id)
}

async fn issue_auth(state: &AppState, memory: &MemoryState, scopes: &[&str]) -> (String, Uuid) {
    let operator = OperatorRecord {
        id: Uuid::new_v4(),
        username: format!("operator-{}", Uuid::new_v4()),
        password_hash: "test-password-hash".to_string(),
        status: "active".to_string(),
        role: "operator".to_string(),
        scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
        preferences: OperatorPreferences::default(),
        totp_enabled: false,
        totp_secret_ciphertext_hex: None,
        totp_secret_nonce_hex: None,
        totp_secret_salt_hex: None,
        totp_last_accepted_step: None,
        session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
        created_at: crate::unix_now().to_string(),
        disabled_at: None,
        deleted_at: None,
    };
    let view = operator.view();
    let operator_id = view.id;
    memory.operators.write().await.push(operator);
    let auth = state.repo.issue_session(view).await.unwrap();
    (auth.access_token, operator_id)
}

async fn seed_terminal_open_job(memory: &MemoryState, job_id: Uuid, actor_id: Uuid) {
    memory.jobs.write().await.push(JobHistoryView {
        id: job_id,
        actor_id: Some(actor_id),
        command_type: "terminal_open".to_string(),
        source_schedule_id: None,
        causation_id: None,
        schedule_lineage: Vec::new(),
        privileged: true,
        status: "running".to_string(),
        target_count: 1,
        payload_hash: "terminal-open-test".to_string(),
        max_timeout_secs: 300,
        created_at: "2026-06-21T00:00:00Z".to_string(),
        completed_at: None,
    });
    memory.job_targets.write().await.push(JobTargetView {
        job_id,
        client_id: "edge-a".to_string(),
        status: "running".to_string(),
        message: None,
        exit_code: None,
        started_at: Some("2026-06-21T00:00:00Z".to_string()),
        deadline_at: None,
        completed_at: None,
        process_incarnation_id: memory.agents.read().await[0].process_incarnation_id,
    });
}

fn test_terminal_session(session_id: Uuid, job_id: Uuid, state: &str) -> TerminalSessionView {
    TerminalSessionView {
        session_id,
        client_id: "edge-a".to_string(),
        job_id,
        state: state.to_string(),
        last_status: "accepted".to_string(),
        argv: vec!["/bin/sh".to_string(), "-l".to_string()],
        cwd: Some("/root".to_string()),
        cols: Some(120),
        rows: Some(40),
        idle_timeout_secs: Some(3600),
        flow_window_bytes: Some(65_536),
        output_first_seq: Some(1),
        output_next_seq: Some(1),
        output_retained_first_seq: Some(1),
        output_retained_bytes: Some(0),
        output_dropped_bytes: Some(0),
        output_dropped_chunks: Some(0),
        output_replay_truncated: false,
        last_input_seq: 2,
        close_reason: None,
        last_event: "terminal_open".to_string(),
        opened_at: Some("2026-06-21T00:00:00Z".to_string()),
        observed_at: "2026-06-21T00:00:00Z".to_string(),
    }
}
