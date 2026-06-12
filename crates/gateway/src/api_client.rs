use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, Mutex},
    time::{self, sleep, Duration},
};
use tracing::warn;
use vpsman_common::GatewayForwardMetricsSnapshot;

#[derive(Clone)]
pub(crate) struct GatewayControlClient {
    api_url: Option<String>,
    internal_token: Option<String>,
    forwarder: Arc<GatewayEventForwarder>,
}

impl GatewayControlClient {
    pub(crate) fn new(api_url: Option<String>, internal_token: Option<String>) -> Self {
        Self {
            api_url: api_url.map(|url| url.trim_end_matches('/').to_string()),
            internal_token: internal_token
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty()),
            forwarder: Arc::default(),
        }
    }

    pub(crate) fn forward_metrics(&self) -> Arc<GatewayForwardMetrics> {
        self.forwarder.metrics.clone()
    }

    pub(crate) async fn post<T: serde::Serialize>(&self, target_key: &str, path: &str, value: &T) {
        let Some(api_url) = &self.api_url else {
            return;
        };
        let Ok(body) = serde_json::to_vec(value) else {
            warn!(path, "failed to serialize gateway event for API forwarding");
            return;
        };
        self.forwarder
            .enqueue(
                target_key.to_string(),
                GatewayForwardEvent {
                    api_url: api_url.clone(),
                    path: path.to_string(),
                    body,
                    internal_token: self.internal_token.clone(),
                },
            )
            .await;
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

#[derive(Default)]
struct GatewayEventForwarder {
    queues: Mutex<HashMap<String, mpsc::UnboundedSender<GatewayForwardEvent>>>,
    metrics: Arc<GatewayForwardMetrics>,
}

#[derive(Default)]
pub(crate) struct GatewayForwardMetrics {
    queued_events: AtomicU64,
    delivered_events: AtomicU64,
    retry_attempts: AtomicU64,
    active_queues: AtomicU64,
}

#[derive(Debug)]
struct GatewayForwardEvent {
    api_url: String,
    path: String,
    body: Vec<u8>,
    internal_token: Option<String>,
}

impl GatewayEventForwarder {
    async fn enqueue(&self, target_key: String, event: GatewayForwardEvent) {
        let mut event = Some(event);
        let mut queues = self.queues.lock().await;
        if let Some(sender) = queues.get(&target_key) {
            match sender.send(event.take().expect("event exists")) {
                Ok(()) => {
                    self.metrics.queued_events.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                Err(error) => {
                    event = Some(error.0);
                }
            }
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        let metrics = self.metrics.clone();
        self.metrics.active_queues.fetch_add(1, Ordering::Relaxed);
        tokio::spawn(run_forward_queue(target_key.clone(), receiver, metrics));
        let _ = sender.send(event.expect("event exists after queue creation"));
        self.metrics.queued_events.fetch_add(1, Ordering::Relaxed);
        queues.insert(target_key, sender);
    }
}

impl GatewayForwardMetrics {
    pub(crate) fn snapshot(&self) -> GatewayForwardMetricsSnapshot {
        GatewayForwardMetricsSnapshot {
            queued_events: self.queued_events.load(Ordering::Relaxed),
            delivered_events: self.delivered_events.load(Ordering::Relaxed),
            retry_attempts: self.retry_attempts.load(Ordering::Relaxed),
            active_queues: self.active_queues.load(Ordering::Relaxed),
        }
    }
}

async fn run_forward_queue(
    target_key: String,
    mut receiver: mpsc::UnboundedReceiver<GatewayForwardEvent>,
    metrics: Arc<GatewayForwardMetrics>,
) {
    while let Some(event) = receiver.recv().await {
        post_json_retry_forever(event, &target_key, &metrics).await;
        metrics.delivered_events.fetch_add(1, Ordering::Relaxed);
    }
    metrics.active_queues.fetch_sub(1, Ordering::Relaxed);
}

async fn post_json_retry_forever(
    event: GatewayForwardEvent,
    target_key: &str,
    metrics: &GatewayForwardMetrics,
) {
    let mut attempt = 1_u64;
    loop {
        match post_json_bytes(
            &event.api_url,
            &event.path,
            &event.body,
            event.internal_token.as_deref(),
        )
        .await
        {
            Ok(_) => return,
            Err(error) => {
                metrics.retry_attempts.fetch_add(1, Ordering::Relaxed);
                warn!(
                    %error,
                    path = %event.path,
                    target_key,
                    attempt,
                    "failed to forward gateway event to API"
                );
                let backoff_ms =
                    250_u64.saturating_mul(2_u64.saturating_pow((attempt - 1).min(7) as u32));
                sleep(Duration::from_millis(backoff_ms.min(30_000))).await;
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

async fn post_json<T: serde::Serialize>(
    base_url: &str,
    path: &str,
    value: &T,
    internal_token: Option<&str>,
) -> Result<String> {
    let body = serde_json::to_vec(value)?;
    post_json_bytes(base_url, path, &body, internal_token).await
}

async fn post_json_bytes(
    base_url: &str,
    path: &str,
    body: &[u8],
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
    let mut stream = time::timeout(Duration::from_secs(10), TcpStream::connect(host_port))
        .await
        .context("API connect timed out")?
        .with_context(|| format!("failed to connect to API at {host_port}"))?;
    let token = internal_token.context("gateway API internal token is not configured")?;
    let auth_header = format!("Authorization: Bearer {token}\r\n");
    let request = format!(
        "POST {request_path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n{auth_header}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    time::timeout(
        Duration::from_secs(10),
        stream.write_all(request.as_bytes()),
    )
    .await
    .context("API request header write timed out")??;
    time::timeout(Duration::from_secs(10), stream.write_all(body))
        .await
        .context("API request body write timed out")??;

    let mut response = Vec::new();
    time::timeout(Duration::from_secs(15), stream.read_to_end(&mut response))
        .await
        .context("API response read timed out")??;
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
