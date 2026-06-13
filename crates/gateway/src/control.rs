use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, UnixListener},
    sync::{mpsc, oneshot},
    time,
};
use tracing::{info, warn};
use vpsman_common::{
    verify_privilege_assertion, GatewayCommandCancel, GatewayCommandCancelResult,
    GatewayCommandDispatch, GatewayCommandDispatchResult, GatewayPrivilegeVerification,
    GatewayPrivilegeVerificationResult, PrivilegeAssertionError,
};

use crate::{
    state::{GatewayCancelCommand, GatewayCommand, GatewaySessionMessage, GatewayState},
    Args,
};

const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn run_control_listener(args: Args, state: GatewayState) -> Result<()> {
    if let Some(path) = control_socket_path(&args.control_bind) {
        prepare_control_socket(&path)?;
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("failed to bind gateway control socket {}", path.display()))?;
        info!(path = %path.display(), "gateway control listening on Unix socket");

        loop {
            let (stream, _) = listener.accept().await?;
            let state = state.clone();
            let internal_token = args.internal_token.clone();
            let privilege_verifier_key_hex = args.privilege_verifier_key_hex.clone();
            tokio::spawn(async move {
                if let Err(error) = handle_control_connection(
                    stream,
                    state,
                    internal_token,
                    privilege_verifier_key_hex,
                )
                .await
                {
                    warn!(%error, "gateway Unix control request failed");
                }
            });
        }
    }
    let listener = TcpListener::bind(&args.control_bind)
        .await
        .with_context(|| format!("failed to bind gateway control on {}", args.control_bind))?;
    info!(bind = %args.control_bind, "gateway control listening");

    loop {
        let (stream, peer) = listener.accept().await?;
        let state = state.clone();
        let internal_token = args.internal_token.clone();
        let privilege_verifier_key_hex = args.privilege_verifier_key_hex.clone();
        tokio::spawn(async move {
            if let Err(error) =
                handle_control_connection(stream, state, internal_token, privilege_verifier_key_hex)
                    .await
            {
                warn!(%peer, %error, "gateway control request failed");
            }
        });
    }
}

fn control_socket_path(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    value
        .strip_prefix("unix://")
        .or_else(|| value.strip_prefix("unix:"))
        .map(PathBuf::from)
}

fn prepare_control_socket(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create control socket directory {}",
                parent.display()
            )
        })?;
    }
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("failed to remove stale control socket {}", path.display()))?;
    }
    Ok(())
}

async fn handle_control_connection<S>(
    mut stream: S,
    state: GatewayState,
    internal_token: Option<String>,
    privilege_verifier_key_hex: Option<String>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request = read_http_request(&mut stream).await?;
    if request.method != "POST"
        || !matches!(
            request.path.as_str(),
            "/internal/v1/gateway/command"
                | "/internal/v1/gateway/command/cancel"
                | "/internal/v1/gateway/metrics"
                | "/internal/v1/gateway/privilege/verify"
        )
    {
        write_http_json(
            &mut stream,
            "404 Not Found",
            &serde_json::json!({"error": "not_found"}),
        )
        .await?;
        return Ok(());
    }
    if !authorized_internal_request(&request.headers, internal_token.as_deref()) {
        write_http_json(
            &mut stream,
            "401 Unauthorized",
            &serde_json::json!({"error": "invalid_internal_token"}),
        )
        .await?;
        return Ok(());
    }

    if request.path == "/internal/v1/gateway/command" {
        let dispatch: GatewayCommandDispatch = match serde_json::from_slice(&request.body) {
            Ok(dispatch) => dispatch,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": format!("invalid_command_dispatch:{error}")}),
                )
                .await?;
                return Ok(());
            }
        };
        match dispatch_gateway_command(&state, dispatch).await {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => write_gateway_error(&mut stream, error).await?,
        }
    } else if request.path == "/internal/v1/gateway/command/cancel" {
        let cancel: GatewayCommandCancel = match serde_json::from_slice(&request.body) {
            Ok(cancel) => cancel,
            Err(error) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &serde_json::json!({"error": format!("invalid_command_cancel:{error}")}),
                )
                .await?;
                return Ok(());
            }
        };
        match cancel_gateway_command(&state, cancel).await {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => write_gateway_error(&mut stream, error).await?,
        }
    } else if request.path == "/internal/v1/gateway/metrics" {
        write_http_json(&mut stream, "200 OK", &state.forward_metrics.snapshot()).await?;
    } else {
        let verification: GatewayPrivilegeVerification = match serde_json::from_slice(&request.body)
        {
            Ok(verification) => verification,
            Err(error) => {
                write_http_json(
                        &mut stream,
                        "400 Bad Request",
                        &serde_json::json!({"error": format!("invalid_privilege_verification:{error}")}),
                    )
                    .await?;
                return Ok(());
            }
        };
        match verify_gateway_privilege(&state, privilege_verifier_key_hex.as_deref(), verification)
            .await
        {
            Ok(result) => write_http_json(&mut stream, "200 OK", &result).await?,
            Err(error) => write_privilege_error(&mut stream, error).await?,
        }
    }
    Ok(())
}

