use std::{
    sync::{Arc, RwLock as StdRwLock},
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpStream, UnixStream},
    time,
};
use vpsman_common::{
    GatewayCommandCancel, GatewayCommandCancelResult, GatewayCommandDispatch,
    GatewayCommandDispatchResult, GatewayForwardMetricsSnapshot, GatewayPrivilegeVerification,
    GatewayPrivilegeVerificationResult, JobCancelRequest, JobRequest, PrivilegeAssertion,
};

const CONTROL_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

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
    #[cfg(test)]
    pub(crate) fn new(control_url: Option<String>, internal_token: Option<String>) -> Self {
        Self::new_with_timeouts(
            control_url,
            internal_token,
            GatewayClientTimeouts::default(),
        )
    }

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
    pub(crate) fn test_privilege_auto_approve() -> Self {
        Self {
            control_url: None,
            internal_token: None,
            timeouts: Arc::new(StdRwLock::new(GatewayClientTimeouts::default())),
            test_privilege_auto_approve: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_privilege_auto_approve(mut self) -> Self {
        self.test_privilege_auto_approve = true;
        self
    }

    #[cfg(test)]
    pub(crate) fn test_privilege_auto_approves(&self) -> bool {
        self.test_privilege_auto_approve
    }

    #[cfg(test)]
    pub(crate) fn test_timeouts(&self) -> GatewayClientTimeouts {
        self.timeouts()
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
            self.timeouts(),
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
    let status = headers
        .lines()
        .next()
        .context("missing gateway control status line")?;
    let body = &response[header_end + 4..];
    if !status.contains(" 2") {
        return Err(anyhow!(
            "gateway control returned {status}: {}",
            String::from_utf8_lossy(body)
        ));
    }
    Ok(serde_json::from_slice(body)?)
}
