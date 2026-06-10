use anyhow::{anyhow, Context, Result};
use serde::de::DeserializeOwned;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use vpsman_common::{
    GatewayCommandDispatch, GatewayCommandDispatchResult, GatewayPrivilegeVerification,
    GatewayPrivilegeVerificationResult, JobRequest, PrivilegeAssertion,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct GatewayDispatchClient {
    control_url: Option<String>,
    internal_token: Option<String>,
    #[cfg(test)]
    test_privilege_auto_approve: bool,
}

impl GatewayDispatchClient {
    pub(crate) fn new(control_url: Option<String>, internal_token: Option<String>) -> Self {
        Self {
            control_url: control_url
                .map(|url| url.trim_end_matches('/').to_string())
                .filter(|url| !url.is_empty()),
            internal_token: internal_token
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty()),
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

    pub(crate) async fn dispatch(
        &self,
        client_id: &str,
        request: JobRequest,
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
            },
            self.internal_token.as_deref(),
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
        )
        .await
    }
}

async fn post_gateway_command(
    control_url: &str,
    dispatch: &GatewayCommandDispatch,
    internal_token: Option<&str>,
) -> Result<GatewayCommandDispatchResult> {
    post_gateway_control(
        control_url,
        "/internal/v1/gateway/command",
        dispatch,
        internal_token,
    )
    .await
}

async fn post_gateway_control<T, R>(
    control_url: &str,
    request_path_suffix: &str,
    body_value: &T,
    internal_token: Option<&str>,
) -> Result<R>
where
    T: serde::Serialize,
    R: DeserializeOwned,
{
    let without_scheme = control_url
        .strip_prefix("http://")
        .context("gateway control URL currently supports http:// URLs")?;
    let (host_port, prefix) = without_scheme
        .split_once('/')
        .map(|(host, rest)| (host, format!("/{rest}")))
        .unwrap_or((without_scheme, String::new()));
    let request_path = format!("{prefix}{request_path_suffix}");
    let body = serde_json::to_vec(body_value)?;
    let token = internal_token.context("gateway internal token is not configured")?;
    let auth_header = format!("Authorization: Bearer {token}\r\n");
    let mut stream = TcpStream::connect(host_port)
        .await
        .with_context(|| format!("failed to connect gateway control at {host_port}"))?;
    let request = format!(
        "POST {request_path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n{auth_header}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(request.as_bytes()).await?;
    stream.write_all(&body).await?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
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
