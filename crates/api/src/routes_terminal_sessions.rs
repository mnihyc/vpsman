use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::{
    encode_json, payload_hash, TerminalControlAck, TerminalControlAction, TerminalControlRequest,
    MAX_TERMINAL_INPUT_BYTES,
};

use crate::{
    error::ApiError,
    job_terminal::{validate_terminal_close, validate_terminal_resize},
    model_terminal::{TerminalControlSubmitRequest, TerminalReplayView, TerminalSessionView},
    security::{operator_has_scope, SCOPE_TERMINAL_READ},
    state::AppState,
    util::limit_or_default,
};

const DEFAULT_TERMINAL_REPLAY_LIMIT: i64 = 100;
const MAX_TERMINAL_REPLAY_BYTES: i64 = 4 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub(crate) struct TerminalSessionQuery {
    pub(crate) limit: Option<i64>,
    pub(crate) client_id: Option<String>,
    pub(crate) session_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TerminalReplayQuery {
    pub(crate) from_seq: Option<i64>,
    pub(crate) limit: Option<i64>,
    pub(crate) max_bytes: Option<i64>,
    pub(crate) include_data: Option<bool>,
}

pub(crate) async fn list_terminal_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TerminalSessionQuery>,
) -> Result<Json<Vec<TerminalSessionView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_TERMINAL_READ)
        .await?;
    let client_id = query
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(client_id) = client_id {
        if client_id.len() > 128 {
            return Err(ApiError::bad_request("terminal_client_id_too_long"));
        }
    }
    let sessions = state
        .repo
        .list_terminal_sessions(limit_or_default(query.limit), client_id, query.session_id)
        .await?;
    for session in &sessions {
        state
            .repo
            .reconcile_terminal_job_by_id(session.job_id)
            .await?;
    }
    Ok(Json(
        state
            .repo
            .list_terminal_sessions(limit_or_default(query.limit), client_id, query.session_id)
            .await?,
    ))
}

pub(crate) async fn terminal_session_replay(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((client_id, session_id)): Path<(String, Uuid)>,
    Query(query): Query<TerminalReplayQuery>,
) -> Result<Json<TerminalReplayView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_TERMINAL_READ)
        .await?;
    validate_terminal_replay_client_id(&client_id)?;
    let sessions = state
        .repo
        .list_terminal_sessions(1, Some(&client_id), Some(session_id))
        .await?;
    let session = sessions
        .first()
        .ok_or_else(|| ApiError::not_found("terminal_session_not_found"))?;
    state
        .repo
        .reconcile_terminal_job_by_id(session.job_id)
        .await?;
    let replay = state
        .repo
        .terminal_session_replay(
            &client_id,
            session_id,
            query.from_seq,
            query.limit.unwrap_or(DEFAULT_TERMINAL_REPLAY_LIMIT),
            query
                .max_bytes
                .unwrap_or(MAX_TERMINAL_REPLAY_BYTES)
                .clamp(1, MAX_TERMINAL_REPLAY_BYTES),
            query.include_data.unwrap_or(true),
        )
        .await?;
    Ok(Json(replay))
}

pub(crate) async fn control_terminal_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((client_id, session_id)): Path<(String, Uuid)>,
    Json(request): Json<TerminalControlSubmitRequest>,
) -> Result<Json<TerminalControlAck>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "jobs:write")
        .await?;
    if !operator_has_scope(&operator.operator.scopes, SCOPE_TERMINAL_READ) {
        return Err(ApiError::forbidden("operator_scope_insufficient"));
    }
    validate_terminal_replay_client_id(&client_id)?;
    if session_id.is_nil() {
        return Err(ApiError::bad_request("terminal_session_id_invalid"));
    }
    if request.request_id.is_nil() {
        return Err(ApiError::bad_request("terminal_control_request_id_invalid"));
    }
    validate_terminal_control_action(session_id, &request.action)?;
    let sessions = state
        .repo
        .list_terminal_sessions(1, Some(&client_id), Some(session_id))
        .await?;
    let session = sessions
        .first()
        .ok_or_else(|| ApiError::not_found("terminal_session_not_found"))?;
    if matches!(session.state.as_str(), "opening" | "open") {
        state
            .repo
            .reconcile_terminal_job_by_id(session.job_id)
            .await?;
    }
    let job_id = state
        .repo
        .authorize_terminal_control(&client_id, session_id, &operator)
        .await?;
    state
        .repo
        .reconcile_terminal_job(job_id, &client_id, "open")
        .await?;
    let agent = state
        .repo
        .agent_by_id(&client_id)
        .await
        .map_err(|_| ApiError::not_found("terminal_agent_not_found"))?;
    let process_incarnation_id = agent
        .process_incarnation_id
        .filter(|_| agent.status == "online")
        .ok_or_else(|| ApiError::conflict("terminal_agent_not_online"))?;
    let action_hash = payload_hash(
        &encode_json(&request.action)
            .map_err(|_| ApiError::bad_request("terminal_control_action_invalid"))?,
    );
    let action = request.action.clone();
    let result = state
        .gateway
        .terminal_control(
            &client_id,
            process_incarnation_id,
            TerminalControlRequest {
                request_id: request.request_id,
                session_id,
                action: request.action,
            },
        )
        .await
        .map_err(map_terminal_gateway_error)?;
    if result.client_id != client_id
        || result.ack.request_id != request.request_id
        || result.ack.session_id != session_id
        || result.ack.action != action.kind()
    {
        return Err(ApiError::conflict("terminal_control_ack_mismatch"));
    }
    validate_terminal_control_ack(&action, &result.ack)?;
    state
        .repo
        .record_terminal_control_ack(
            &operator,
            &client_id,
            job_id,
            &action,
            &action_hash,
            &result.ack,
        )
        .await?;
    if !result.ack.accepted {
        return Err(ApiError::conflict_with_message(
            "terminal_control_rejected",
            result.ack.message,
        ));
    }
    Ok(Json(result.ack))
}

