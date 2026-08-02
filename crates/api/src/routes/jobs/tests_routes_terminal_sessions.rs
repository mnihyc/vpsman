use super::*;
use axum::http::{header::AUTHORIZATION, HeaderMap};

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{AgentView, JobHistoryView, JobTargetView, OperatorPreferences, OperatorRecord},
    model_terminal::TerminalSessionView,
    repository::{MemoryState, Repository},
};
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
fn terminal_control_request_uses_only_the_session_control_shape() {
    let request_id = Uuid::new_v4();
    let request = serde_json::from_value::<TerminalControlSubmitRequest>(serde_json::json!({
        "request_id": request_id,
        "action": {
            "type": "input",
            "data_base64": "Aw=="
        }
    }))
    .unwrap();
    assert_eq!(request.request_id, request_id);
    assert_eq!(
        request.action,
        TerminalControlAction::Input {
            data_base64: "Aw==".to_string()
        }
    );

    let legacy_input = serde_json::from_value::<TerminalControlSubmitRequest>(serde_json::json!({
        "job_id": Uuid::new_v4(),
        "text": "uptime\n",
        "confirmed": true
    }));
    assert!(legacy_input.is_err());

    let unknown_field = serde_json::from_value::<TerminalControlSubmitRequest>(serde_json::json!({
        "request_id": Uuid::new_v4(),
        "action": {
            "type": "close",
            "reason": "done"
        },
        "confirmed": true
    }));
    assert!(unknown_field.is_err());
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

    let invalid_session = validate_terminal_control_action(
        Uuid::nil(),
        &TerminalControlAction::Resize { cols: 80, rows: 24 },
    )
    .unwrap_err();
    assert_eq!(invalid_session.code, "terminal_session_id_invalid");
}