async fn verify_gateway_privilege(
    state: &GatewayState,
    verifier_key_hex: Option<&str>,
    verification: GatewayPrivilegeVerification,
) -> Result<GatewayPrivilegeVerificationResult> {
    let verifier_key = decode_verifier_key(verifier_key_hex)?;
    let now_unix = unix_now();
    let mut replay_cache = state.privilege_assertions.lock().await;
    let intent_hash_hex = verify_privilege_assertion(
        &verifier_key,
        &verification.intent,
        &verification.assertion,
        now_unix,
        &mut replay_cache,
    )
    .map_err(|error| anyhow!("privilege_assertion_{error:?}"))?;
    Ok(GatewayPrivilegeVerificationResult {
        approved: true,
        intent_hash_hex,
        message: "approved".to_string(),
    })
}

fn decode_verifier_key(value: Option<&str>) -> Result<[u8; 32]> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("privilege verifier is not configured")?;
    let bytes = hex::decode(value).context("privilege verifier key must be hex")?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("privilege verifier key must be 32 bytes"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

async fn dispatch_gateway_command(
    state: &GatewayState,
    dispatch: GatewayCommandDispatch,
) -> Result<GatewayCommandDispatchResult> {
    let grace_deadline = state
        .disconnected_at
        .read()
        .await
        .get(&dispatch.client_id)
        .map(|disconnected| *disconnected + Duration::from_secs(state.reconnect_grace_secs));

    loop {
        if let Some(sender) = state
            .sessions
            .read()
            .await
            .get(&dispatch.client_id)
            .map(|session| session.sender.clone())
        {
            let (response_tx, response_rx) = oneshot::channel();
            sender
                .try_send(GatewaySessionMessage::Command(GatewayCommand {
                    request: dispatch.request.clone(),
                    response: response_tx,
                }))
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(_) => {
                        anyhow!("agent_session_command_queue_full:{}", dispatch.client_id)
                    }
                    mpsc::error::TrySendError::Closed(_) => {
                        anyhow!("agent_session_closed:{}", dispatch.client_id)
                    }
                })?;
            return time::timeout(Duration::from_secs(state.dispatch_ack_secs), response_rx)
                .await
                .context("gateway command ack timed out")?
                .context("gateway command response dropped");
        }
        match grace_deadline {
            Some(deadline) if std::time::Instant::now() < deadline => {
                time::sleep(Duration::from_millis(500)).await;
                continue;
            }
            _ => {
                return Err(anyhow!("agent_not_online:{}", dispatch.client_id));
            }
        }
    }
}

async fn cancel_gateway_command(
    state: &GatewayState,
    cancel: GatewayCommandCancel,
) -> Result<GatewayCommandCancelResult> {
    let Some(sender) = state
        .sessions
        .read()
        .await
        .get(&cancel.client_id)
        .map(|session| session.sender.clone())
    else {
        return Ok(GatewayCommandCancelResult {
            client_id: cancel.client_id,
            job_id: cancel.request.job_id,
            acked: false,
            accepted: false,
            applied: false,
            message: "agent_not_online".to_string(),
        });
    };
    let (response_tx, response_rx) = oneshot::channel();
    sender
        .try_send(GatewaySessionMessage::Cancel(GatewayCancelCommand {
            request: cancel.request.clone(),
            response: response_tx,
        }))
        .map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => {
                anyhow!("agent_session_command_queue_full:{}", cancel.client_id)
            }
            mpsc::error::TrySendError::Closed(_) => {
                anyhow!("agent_session_closed:{}", cancel.client_id)
            }
        })?;
    time::timeout(Duration::from_secs(state.dispatch_ack_secs), response_rx)
        .await
        .context("gateway command cancel timed out")?
        .context("gateway command cancel response dropped")
}

