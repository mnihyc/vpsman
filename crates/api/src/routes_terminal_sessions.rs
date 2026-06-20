use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::{
    canonical_terminal_input_privilege_intent, id_selector_expression, payload_hash, JobCommand,
    TerminalInputPrivilegeIntentInput, MAX_TERMINAL_INPUT_BYTES,
};

use crate::{
    error::ApiError,
    model::CreateJobRequest,
    model_terminal::{
        TerminalInputSubmitRequest, TerminalInputSubmitResponse, TerminalReplayView,
        TerminalSessionView,
    },
    privilege::verify_privilege_intent,
    routes_jobs::create_job_from_terminal_input_route,
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

pub(crate) async fn submit_terminal_session_input(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((client_id, session_id)): Path<(String, Uuid)>,
    Json(request): Json<TerminalInputSubmitRequest>,
) -> Result<(StatusCode, Json<TerminalInputSubmitResponse>), ApiError> {
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
    if request.job_id.is_nil() {
        return Err(ApiError::bad_request("job_id_required"));
    }
    if !request.confirmed {
        return Err(ApiError::conflict("terminal_input_confirmation_required"));
    }
    let timeout_secs = request.timeout_secs.unwrap_or(30).clamp(1, 3600);
    let data = terminal_input_request_data(&request)?;
    let data_base64 = BASE64_STANDARD.encode(&data);
    let input_payload_hash = payload_hash(&data);
    let session_id_text = session_id.to_string();
    let intent = canonical_terminal_input_privilege_intent(TerminalInputPrivilegeIntentInput {
        client_id: &client_id,
        session_id: &session_id_text,
        input_payload_hash: &input_payload_hash,
        timeout_secs,
        confirmed: request.confirmed,
    })
    .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
    verify_privilege_intent(&state, &intent, request.privilege_assertion.clone()).await?;
    let reservation = state
        .repo
        .reserve_terminal_input_request(
            &client_id,
            session_id,
            request.job_id,
            &input_payload_hash,
            i64::try_from(data.len())
                .map_err(|_| ApiError::bad_request("terminal_input_size_invalid"))?,
        )
        .await?;
    let selector_expression = id_selector_expression(&client_id);
    let job_request = CreateJobRequest {
        job_id: Some(request.job_id),
        selector_expression,
        target_client_ids: vec![client_id.clone()],
        destructive: true,
        confirmed: true,
        command: "terminal_input".to_string(),
        argv: Vec::new(),
        operation: Some(JobCommand::TerminalInput {
            session_id,
            input_seq: u64::try_from(reservation.input_seq)
                .map_err(|_| ApiError::bad_request("terminal_input_seq_out_of_range"))?,
            data_base64,
        }),
        timeout_secs: Some(timeout_secs),
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: None,
    };
    let result = create_job_from_terminal_input_route(&state, &operator, job_request).await;
    match result {
        Ok((status, Json(job))) => {
            let request_status = job.status.clone();
            state
                .repo
                .mark_terminal_input_request_status(request.job_id, &request_status)
                .await?;
            Ok((
                status,
                Json(TerminalInputSubmitResponse {
                    job,
                    input_seq: reservation.input_seq,
                    request_status,
                }),
            ))
        }
        Err(error) => {
            let _ = state
                .repo
                .mark_terminal_input_request_status(request.job_id, "failed")
                .await;
            Err(error)
        }
    }
}

fn terminal_input_request_data(request: &TerminalInputSubmitRequest) -> Result<Vec<u8>, ApiError> {
    match (&request.text, &request.data_base64) {
        (Some(_), Some(_)) => Err(ApiError::bad_request("terminal_input_data_ambiguous")),
        (Some(text), None) => validate_terminal_input_bytes(text.as_bytes().to_vec()),
        (None, Some(data_base64)) => {
            if data_base64.is_empty()
                || data_base64.len() > MAX_TERMINAL_INPUT_BYTES.div_ceil(3) * 4 + 16
            {
                return Err(ApiError::bad_request("terminal_input_size_invalid"));
            }
            let data = BASE64_STANDARD
                .decode(data_base64.as_bytes())
                .map_err(|_| ApiError::bad_request("terminal_input_base64_invalid"))?;
            validate_terminal_input_bytes(data)
        }
        (None, None) => Err(ApiError::bad_request("terminal_input_data_required")),
    }
}

fn validate_terminal_input_bytes(data: Vec<u8>) -> Result<Vec<u8>, ApiError> {
    if data.is_empty() || data.len() > MAX_TERMINAL_INPUT_BYTES {
        return Err(ApiError::bad_request("terminal_input_size_invalid"));
    }
    Ok(data)
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
        model::{AgentView, OperatorPreferences, OperatorRecord},
        model_terminal::TerminalSessionView,
        repository::{MemoryState, Repository},
    };
    use uuid::Uuid;
    use vpsman_common::{AgentCapabilitySnapshot, JobCommand};

    #[tokio::test]
    async fn terminal_input_route_assigns_sequence_and_creates_internal_job() {
        let (state, memory, session_id) = route_test_state().await;
        let headers = auth_headers(&state, &memory, &["jobs:write", "terminal:read"]).await;
        let job_id = Uuid::new_v4();

        let (status, Json(response)) = submit_terminal_session_input(
            State(state),
            headers,
            Path(("edge-a".to_string(), session_id)),
            Json(TerminalInputSubmitRequest {
                job_id,
                text: Some("uptime\n".to_string()),
                data_base64: None,
                timeout_secs: Some(30),
                confirmed: true,
                privilege_assertion: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(response.job.job_id, job_id);
        assert_eq!(response.job.target_count, 1);
        assert_eq!(response.input_seq, 3);

        let requests = memory.terminal_input_requests.read().await;
        let request = requests
            .iter()
            .find(|request| request.job_id == job_id)
            .unwrap();
        assert_eq!(request.client_id, "edge-a");
        assert_eq!(request.session_id, session_id);
        assert_eq!(request.input_seq, 3);
        assert_eq!(request.status, response.request_status);
        drop(requests);

        let operations = memory.job_operations.read().await;
        let operation = operations.get(&job_id).unwrap();
        assert!(matches!(
            operation,
            JobCommand::TerminalInput {
                session_id: recorded_session_id,
                input_seq: 3,
                data_base64,
            } if *recorded_session_id == session_id && data_base64 == "dXB0aW1lCg=="
        ));
    }

    #[tokio::test]
    async fn terminal_input_route_requires_terminal_scope_and_confirmation() {
        let (state, memory, session_id) = route_test_state().await;
        let missing_terminal_scope_headers = auth_headers(&state, &memory, &["jobs:write"]).await;

        let missing_scope = submit_terminal_session_input(
            State(state.clone()),
            missing_terminal_scope_headers,
            Path(("edge-a".to_string(), session_id)),
            Json(TerminalInputSubmitRequest {
                job_id: Uuid::new_v4(),
                text: Some("uptime\n".to_string()),
                data_base64: None,
                timeout_secs: Some(30),
                confirmed: true,
                privilege_assertion: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(missing_scope.status, StatusCode::FORBIDDEN);
        assert_eq!(missing_scope.code, "operator_scope_insufficient");
        assert!(memory.terminal_input_requests.read().await.is_empty());

        let headers = auth_headers(&state, &memory, &["jobs:write", "terminal:read"]).await;
        let missing_confirmation = submit_terminal_session_input(
            State(state),
            headers,
            Path(("edge-a".to_string(), session_id)),
            Json(TerminalInputSubmitRequest {
                job_id: Uuid::new_v4(),
                text: Some("uptime\n".to_string()),
                data_base64: None,
                timeout_secs: Some(30),
                confirmed: false,
                privilege_assertion: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(missing_confirmation.status, StatusCode::CONFLICT);
        assert_eq!(
            missing_confirmation.code,
            "terminal_input_confirmation_required"
        );
        assert!(memory.terminal_input_requests.read().await.is_empty());
    }

    async fn route_test_state() -> (AppState, MemoryState, Uuid) {
        let memory = MemoryState::default();
        let session_id = Uuid::new_v4();
        memory.agents.write().await.push(AgentView {
            id: "edge-a".to_string(),
            display_name: "edge-a".to_string(),
            status: "online".to_string(),
            tags: Vec::new(),
            registration_ip: None,
            last_ip: None,
            last_seen_at: Some("2026-06-21T00:00:00Z".to_string()),
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
            .push(test_terminal_session(session_id));
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
        (state, memory, session_id)
    }

    async fn auth_headers(state: &AppState, memory: &MemoryState, scopes: &[&str]) -> HeaderMap {
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
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        };
        let view = operator.view();
        memory.operators.write().await.push(operator);
        let auth = state.repo.issue_session(view).await.unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            format!("Bearer {}", auth.access_token).parse().unwrap(),
        );
        headers
    }

    fn test_terminal_session(session_id: Uuid) -> TerminalSessionView {
        TerminalSessionView {
            session_id,
            client_id: "edge-a".to_string(),
            state: "open".to_string(),
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
            last_input_seq: Some(2),
            session_exited: false,
            close_reason: None,
            last_event: "accepted".to_string(),
            last_job_id: Uuid::new_v4(),
            last_command_type: "terminal_open".to_string(),
            last_seq: 0,
            observed_at: "2026-06-21T00:00:00Z".to_string(),
        }
    }
}