#[tokio::test]
async fn terminal_control_route_requires_scope_and_session_ownership() {
    let (state, memory, session_id, job_id) = route_test_state("open").await;
    let (missing_scope_headers, _) = auth_headers(&state, &memory, &["jobs:write"]).await;
    let action = TerminalControlAction::Resize {
        cols: 100,
        rows: 30,
    };

    let missing_scope = control_terminal_session(
        State(state.clone()),
        missing_scope_headers,
        Path(("edge-a".to_string(), session_id)),
        Json(TerminalControlSubmitRequest {
            request_id: Uuid::new_v4(),
            action: action.clone(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(missing_scope.status, StatusCode::FORBIDDEN);
    assert_eq!(missing_scope.code, "operator_scope_insufficient");

    let (owner_headers, owner_id) =
        auth_headers(&state, &memory, &["jobs:write", "terminal:read"]).await;
    seed_terminal_open_job(&memory, job_id, Uuid::nil()).await;
    let not_owned = control_terminal_session(
        State(state),
        owner_headers,
        Path(("edge-a".to_string(), session_id)),
        Json(TerminalControlSubmitRequest {
            request_id: Uuid::new_v4(),
            action,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(not_owned.status, StatusCode::FORBIDDEN);
    assert_eq!(not_owned.code, "terminal_session_not_owned");
    assert_ne!(owner_id, memory.jobs.read().await[0].actor_id.unwrap());
    assert!(memory.audits.read().await.is_empty());
}

#[tokio::test]
async fn terminal_control_route_rejects_invalid_identifiers_and_closed_sessions() {
    let (state, memory, session_id, _) = route_test_state("closed").await;
    let (headers, _) = auth_headers(&state, &memory, &["jobs:write", "terminal:read"]).await;
    let action = TerminalControlAction::Resize {
        cols: 100,
        rows: 30,
    };

    let invalid_request = control_terminal_session(
        State(state.clone()),
        headers.clone(),
        Path(("edge-a".to_string(), session_id)),
        Json(TerminalControlSubmitRequest {
            request_id: Uuid::nil(),
            action: action.clone(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(invalid_request.code, "terminal_control_request_id_invalid");

    let invalid_session = control_terminal_session(
        State(state.clone()),
        headers.clone(),
        Path(("edge-a".to_string(), Uuid::nil())),
        Json(TerminalControlSubmitRequest {
            request_id: Uuid::new_v4(),
            action: action.clone(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(invalid_session.code, "terminal_session_id_invalid");

    let closed = control_terminal_session(
        State(state),
        headers,
        Path(("edge-a".to_string(), session_id)),
        Json(TerminalControlSubmitRequest {
            request_id: Uuid::new_v4(),
            action,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(closed.status, StatusCode::CONFLICT);
    assert_eq!(closed.code, "terminal_session_not_open");
}

#[tokio::test]
async fn terminal_control_route_updates_resize_and_close_lifecycle() {
    let (state, memory, session_id, job_id) = route_test_state("open").await;
    let (headers, owner_id) = auth_headers(&state, &memory, &["jobs:write", "terminal:read"]).await;
    seed_terminal_open_job(&memory, job_id, owner_id).await;

    let Json(resize_ack) = control_terminal_session(
        State(state.clone()),
        headers.clone(),
        Path(("edge-a".to_string(), session_id)),
        Json(TerminalControlSubmitRequest {
            request_id: Uuid::new_v4(),
            action: TerminalControlAction::Resize {
                cols: 132,
                rows: 43,
            },
        }),
    )
    .await
    .unwrap();
    assert!(resize_ack.accepted);
    assert_eq!(resize_ack.action, "resize");
    let resized = memory.terminal_sessions.read().await[0].clone();
    assert_eq!((resized.cols, resized.rows), (Some(132), Some(43)));
    assert_eq!(resized.state, "open");
    assert_eq!(resized.last_event, "terminal_resize");

    let Json(close_ack) = control_terminal_session(
        State(state),
        headers,
        Path(("edge-a".to_string(), session_id)),
        Json(TerminalControlSubmitRequest {
            request_id: Uuid::new_v4(),
            action: TerminalControlAction::Close {
                reason: Some("operator finished".to_string()),
            },
        }),
    )
    .await
    .unwrap();
    assert!(close_ack.accepted);
    assert_eq!(close_ack.action, "close");

    let closed = memory.terminal_sessions.read().await[0].clone();
    assert_eq!(closed.state, "closed");
    assert_eq!(closed.close_reason.as_deref(), Some("operator finished"));
    assert_eq!(closed.last_event, "terminal_close");
    assert_eq!(memory.jobs.read().await[0].status, "completed");
    assert_eq!(memory.job_targets.read().await[0].status, "completed");
    let audits = memory.audits.read().await;
    assert_eq!(audits.len(), 2);
    assert_eq!(audits[0].action, "terminal.resize");
    assert_eq!(audits[1].action, "terminal.close");
}

#[tokio::test]
async fn terminal_control_lazily_fails_a_session_from_an_old_agent_process() {
    let (state, memory, session_id, job_id) = route_test_state("open").await;
    let (headers, owner_id) = auth_headers(&state, &memory, &["jobs:write", "terminal:read"]).await;
    seed_terminal_open_job(&memory, job_id, owner_id).await;
    memory.agents.write().await[0].process_incarnation_id = Some(Uuid::new_v4());

    let error = control_terminal_session(
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
    let (events, _) = tokio::sync::broadcast::channel(1);
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
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    };
    (state, memory, session_id, job_id)
}

async fn auth_headers(
    state: &AppState,
    memory: &MemoryState,
    scopes: &[&str],
) -> (HeaderMap, Uuid) {
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
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        format!("Bearer {}", auth.access_token).parse().unwrap(),
    );
    (headers, operator_id)
}

async fn seed_terminal_open_job(memory: &MemoryState, job_id: Uuid, actor_id: Uuid) {
    memory.jobs.write().await.push(JobHistoryView {
        id: job_id,
        actor_id: Some(actor_id),
        command_type: "terminal_open".to_string(),
        source_schedule_id: None,
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