async fn write_gateway_error<S>(stream: &mut S, error: anyhow::Error) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let message = error.to_string();
    let status = if message.contains("agent_not_online") {
        "404 Not Found"
    } else if message.contains("agent_session_command_queue_full") {
        "503 Service Unavailable"
    } else if message.contains("timed out") {
        "504 Gateway Timeout"
    } else {
        "500 Internal Server Error"
    };
    write_http_json(stream, status, &serde_json::json!({"error": message})).await
}

async fn write_privilege_error<S>(stream: &mut S, error: anyhow::Error) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let message = error.to_string();
    let status = if message.contains("not configured") {
        "503 Service Unavailable"
    } else if message.contains(&format!("{:?}", PrivilegeAssertionError::Replay)) {
        "409 Conflict"
    } else if message.contains("privilege_assertion_") {
        "403 Forbidden"
    } else {
        "500 Internal Server Error"
    };
    write_http_json(stream, status, &serde_json::json!({"error": message})).await
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

async fn read_http_request<S>(stream: &mut S) -> Result<HttpRequest>
where
    S: AsyncRead + Unpin,
{
    let mut buffer = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let read = time::timeout(HTTP_READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .context("HTTP header read timed out")??;
        if read == 0 {
            return Err(anyhow!("connection closed before HTTP headers"));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_header_end(&buffer) {
            break position;
        }
        if buffer.len() > 64 * 1024 {
            return Err(anyhow!("HTTP headers too large"));
        }
    };
    let header_bytes = &buffer[..header_end];
    let headers_text = std::str::from_utf8(header_bytes).context("HTTP headers are not UTF-8")?;
    let mut lines = headers_text.split("\r\n");
    let request_line = lines.next().context("missing request line")?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let path = request_parts.next().unwrap_or_default().to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<Vec<_>>();
    let content_length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > 16 * 1024 * 1024 {
        return Err(anyhow!("HTTP body too large"));
    }
    let body_start = header_end + 4;
    let mut body = buffer[body_start..].to_vec();
    while body.len() < content_length {
        let mut chunk = vec![0_u8; content_length - body.len()];
        let read = time::timeout(HTTP_READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .context("HTTP body read timed out")??;
        if read == 0 {
            return Err(anyhow!("connection closed before HTTP body"));
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn authorized_internal_request(headers: &[(String, String)], internal_token: Option<&str>) -> bool {
    let Some(expected) = internal_token else {
        return false;
    };
    headers
        .iter()
        .find(|(name, _)| name == "authorization")
        .and_then(|(_, value)| value.strip_prefix("Bearer "))
        .is_some_and(|provided| constant_time_eq(provided.as_bytes(), expected.as_bytes()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (left, right) in left.iter().zip(right.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

async fn write_http_json<S, T>(stream: &mut S, status: &str, value: &T) -> Result<()>
where
    S: AsyncWrite + Unpin,
    T: serde::Serialize,
{
    let body = serde_json::to_vec(value)?;
    let response = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(&body).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{GatewaySession, SESSION_COMMAND_QUEUE_CAPACITY};
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
                .try_send(GatewaySessionMessage::Command(GatewayCommand {
                    request: test_job_request(),
                    response,
                }))
                .unwrap();
        }
        state.sessions.write().await.insert(
            "client-a".to_string(),
            GatewaySession {
                session_id: uuid::Uuid::new_v4(),
                sender,
            },
        );

        let error = dispatch_gateway_command(
            &state,
            GatewayCommandDispatch {
                client_id: "client-a".to_string(),
                request: test_job_request(),
            },
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("agent_session_command_queue_full:client-a"));
    }

    fn test_job_request() -> JobRequest {
        JobRequest {
            job_id: uuid::Uuid::new_v4(),
            command_version: 1,
            command: JobCommand::Shell {
                argv: vec!["/bin/true".to_string()],
                pty: false,
            },
            timeout_secs: 30,
        }
    }
}
