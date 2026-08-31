use std::{
    error::Error as StdError,
    fmt,
    sync::{Arc, RwLock as StdRwLock},
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
#[cfg(test)]
use base64::Engine as _;
use serde::de::DeserializeOwned;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpStream, UnixStream},
    time,
};
#[cfg(test)]
use vpsman_common::GatewayClientSuspensionFenceResult;
use vpsman_common::{
    GatewayClientSuspensionFenceBatchResult, GatewayClientSuspensionFenceClear,
    GatewayClientSuspensionFenceClearBatchRequest, GatewayClientSuspensionFencePrepare,
    GatewayClientSuspensionFencePrepareBatchRequest, GatewayClientSuspensionFencePromote,
    GatewayClientSuspensionFencePromoteBatchRequest, GatewayCommandCancel,
    GatewayCommandCancelResult, GatewayCommandDispatch, GatewayCommandDispatchResult,
    GatewayForwardMetricsSnapshot, GatewayPrivilegeVerification,
    GatewayPrivilegeVerificationBatchItem, GatewayPrivilegeVerificationBatchRequest,
    GatewayPrivilegeVerificationBatchResult, GatewayPrivilegeVerificationResult,
    GatewaySessionDisconnect, GatewaySessionDisconnectBatchRequest,
    GatewaySessionDisconnectBatchResult, GatewaySessionDisconnectResult, GatewayTerminalControl,
    GatewayTerminalControlResult, JobCancelRequest, JobRequest, PrivilegeAssertion,
    TerminalControlRequest,
};

const CONTROL_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct GatewayControlResponseError {
    pub(crate) status_code: u16,
    status_line: String,
    response_body: String,
}

impl fmt::Display for GatewayControlResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "gateway control returned {}: {}",
            self.status_line, self.response_body
        )
    }
}

impl StdError for GatewayControlResponseError {}

#[derive(Clone, Debug, Default)]
pub(crate) struct GatewayDispatchClient {
    control_url: Option<String>,
    internal_token: Option<String>,
    timeouts: Arc<StdRwLock<GatewayClientTimeouts>>,
    #[cfg(test)]
    test_privilege_auto_approve: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GatewayClientTimeouts {
    pub(crate) connect: Duration,
    pub(crate) write: Duration,
    pub(crate) read: Duration,
}

impl Default for GatewayClientTimeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            write: Duration::from_secs(10),
            read: Duration::from_secs(30),
        }
    }
}

