use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tracing::warn;

#[derive(Clone, Debug)]
pub(crate) struct GatewayControlClient {
    api_url: Option<String>,
    internal_token: Option<String>,
}

impl GatewayControlClient {
    pub(crate) fn new(api_url: Option<String>, internal_token: Option<String>) -> Self {
        Self {
            api_url: api_url.map(|url| url.trim_end_matches('/').to_string()),
            internal_token: internal_token
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty()),
        }
    }

    pub(crate) async fn post<T: serde::Serialize>(&self, path: &str, value: &T) {
        let Some(api_url) = &self.api_url else {
            return;
        };
        if let Err(error) = post_json(api_url, path, value, self.internal_token.as_deref()).await {
            warn!(%error, path, "failed to forward gateway event to API");
        }
    }

    pub(crate) async fn validate_agent_identity(
        &self,
        client_id: &str,
        noise_public_key_hex: &str,
    ) -> Result<GatewayIdentityValidationResponse> {
        let Some(api_url) = &self.api_url else {
            anyhow::bail!("enrolled IK identity validation requires VPSMAN_API_URL or a static expected client key");
        };
        let body = post_json(
            api_url,
            "/internal/v1/gateway/agent-identity",
            &GatewayIdentityValidationRequest {
                client_id: client_id.to_string(),
                noise_public_key_hex: noise_public_key_hex.to_string(),
            },
            self.internal_token.as_deref(),
        )
        .await?;
        serde_json::from_str(&body).context("failed to parse gateway identity validation response")
    }
}

async fn post_json<T: serde::Serialize>(
    base_url: &str,
    path: &str,
    value: &T,
    internal_token: Option<&str>,
) -> Result<String> {
    let without_scheme = base_url
        .strip_prefix("http://")
        .context("gateway internal API forwarding currently supports http:// URLs")?;
    let (host_port, prefix) = without_scheme
        .split_once('/')
        .map(|(host, rest)| (host, format!("/{rest}")))
        .unwrap_or((without_scheme, String::new()));
    let request_path = format!("{prefix}{path}");
    let body = serde_json::to_vec(value)?;
    let mut stream = TcpStream::connect(host_port)
        .await
        .with_context(|| format!("failed to connect to API at {host_port}"))?;
    let token = internal_token.context("gateway API internal token is not configured")?;
    let auth_header = format!("Authorization: Bearer {token}\r\n");
    let request = format!(
        "POST {request_path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n{auth_header}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(request.as_bytes()).await?;
    stream.write_all(&body).await?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    let response = String::from_utf8_lossy(&response);
    let status = response
        .lines()
        .next()
        .ok_or_else(|| anyhow!("invalid API response"))?;
    let (_, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid API response missing HTTP body"))?;
    if !status.contains(" 2") {
        return Err(anyhow!("API returned {status}"));
    }
    Ok(body.trim().to_string())
}

#[derive(Debug, Serialize)]
struct GatewayIdentityValidationRequest {
    client_id: String,
    noise_public_key_hex: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GatewayIdentityValidationResponse {
    pub(crate) accepted: bool,
    pub(crate) message: String,
}