fn validate_terminal_control_action(
    session_id: Uuid,
    action: &TerminalControlAction,
) -> Result<(), ApiError> {
    match action {
        TerminalControlAction::Input { data_base64 } => {
            if data_base64.is_empty()
                || data_base64.len() > MAX_TERMINAL_INPUT_BYTES.div_ceil(3) * 4 + 16
            {
                return Err(ApiError::bad_request("terminal_input_size_invalid"));
            }
            let data = BASE64_STANDARD
                .decode(data_base64.as_bytes())
                .map_err(|_| ApiError::bad_request("terminal_input_base64_invalid"))?;
            if data.is_empty() || data.len() > MAX_TERMINAL_INPUT_BYTES {
                return Err(ApiError::bad_request("terminal_input_size_invalid"));
            }
            Ok(())
        }
        TerminalControlAction::Resize { cols, rows } => {
            validate_terminal_resize(session_id, *cols, *rows)
        }
        TerminalControlAction::Close { reason } => {
            validate_terminal_close(session_id, reason.as_deref())
        }
    }
}

fn validate_terminal_control_ack(
    action: &TerminalControlAction,
    ack: &TerminalControlAck,
) -> Result<(), ApiError> {
    if !ack.accepted {
        if matches!(
            ack.status.as_str(),
            "rejected" | "missing" | "failed" | "exited"
        ) {
            return Ok(());
        }
        return Err(ApiError::conflict("terminal_control_ack_mismatch"));
    }
    let valid = match action {
        TerminalControlAction::Input { data_base64 } => {
            let expected_bytes = BASE64_STANDARD
                .decode(data_base64.as_bytes())
                .map_err(|_| ApiError::bad_request("terminal_input_base64_invalid"))?
                .len() as u64;
            ack.status == "accepted"
                && ack.input_seq.is_some_and(|input_seq| input_seq > 0)
                && ack.written_bytes == Some(expected_bytes)
                && ack.cols.is_none()
                && ack.rows.is_none()
        }
        TerminalControlAction::Resize { cols, rows } => {
            ack.status == "resized"
                && ack.cols == Some(*cols)
                && ack.rows == Some(*rows)
                && ack.input_seq.is_none()
                && ack.written_bytes.is_none()
        }
        TerminalControlAction::Close { .. } => {
            ack.status == "closed"
                && ack.input_seq.is_none()
                && ack.written_bytes.is_none()
                && ack.cols.is_none()
                && ack.rows.is_none()
        }
    };
    if valid {
        Ok(())
    } else {
        Err(ApiError::conflict("terminal_control_ack_mismatch"))
    }
}

fn map_terminal_gateway_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    let (status, code) =
        if message.contains("agent_not_online") || message.contains("agent_session_closed") {
            (StatusCode::CONFLICT, "terminal_agent_not_online")
        } else if message.contains("agent_incarnation_mismatch") {
            (StatusCode::CONFLICT, "terminal_agent_reconnected")
        } else if message.contains("queue_full") {
            (StatusCode::SERVICE_UNAVAILABLE, "terminal_control_busy")
        } else if message.contains("timed out") {
            (StatusCode::GATEWAY_TIMEOUT, "terminal_control_timeout")
        } else {
            (StatusCode::BAD_GATEWAY, "terminal_control_delivery_failed")
        };
    ApiError {
        status,
        code,
        error,
        public_message: None,
    }
}

fn validate_terminal_replay_client_id(client_id: &str) -> Result<(), ApiError> {
    if client_id.trim().is_empty() || client_id.len() > 128 || client_id.contains('/') {
        return Err(ApiError::bad_request("terminal_replay_client_id_invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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

        let legacy_input =
            serde_json::from_value::<TerminalControlSubmitRequest>(serde_json::json!({
                "job_id": Uuid::new_v4(),
                "text": "uptime\n",
                "confirmed": true
            }));
        assert!(legacy_input.is_err());

        let unknown_field =
            serde_json::from_value::<TerminalControlSubmitRequest>(serde_json::json!({
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
        let (headers, owner_id) =
            auth_headers(&state, &memory, &["jobs:write", "terminal:read"]).await;
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
        let (headers, owner_id) =
            auth_headers(&state, &memory, &["jobs:write", "terminal:read"]).await;
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
}