impl GatewayDispatchClient {
    pub(crate) fn new_with_timeouts(
        control_url: Option<String>,
        internal_token: Option<String>,
        timeouts: GatewayClientTimeouts,
    ) -> Self {
        Self {
            control_url: control_url
                .map(|url| url.trim_end_matches('/').to_string())
                .filter(|url| !url.is_empty()),
            internal_token: internal_token
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty()),
            timeouts: Arc::new(StdRwLock::new(timeouts)),
            #[cfg(test)]
            test_privilege_auto_approve: false,
        }
    }

    pub(crate) fn configured(&self) -> bool {
        self.control_url.is_some()
    }

    pub(crate) fn privilege_configured(&self) -> bool {
        self.control_url.is_some() || {
            #[cfg(test)]
            {
                self.test_privilege_auto_approve
            }
            #[cfg(not(test))]
            {
                false
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn test_privilege_auto_approves(&self) -> bool {
        self.test_privilege_auto_approve
    }

    #[cfg(test)]
    pub(crate) fn with_test_privilege_auto_approve(mut self) -> Self {
        self.test_privilege_auto_approve = true;
        self
    }

    pub(crate) fn set_read_timeout(&self, read: Duration) {
        if let Ok(mut timeouts) = self.timeouts.write() {
            timeouts.read = read;
        }
    }

    fn timeouts(&self) -> GatewayClientTimeouts {
        self.timeouts
            .read()
            .map(|timeouts| *timeouts)
            .unwrap_or_default()
    }

    pub(crate) async fn dispatch(
        &self,
        client_id: &str,
        request: JobRequest,
        expected_process_incarnation_id: uuid::Uuid,
        payload_hash: String,
        timeouts: GatewayClientTimeouts,
    ) -> Result<GatewayCommandDispatchResult> {
        let control_url = self
            .control_url
            .as_deref()
            .context("gateway control URL is not configured")?;
        post_gateway_command(
            control_url,
            &GatewayCommandDispatch {
                client_id: client_id.to_string(),
                request,
                expected_process_incarnation_id,
                payload_hash,
            },
            self.internal_token.as_deref(),
            timeouts,
        )
        .await
    }

    pub(crate) async fn cancel(
        &self,
        client_id: &str,
        request: JobCancelRequest,
    ) -> Result<GatewayCommandCancelResult> {
        let control_url = self
            .control_url
            .as_deref()
            .context("gateway control URL is not configured")?;
        post_gateway_control(
            control_url,
            "/internal/v1/gateway/command/cancel",
            &GatewayCommandCancel {
                client_id: client_id.to_string(),
                request,
            },
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await
    }

    pub(crate) async fn terminal_control(
        &self,
        client_id: &str,
        expected_process_incarnation_id: uuid::Uuid,
        request: TerminalControlRequest,
    ) -> Result<GatewayTerminalControlResult> {
        #[cfg(test)]
        if self.test_privilege_auto_approve {
            let action = request.action.kind().to_string();
            let (status, input_seq, written_bytes, cols, rows) = match &request.action {
                vpsman_common::TerminalControlAction::Input { data_base64 } => (
                    "accepted",
                    Some(1),
                    base64::engine::general_purpose::STANDARD
                        .decode(data_base64)
                        .ok()
                        .map(|data| data.len() as u64),
                    None,
                    None,
                ),
                vpsman_common::TerminalControlAction::Resize { cols, rows } => {
                    ("resized", None, None, Some(*cols), Some(*rows))
                }
                vpsman_common::TerminalControlAction::Close { .. } => {
                    ("closed", None, None, None, None)
                }
            };
            return Ok(GatewayTerminalControlResult {
                client_id: client_id.to_string(),
                ack: vpsman_common::TerminalControlAck {
                    request_id: request.request_id,
                    session_id: request.session_id,
                    action,
                    accepted: true,
                    status: status.to_string(),
                    message: "test terminal control accepted".to_string(),
                    input_seq,
                    written_bytes,
                    cols,
                    rows,
                },
            });
        }
        let control_url = self
            .control_url
            .as_deref()
            .context("gateway control URL is not configured")?;
        post_gateway_control(
            control_url,
            "/internal/v1/gateway/terminal/control",
            &GatewayTerminalControl {
                client_id: client_id.to_string(),
                expected_process_incarnation_id,
                request,
            },
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await
    }

    pub(crate) async fn disconnect_session(
        &self,
        client_id: &str,
        reason: &str,
    ) -> Result<GatewaySessionDisconnectResult> {
        #[cfg(test)]
        if self.test_privilege_auto_approve {
            return Ok(GatewaySessionDisconnectResult {
                client_id: client_id.to_string(),
                accepted: true,
                disconnected: false,
                message: "test gateway disconnect auto-approved".to_string(),
            });
        }
        let control_url = self
            .control_url
            .as_deref()
            .context("gateway control URL is not configured")?;
        post_gateway_control(
            control_url,
            "/internal/v1/gateway/session/disconnect",
            &GatewaySessionDisconnect {
                client_id: client_id.to_string(),
                reason: reason.to_string(),
            },
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await
    }

    pub(crate) async fn disconnect_sessions(
        &self,
        items: Vec<GatewaySessionDisconnect>,
    ) -> Result<GatewaySessionDisconnectBatchResult> {
        #[cfg(test)]
        if self.test_privilege_auto_approve {
            return Ok(GatewaySessionDisconnectBatchResult {
                results: items
                    .into_iter()
                    .map(|item| GatewaySessionDisconnectResult {
                        client_id: item.client_id,
                        accepted: true,
                        disconnected: false,
                        message: "test gateway disconnect auto-approved".to_string(),
                    })
                    .collect(),
            });
        }
        let control_url = self
            .control_url
            .as_deref()
            .context("gateway control URL is not configured")?;
        post_gateway_control(
            control_url,
            "/internal/v1/gateway/session/disconnect/batch",
            &GatewaySessionDisconnectBatchRequest { items },
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await
    }

    pub(crate) async fn prepare_client_suspension_fences(
        &self,
        items: Vec<GatewayClientSuspensionFencePrepare>,
    ) -> Result<GatewayClientSuspensionFenceBatchResult> {
        #[cfg(test)]
        if self.test_privilege_auto_approve {
            return Ok(GatewayClientSuspensionFenceBatchResult {
                results: items
                    .into_iter()
                    .map(|item| GatewayClientSuspensionFenceResult {
                        client_id: item.client_id,
                        accepted: true,
                        fenced: true,
                        message: "test suspension fence prepared".to_string(),
                        enqueued_job_ids: Vec::new(),
                    })
                    .collect(),
            });
        }
        let control_url = self
            .control_url
            .as_deref()
            .context("gateway control URL is not configured")?;
        post_gateway_control(
            control_url,
            "/internal/v1/gateway/client/suspension-fence/batch/prepare",
            &GatewayClientSuspensionFencePrepareBatchRequest { items },
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await
    }

    pub(crate) async fn promote_client_suspension_fences(
        &self,
        items: Vec<GatewayClientSuspensionFencePromote>,
    ) -> Result<GatewayClientSuspensionFenceBatchResult> {
        #[cfg(test)]
        if self.test_privilege_auto_approve {
            return Ok(GatewayClientSuspensionFenceBatchResult {
                results: items
                    .into_iter()
                    .map(|item| GatewayClientSuspensionFenceResult {
                        client_id: item.client_id,
                        accepted: true,
                        fenced: true,
                        message: "test suspension fence promoted".to_string(),
                        enqueued_job_ids: Vec::new(),
                    })
                    .collect(),
            });
        }
        let control_url = self
            .control_url
            .as_deref()
            .context("gateway control URL is not configured")?;
        post_gateway_control(
            control_url,
            "/internal/v1/gateway/client/suspension-fence/batch/promote",
            &GatewayClientSuspensionFencePromoteBatchRequest { items },
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await
    }

    pub(crate) async fn clear_client_suspension_fences(
        &self,
        items: Vec<GatewayClientSuspensionFenceClear>,
    ) -> Result<GatewayClientSuspensionFenceBatchResult> {
        #[cfg(test)]
        if self.test_privilege_auto_approve {
            return Ok(GatewayClientSuspensionFenceBatchResult {
                results: items
                    .into_iter()
                    .map(|item| GatewayClientSuspensionFenceResult {
                        client_id: item.client_id,
                        accepted: true,
                        fenced: false,
                        message: "test suspension fence cleared".to_string(),
                        enqueued_job_ids: Vec::new(),
                    })
                    .collect(),
            });
        }
        let control_url = self
            .control_url
            .as_deref()
            .context("gateway control URL is not configured")?;
        post_gateway_control(
            control_url,
            "/internal/v1/gateway/client/suspension-fence/batch/clear",
            &GatewayClientSuspensionFenceClearBatchRequest { items },
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await
    }

    pub(crate) async fn verify_privilege(
        &self,
        intent: String,
        assertion: PrivilegeAssertion,
    ) -> Result<GatewayPrivilegeVerificationResult> {
        #[cfg(test)]
        if self.test_privilege_auto_approve {
            let _ = (intent, assertion);
            return Ok(GatewayPrivilegeVerificationResult {
                approved: true,
                intent_hash_hex: "test-auto-approved".to_string(),
                message: "test privilege auto-approved".to_string(),
            });
        }
        let control_url = self
            .control_url
            .as_deref()
            .context("gateway control URL is not configured")?;
        post_gateway_control(
            control_url,
            "/internal/v1/gateway/privilege/verify",
            &GatewayPrivilegeVerification { intent, assertion },
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await
    }

    pub(crate) async fn verify_privileges(
        &self,
        items: Vec<GatewayPrivilegeVerificationBatchItem>,
    ) -> Result<GatewayPrivilegeVerificationBatchResult> {
        #[cfg(test)]
        if self.test_privilege_auto_approve {
            return Ok(GatewayPrivilegeVerificationBatchResult {
                results: items
                    .into_iter()
                    .map(
                        |item| vpsman_common::GatewayPrivilegeVerificationBatchItemResult {
                            request_id: item.request_id,
                            approved: true,
                            intent_hash_hex: Some("test-auto-approved".to_string()),
                            message: "test privilege auto-approved".to_string(),
                            error_code: None,
                        },
                    )
                    .collect(),
            });
        }
        let control_url = self
            .control_url
            .as_deref()
            .context("gateway control URL is not configured")?;
        post_gateway_control(
            control_url,
            "/internal/v1/gateway/privilege/verify/batch",
            &GatewayPrivilegeVerificationBatchRequest { items },
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await
    }

    pub(crate) async fn forward_metrics(&self) -> Result<GatewayForwardMetricsSnapshot> {
        let control_url = self
            .control_url
            .as_deref()
            .context("gateway control URL is not configured")?;
        post_gateway_control(
            control_url,
            "/internal/v1/gateway/metrics",
            &serde_json::json!({}),
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await
    }
}

async fn post_gateway_command(
    control_url: &str,
    dispatch: &GatewayCommandDispatch,
    internal_token: Option<&str>,
    timeouts: GatewayClientTimeouts,
) -> Result<GatewayCommandDispatchResult> {
    post_gateway_control(
        control_url,
        "/internal/v1/gateway/command",
        dispatch,
        internal_token,
        timeouts,
    )
    .await
}

async fn post_gateway_control<T, R>(
    control_url: &str,
    request_path_suffix: &str,
    body_value: &T,
    internal_token: Option<&str>,
    timeouts: GatewayClientTimeouts,
) -> Result<R>
where
    T: serde::Serialize,
    R: DeserializeOwned,
{
    if let Some(path) = control_url
        .strip_prefix("unix://")
        .or_else(|| control_url.strip_prefix("unix:"))
    {
        let body = serde_json::to_vec(body_value)?;
        let token = internal_token.context("gateway internal token is not configured")?;
        let mut stream = time::timeout(timeouts.connect, UnixStream::connect(path))
            .await
            .context("gateway control socket connect timed out")?
            .with_context(|| format!("failed to connect gateway control socket at {path}"))?;
        return send_gateway_control_request(
            &mut stream,
            "gateway-control",
            request_path_suffix,
            &body,
            token,
            timeouts,
        )
        .await;
    }
    let without_scheme = control_url
        .strip_prefix("http://")
        .context("gateway control URL currently supports http:// or unix: URLs")?;
    let (host_port, prefix) = without_scheme
        .split_once('/')
        .map(|(host, rest)| (host, format!("/{rest}")))
        .unwrap_or((without_scheme, String::new()));
    let request_path = format!("{prefix}{request_path_suffix}");
    let body = serde_json::to_vec(body_value)?;
    let token = internal_token.context("gateway internal token is not configured")?;
    let mut stream = time::timeout(timeouts.connect, TcpStream::connect(host_port))
        .await
        .context("gateway control connect timed out")?
        .with_context(|| format!("failed to connect gateway control at {host_port}"))?;
    send_gateway_control_request(
        &mut stream,
        host_port,
        &request_path,
        &body,
        token,
        timeouts,
    )
    .await
}

async fn send_gateway_control_request<S, R>(
    stream: &mut S,
    host: &str,
    request_path: &str,
    body: &[u8],
    token: &str,
    timeouts: GatewayClientTimeouts,
) -> Result<R>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: DeserializeOwned,
{
    let auth_header = format!("Authorization: Bearer {token}\r\n");
    let request = format!(
        "POST {request_path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{auth_header}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    time::timeout(timeouts.write, stream.write_all(request.as_bytes()))
        .await
        .context("gateway control request header write timed out")??;
    time::timeout(timeouts.write, stream.write_all(body))
        .await
        .context("gateway control request body write timed out")??;

    let mut response = Vec::new();
    time::timeout(timeouts.read, stream.read_to_end(&mut response))
        .await
        .context("gateway control response read timed out")??;
    if response.len() > CONTROL_MAX_RESPONSE_BYTES {
        return Err(anyhow!("gateway control response too large"));
    }
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .context("invalid gateway control response")?;
    let headers = std::str::from_utf8(&response[..header_end])
        .context("gateway control response headers are not UTF-8")?;
    let status_line = headers
        .lines()
        .next()
        .context("missing gateway control status line")?;
    let status_code = status_line
        .split_ascii_whitespace()
        .nth(1)
        .context("missing gateway control status code")?
        .parse::<u16>()
        .context("invalid gateway control status code")?;
    let body = &response[header_end + 4..];
    if !(200..300).contains(&status_code) {
        return Err(GatewayControlResponseError {
            status_code,
            status_line: status_line.to_string(),
            response_body: String::from_utf8_lossy(body).into_owned(),
        }
        .into());
    }
    Ok(serde_json::from_slice(body)?)
}

#[cfg(test)]
#[path = "tests_gateway_client.rs"]
mod tests;
