use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex as StdMutex, RwLock as StdRwLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, Mutex, Notify},
    time::{self, sleep, Duration},
};
use tracing::warn;
use vpsman_common::{
    create_private_file_new_async, ensure_private_dir_async, open_private_file_read_async,
    payload_hash, repair_private_file_permissions_async, AgentUpdateVerificationResult,
    GatewayAgentHelloIngest, GatewayAgentUpdateVerificationIngest, GatewayCommandOutputIngest,
    GatewayForwardCriticalFailureCounters, GatewayForwardDropReasonCounters,
    GatewayForwardEventKindCounters, GatewayForwardMetricsSnapshot, GatewayTerminalOutputIngest,
    OutputStream,
};

type CriticalForwardingFailureHandler = Arc<dyn Fn(String, &'static str) + Send + Sync + 'static>;
const SPOOL_MAGIC: &[u8] = b"VPSMAN_GATEWAY_SPOOL_V2\n";
const SPOOL_SCHEMA_VERSION: u16 = 2;
const COMMAND_OUTPUT_PATH: &str = "/internal/v1/gateway/command-output";
const AGENT_UPDATE_VERIFICATION_PATH: &str = "/internal/v1/gateway/agent-update-verification";
const DEFAULT_SPOOL_RAM_MAX_BYTES: u64 = 1024 * 1024 * 1024;
const DEFAULT_SPOOL_DISK_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const DEFAULT_SPOOL_SHUTDOWN_FLUSH_SECS: u64 = 30;

#[derive(Clone)]
pub(crate) struct GatewayControlClient {
    api_url: Option<String>,
    internal_token: Option<String>,
    forwarder: Arc<GatewayEventForwarder>,
    timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
}

impl GatewayControlClient {
    #[cfg(test)]
    pub(crate) fn new(
        api_url: Option<String>,
        internal_token: Option<String>,
        timeouts: GatewayHttpTimeouts,
    ) -> Self {
        Self {
            api_url: api_url.map(|url| url.trim_end_matches('/').to_string()),
            internal_token: internal_token
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty()),
            forwarder: Arc::default(),
            timeouts: Arc::new(StdRwLock::new(timeouts)),
        }
    }

    pub(crate) fn new_with_spool(
        api_url: Option<String>,
        internal_token: Option<String>,
        timeouts: GatewayHttpTimeouts,
        spool_config: GatewaySpoolConfig,
        forward_config: GatewayForwardConfig,
    ) -> Self {
        let timeouts = Arc::new(StdRwLock::new(timeouts));
        let forwarder = Arc::new(GatewayEventForwarder::with_config(
            spool_config,
            forward_config,
        ));
        let client = Self {
            api_url: api_url.map(|url| url.trim_end_matches('/').to_string()),
            internal_token: internal_token
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty()),
            forwarder,
            timeouts,
        };
        client.forwarder.start_spool_replay(client.timeouts.clone());
        client
    }

    pub(crate) fn forward_metrics(&self) -> Arc<GatewayForwardMetrics> {
        self.forwarder.metrics.clone()
    }

    pub(crate) fn set_critical_failure_handler<F>(&self, handler: F)
    where
        F: Fn(String, &'static str) + Send + Sync + 'static,
    {
        if let Ok(mut slot) = self.forwarder.critical_failure_handler.write() {
            *slot = Some(Arc::new(handler));
        }
    }

    pub(crate) fn set_timeouts(&self, timeouts: GatewayHttpTimeouts) {
        if let Ok(mut current) = self.timeouts.write() {
            *current = timeouts;
        }
    }

    pub(crate) fn set_forward_config(&self, config: GatewayForwardConfig) {
        self.forwarder.set_runtime_config(config);
    }

    pub(crate) fn timeouts(&self) -> GatewayHttpTimeouts {
        current_gateway_http_timeouts(&self.timeouts)
    }

    pub(crate) async fn shutdown_flush(&self, timeout: Duration) {
        self.forwarder.shutdown_flush(timeout).await;
    }

    pub(crate) async fn post<T: serde::Serialize>(
        &self,
        target_key: &str,
        path: &str,
        value: &T,
    ) -> Result<()> {
        let Some(api_url) = &self.api_url else {
            anyhow::bail!("gateway API URL is required for event forwarding");
        };
        let Ok(body) = serde_json::to_vec(value) else {
            warn!(path, "failed to serialize gateway event for API forwarding");
            return Ok(());
        };
        let kind = GatewayForwardEventKind::for_path(path);
        let critical = gateway_event_critical(kind, &body);
        self.forwarder
            .enqueue(
                target_key.to_string(),
                GatewayForwardEvent {
                    api_url: api_url.clone(),
                    path: path.to_string(),
                    body,
                    internal_token: self.internal_token.clone(),
                    kind,
                    critical,
                    command_output: None,
                    created_at: time::Instant::now(),
                    created_unix: unix_now(),
                    enqueue_seq: self.forwarder.next_enqueue_seq(),
                },
                self.timeouts.clone(),
            )
            .await
    }

    pub(crate) async fn post_command_output(
        &self,
        target_key: &str,
        value: &GatewayCommandOutputIngest,
    ) -> Result<()> {
        let Some(api_url) = &self.api_url else {
            anyhow::bail!("gateway API URL is required for event forwarding");
        };
        let Ok(body) = serde_json::to_vec(value) else {
            warn!(
                path = COMMAND_OUTPUT_PATH,
                "failed to serialize gateway event for API forwarding"
            );
            return Ok(());
        };
        self.forwarder
            .enqueue(
                target_key.to_string(),
                GatewayForwardEvent {
                    api_url: api_url.clone(),
                    path: COMMAND_OUTPUT_PATH.to_string(),
                    body,
                    internal_token: self.internal_token.clone(),
                    kind: GatewayForwardEventKind::CommandOutput,
                    critical: true,
                    command_output: Some(CommandOutputReplayRef::from(value)),
                    created_at: time::Instant::now(),
                    created_unix: unix_now(),
                    enqueue_seq: self.forwarder.next_enqueue_seq(),
                },
                self.timeouts.clone(),
            )
            .await
    }

    pub(crate) async fn accept_agent_session(
        &self,
        value: &GatewayAgentHelloIngest,
    ) -> Result<GatewayIngestResponse> {
        let Some(api_url) = &self.api_url else {
            anyhow::bail!("gateway API URL is required for agent session acceptance");
        };
        let body = post_json(
            api_url,
            "/internal/v1/gateway/agent-hello",
            value,
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await?;
        serde_json::from_str(&body).context("failed to parse gateway ingest response")
    }

    pub(crate) async fn validate_agent_identity(
        &self,
        client_id: &str,
        noise_public_key_hex: &str,
    ) -> Result<GatewayIdentityValidationResponse> {
        let Some(api_url) = &self.api_url else {
            anyhow::bail!("enrolled IK identity validation requires VPSMAN_API_URL");
        };
        let body = post_json(
            api_url,
            "/internal/v1/gateway/agent-identity",
            &GatewayIdentityValidationRequest {
                client_id: client_id.to_string(),
                noise_public_key_hex: noise_public_key_hex.to_string(),
            },
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await?;
        serde_json::from_str(&body).context("failed to parse gateway identity validation response")
    }

    pub(crate) async fn verify_agent_update_artifact(
        &self,
        value: &GatewayAgentUpdateVerificationIngest,
    ) -> Result<AgentUpdateVerificationResult> {
        let Some(api_url) = &self.api_url else {
            anyhow::bail!("gateway API URL is required for agent update verification");
        };
        let body = post_json(
            api_url,
            AGENT_UPDATE_VERIFICATION_PATH,
            value,
            self.internal_token.as_deref(),
            self.timeouts(),
        )
        .await?;
        serde_json::from_str(&body).context("failed to parse agent update verification response")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GatewaySpoolConfig {
    pub(crate) dir: PathBuf,
    pub(crate) ram_max_bytes: u64,
    pub(crate) disk_max_bytes: u64,
    pub(crate) shutdown_flush: Duration,
    pub(crate) enabled: bool,
}

impl GatewaySpoolConfig {
    pub(crate) fn enabled(
        dir: PathBuf,
        ram_max_bytes: u64,
        disk_max_bytes: u64,
        shutdown_flush_secs: u64,
    ) -> Self {
        Self {
            dir,
            ram_max_bytes: ram_max_bytes.clamp(1024 * 1024, 16 * 1024 * 1024 * 1024),
            disk_max_bytes: disk_max_bytes.clamp(1024 * 1024, 1024 * 1024 * 1024 * 1024),
            shutdown_flush: Duration::from_secs(shutdown_flush_secs.clamp(1, 3600)),
            enabled: true,
        }
    }

    fn disabled() -> Self {
        Self {
            dir: PathBuf::new(),
            ram_max_bytes: DEFAULT_SPOOL_RAM_MAX_BYTES,
            disk_max_bytes: DEFAULT_SPOOL_DISK_MAX_BYTES,
            shutdown_flush: Duration::from_secs(DEFAULT_SPOOL_SHUTDOWN_FLUSH_SECS),
            enabled: false,
        }
    }
}

impl Default for GatewaySpoolConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

struct GatewayEventForwarder {
    queues: Mutex<HashMap<String, GatewayForwardQueue>>,
    telemetry_pending: Arc<Mutex<HashMap<String, GatewayForwardEvent>>>,
    critical_failure_handler: Arc<StdRwLock<Option<CriticalForwardingFailureHandler>>>,
    metrics: Arc<GatewayForwardMetrics>,
    spool: Arc<GatewayForwardSpool>,
    runtime_config: Arc<GatewayForwardRuntimeConfig>,
    enqueue_seq: AtomicU64,
}

struct GatewayForwardQueue {
    sender: mpsc::Sender<GatewayForwardQueueItem>,
    last_enqueue_unix: u64,
}

struct GatewayForwardSpool {
    config: GatewaySpoolConfig,
    ram_bytes: AtomicU64,
    disk_bytes: AtomicU64,
    accounted_spool_files: StdMutex<HashMap<PathBuf, u64>>,
    shutdown_requested: AtomicBool,
    shutdown_notify: Notify,
}

#[derive(Default)]
pub(crate) struct GatewayForwardMetrics {
    queued_events: AtomicU64,
    delivered_events: AtomicU64,
    retry_attempts: AtomicU64,
    active_queues: AtomicU64,
    current_queue_depth: AtomicU64,
    oldest_event_unix: AtomicU64,
    dropped_events: AtomicU64,
    telemetry_dropped_events: AtomicU64,
    expired_events: AtomicU64,
    critical_failures: AtomicU64,
    dropped_by_kind: GatewayForwardKindAtomicCounters,
    dropped_by_reason: GatewayForwardDropReasonAtomicCounters,
    critical_failures_by_reason: GatewayForwardCriticalFailureAtomicCounters,
    retained_output_truncated_events: AtomicU64,
    rejected_agent_connections: AtomicU64,
    unhealthy: AtomicBool,
}

#[derive(Default)]
struct GatewayForwardKindAtomicCounters {
    telemetry: AtomicU64,
    command_output: AtomicU64,
    lifecycle: AtomicU64,
    terminal_output: AtomicU64,
    other: AtomicU64,
}

#[derive(Default)]
struct GatewayForwardDropReasonAtomicCounters {
    global_queue_full: AtomicU64,
    target_queue_full: AtomicU64,
    expired: AtomicU64,
    coalesced: AtomicU64,
    protocol_conflict: AtomicU64,
}

#[derive(Default)]
struct GatewayForwardCriticalFailureAtomicCounters {
    global_queue_full: AtomicU64,
    target_queue_full: AtomicU64,
    expired: AtomicU64,
}

#[derive(Debug)]
struct GatewayForwardEvent {
    api_url: String,
    path: String,
    body: Vec<u8>,
    internal_token: Option<String>,
    kind: GatewayForwardEventKind,
    critical: bool,
    command_output: Option<CommandOutputReplayRef>,
    created_at: time::Instant,
    created_unix: u64,
    enqueue_seq: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum GatewayForwardEventKind {
    Telemetry,
    CommandOutput,
    Lifecycle,
    TerminalOutput,
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct CommandOutputReplayRef {
    client_id: String,
    job_id: uuid::Uuid,
    seq: i32,
}

impl From<&GatewayCommandOutputIngest> for CommandOutputReplayRef {
    fn from(event: &GatewayCommandOutputIngest) -> Self {
        Self {
            client_id: event.client_id.clone(),
            job_id: event.job_id,
            seq: event.seq,
        }
    }
}

#[derive(Debug)]
enum GatewayForwardQueueItem {
    Event {
        event: GatewayForwardEvent,
        ram_bytes: u64,
    },
    Spooled {
        path: PathBuf,
        created_unix: u64,
        enqueue_seq: u64,
        disk_bytes: u64,
        kind: GatewayForwardEventKind,
        critical: bool,
    },
    Telemetry {
        created_unix: u64,
    },
}

struct GatewayPendingSpoolCandidate {
    target_key: String,
    path: PathBuf,
    created_unix: u64,
    enqueue_seq: u64,
    disk_bytes: u64,
    kind: GatewayForwardEventKind,
    critical: bool,
}

struct GatewayForwardEventHandle {
    event: GatewayForwardEvent,
    ram_bytes: u64,
    spool_path: Option<PathBuf>,
    spool_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GatewayForwardOutcome {
    Delivered,
    NotDelivered,
    DeferredToSpool,
    DeferredForShutdown,
}

#[derive(Debug, Deserialize, Serialize)]
struct SpooledGatewayForwardHeader {
    schema_version: u16,
    api_url: String,
    path: String,
    internal_token: Option<String>,
    kind: GatewayForwardEventKind,
    critical: bool,
    created_unix: u64,
    #[serde(default)]
    enqueue_seq: u64,
    body_sha256_hex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command_output: Option<CommandOutputReplayRef>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GatewayForwardDropReason {
    GlobalQueueFull,
    TargetQueueFull,
    Expired,
    Coalesced,
    ProtocolConflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GatewayHttpTimeouts {
    pub(crate) connect: Duration,
    pub(crate) write: Duration,
    pub(crate) read: Duration,
    pub(crate) event_post: Duration,
}

impl Default for GatewayHttpTimeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            write: Duration::from_secs(10),
            read: Duration::from_secs(15),
            event_post: Duration::from_secs(15),
        }
    }
}

fn current_gateway_http_timeouts(timeouts: &StdRwLock<GatewayHttpTimeouts>) -> GatewayHttpTimeouts {
    timeouts
        .read()
        .map(|timeouts| *timeouts)
        .unwrap_or_default()
}

fn initial_gateway_enqueue_seq() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or_else(|_| unix_now())
}

fn initial_gateway_enqueue_seq_for_spool(spool: &GatewayForwardSpool) -> u64 {
    let mut enqueue_seq = initial_gateway_enqueue_seq();
    if !spool.config.enabled {
        return enqueue_seq;
    }
    let pending_dir = spool.pending_dir();
    let Ok(entries) = fs::read_dir(&pending_dir) else {
        return enqueue_seq;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("spool") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(event) = decode_spooled_event(&path, &bytes) else {
            continue;
        };
        enqueue_seq = enqueue_seq.max(event.enqueue_seq);
    }
    enqueue_seq
}

const PER_TARGET_QUEUE_CAPACITY: usize = 512;
const GLOBAL_QUEUE_CAPACITY: u64 = 10_000;
const QUEUE_IDLE_REAP_SECS: u64 = 600;
const TELEMETRY_EVENT_TTL: Duration = Duration::from_secs(60);
const CRITICAL_EVENT_TTL: Duration = Duration::from_secs(300);
pub(crate) const DEFAULT_COMMAND_OUTPUT_EVENT_TTL_SECS: u64 = 24 * 60 * 60;
const NONCRITICAL_EVENT_TTL: Duration = Duration::from_secs(120);
const MIN_COMMAND_OUTPUT_EVENT_TTL_SECS: u64 = 300;
const MAX_COMMAND_OUTPUT_EVENT_TTL_SECS: u64 = 30 * 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GatewayForwardConfig {
    pub(crate) command_output_event_ttl_secs: u64,
}

impl GatewayForwardConfig {
    pub(crate) fn new(command_output_event_ttl_secs: u64) -> Self {
        Self {
            command_output_event_ttl_secs: command_output_event_ttl_secs.clamp(
                MIN_COMMAND_OUTPUT_EVENT_TTL_SECS,
                MAX_COMMAND_OUTPUT_EVENT_TTL_SECS,
            ),
        }
    }
}

impl Default for GatewayForwardConfig {
    fn default() -> Self {
        Self::new(DEFAULT_COMMAND_OUTPUT_EVENT_TTL_SECS)
    }
}

#[derive(Default)]
struct GatewayForwardRuntimeConfig {
    command_output_event_ttl_secs: AtomicU64,
}

impl GatewayForwardRuntimeConfig {
    fn new(config: GatewayForwardConfig) -> Self {
        Self {
            command_output_event_ttl_secs: AtomicU64::new(config.command_output_event_ttl_secs),
        }
    }

    fn set(&self, config: GatewayForwardConfig) {
        self.command_output_event_ttl_secs
            .store(config.command_output_event_ttl_secs, Ordering::Relaxed);
    }

    fn command_output_event_ttl(&self) -> Duration {
        Duration::from_secs(
            self.command_output_event_ttl_secs
                .load(Ordering::Relaxed)
                .clamp(
                    MIN_COMMAND_OUTPUT_EVENT_TTL_SECS,
                    MAX_COMMAND_OUTPUT_EVENT_TTL_SECS,
                ),
        )
    }
}

impl Default for GatewayEventForwarder {
    fn default() -> Self {
        Self::with_config(
            GatewaySpoolConfig::disabled(),
            GatewayForwardConfig::default(),
        )
    }
}

impl GatewayEventForwarder {
    #[cfg(test)]
    fn with_spool_config(spool_config: GatewaySpoolConfig) -> Self {
        Self::with_config(spool_config, GatewayForwardConfig::default())
    }

    fn with_config(spool_config: GatewaySpoolConfig, forward_config: GatewayForwardConfig) -> Self {
        let spool = Arc::new(GatewayForwardSpool::new(spool_config));
        let enqueue_seq = initial_gateway_enqueue_seq_for_spool(&spool);
        Self {
            queues: Mutex::default(),
            telemetry_pending: Arc::default(),
            critical_failure_handler: Arc::default(),
            metrics: Arc::default(),
            spool,
            runtime_config: Arc::new(GatewayForwardRuntimeConfig::new(forward_config)),
            enqueue_seq: AtomicU64::new(enqueue_seq),
        }
    }

    fn set_runtime_config(&self, config: GatewayForwardConfig) {
        self.runtime_config.set(config);
    }

    fn next_enqueue_seq(&self) -> u64 {
        self.enqueue_seq
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }

    fn start_spool_replay(self: &Arc<Self>, timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>) {
        if !self.spool.config.enabled {
            return;
        }
        let forwarder = self.clone();
        tokio::spawn(async move {
            loop {
                let saw_pending = forwarder.replay_pending_spool_once(timeouts.clone()).await;
                if forwarder.spool.shutdown_requested() {
                    break;
                }
                while forwarder
                    .metrics
                    .current_queue_depth
                    .load(Ordering::Relaxed)
                    > 0
                    && !forwarder.spool.shutdown_requested()
                {
                    sleep(Duration::from_millis(250)).await;
                }
                if !saw_pending {
                    sleep(Duration::from_secs(1)).await;
                }
            }
        });
    }

    async fn replay_pending_spool_once(
        &self,
        timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
    ) -> bool {
        let items = self.spool.pending_items().await;
        let saw_pending = !items.is_empty();
        for (target_key, item) in items {
            if let Err(error) = self
                .enqueue_queue_item(target_key.clone(), item, timeouts.clone())
                .await
            {
                warn!(
                    %error,
                    target_key,
                    "failed to enqueue spooled gateway event for replay"
                );
            }
        }
        saw_pending
    }

    async fn shutdown_flush(&self, timeout: Duration) {
        self.spool.request_shutdown();
        let deadline = time::Instant::now() + timeout;
        while self.metrics.current_queue_depth.load(Ordering::Relaxed) > 0
            && time::Instant::now() < deadline
        {
            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn enqueue(
        &self,
        target_key: String,
        event: GatewayForwardEvent,
        timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
    ) -> Result<()> {
        if event.kind == GatewayForwardEventKind::Telemetry {
            return self.enqueue_telemetry(target_key, event, timeouts).await;
        }
        self.enqueue_event(target_key, event, timeouts).await
    }

    async fn enqueue_telemetry(
        &self,
        target_key: String,
        event: GatewayForwardEvent,
        timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
    ) -> Result<()> {
        if self.metrics.current_queue_depth.load(Ordering::Relaxed) >= GLOBAL_QUEUE_CAPACITY {
            return self
                .drop_enqueue_event(
                    &target_key,
                    event,
                    GatewayForwardDropReason::GlobalQueueFull,
                )
                .await;
        }

        let mut pending = self.telemetry_pending.lock().await;
        let created_unix = event.created_unix;
        if let Some(previous) = pending.insert(target_key.clone(), event) {
            drop(pending);
            self.record_drop(&previous, GatewayForwardDropReason::Coalesced);
            warn!(
                path = %previous.path,
                kind = ?previous.kind,
                target_key,
                "coalesced stale gateway telemetry before API forwarding"
            );
            return Ok(());
        }
        drop(pending);

        if let Err(error) = self
            .enqueue_queue_item(
                target_key.clone(),
                GatewayForwardQueueItem::Telemetry { created_unix },
                timeouts,
            )
            .await
        {
            let removed = self.telemetry_pending.lock().await.remove(&target_key);
            if let Some(event) = removed {
                return self
                    .drop_enqueue_event(
                        &target_key,
                        event,
                        GatewayForwardDropReason::TargetQueueFull,
                    )
                    .await;
            }
            return Err(error);
        }
        Ok(())
    }

    async fn enqueue_event(
        &self,
        target_key: String,
        event: GatewayForwardEvent,
        timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
    ) -> Result<()> {
        if event_spools_under_pressure(&event) && self.spool.target_has_pending(&target_key).await {
            return match self
                .spool_event_for_later_replay(
                    &target_key,
                    &event,
                    GatewayForwardDropReason::TargetQueueFull,
                )
                .await
            {
                Ok(()) => Ok(()),
                Err(error) => {
                    warn!(
                        %error,
                        path = %event.path,
                        kind = ?event.kind,
                        target_key,
                        "failed to spool critical gateway output behind pending replay fence"
                    );
                    self.drop_enqueue_event(
                        &target_key,
                        event,
                        GatewayForwardDropReason::TargetQueueFull,
                    )
                    .await
                }
            };
        }
        if self.metrics.current_queue_depth.load(Ordering::Relaxed) >= GLOBAL_QUEUE_CAPACITY {
            if event_spools_under_pressure(&event) {
                return match self
                    .spool_event_for_later_replay(
                        &target_key,
                        &event,
                        GatewayForwardDropReason::GlobalQueueFull,
                    )
                    .await
                {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        warn!(
                            %error,
                            path = %event.path,
                            kind = ?event.kind,
                            target_key,
                            "failed to spool critical gateway output under global queue pressure"
                        );
                        self.drop_enqueue_event(
                            &target_key,
                            event,
                            GatewayForwardDropReason::GlobalQueueFull,
                        )
                        .await
                    }
                };
            }
            return self
                .drop_enqueue_event(
                    &target_key,
                    event,
                    GatewayForwardDropReason::GlobalQueueFull,
                )
                .await;
        }
        let item = match self.prepare_queue_item(&target_key, event).await {
            Ok(item) => item,
            Err((event, error)) => {
                warn!(
                    %error,
                    path = %event.path,
                    kind = ?event.kind,
                    target_key,
                    "failed to spool gateway event before API forwarding"
                );
                return self
                    .drop_enqueue_event(
                        &target_key,
                        event,
                        GatewayForwardDropReason::GlobalQueueFull,
                    )
                    .await;
            }
        };
        self.enqueue_queue_item(target_key, item, timeouts).await
    }

    async fn prepare_queue_item(
        &self,
        target_key: &str,
        event: GatewayForwardEvent,
    ) -> std::result::Result<GatewayForwardQueueItem, (GatewayForwardEvent, anyhow::Error)> {
        let ram_bytes = event.body.len() as u64;
        if matches!(
            event.kind,
            GatewayForwardEventKind::CommandOutput | GatewayForwardEventKind::TerminalOutput
        ) && !self.spool.try_reserve_ram(ram_bytes)
        {
            return match self.spool.spool_event(target_key, &event).await {
                Ok(item) => Ok(item),
                Err(error) => Err((event, error)),
            };
        }
        let ram_bytes = if self.spool.config.enabled {
            if matches!(
                event.kind,
                GatewayForwardEventKind::CommandOutput | GatewayForwardEventKind::TerminalOutput
            ) {
                ram_bytes
            } else {
                self.spool.reserve_ram_unchecked(ram_bytes);
                ram_bytes
            }
        } else {
            0
        };
        Ok(GatewayForwardQueueItem::Event { event, ram_bytes })
    }

    async fn enqueue_queue_item(
        &self,
        target_key: String,
        item: GatewayForwardQueueItem,
        timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
    ) -> Result<()> {
        let event_unix = item.created_unix();
        let event_enqueue_seq = item.enqueue_seq();
        let sender = {
            let mut queues = self.queues.lock().await;
            self.reap_idle_queues_locked(&mut queues, unix_now());
            if !queues.contains_key(&target_key) {
                let (sender, receiver) = mpsc::channel(PER_TARGET_QUEUE_CAPACITY);
                let metrics = self.metrics.clone();
                let telemetry_pending = self.telemetry_pending.clone();
                let critical_failure_handler = self.critical_failure_handler.clone();
                let spool = self.spool.clone();
                let runtime_config = self.runtime_config.clone();
                self.metrics.active_queues.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(run_forward_queue(
                    target_key.clone(),
                    receiver,
                    telemetry_pending,
                    metrics,
                    critical_failure_handler,
                    spool,
                    runtime_config,
                    timeouts,
                ));
                queues.insert(
                    target_key.clone(),
                    GatewayForwardQueue {
                        sender,
                        last_enqueue_unix: event_unix,
                    },
                );
            }
            let queue = queues
                .get_mut(&target_key)
                .expect("queue sender exists after creation");
            queue.last_enqueue_unix = event_unix;
            queue.sender.clone()
        };
        let previous_depth = self
            .metrics
            .current_queue_depth
            .fetch_add(1, Ordering::Relaxed);
        if previous_depth == 0 {
            self.metrics
                .oldest_event_unix
                .store(event_unix, Ordering::Relaxed);
        }
        match sender.try_send(item) {
            Ok(()) => {
                self.metrics.queued_events.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(item))
            | Err(mpsc::error::TrySendError::Closed(item)) => {
                let previous = self
                    .metrics
                    .current_queue_depth
                    .fetch_sub(1, Ordering::Relaxed);
                if previous <= 1 {
                    self.metrics.oldest_event_unix.store(0, Ordering::Relaxed);
                }
                match item {
                    GatewayForwardQueueItem::Event { event, ram_bytes } => {
                        self.spool.release_ram(ram_bytes);
                        if event_spools_under_pressure(&event) {
                            return match self
                                .spool_event_for_later_replay(
                                    &target_key,
                                    &event,
                                    GatewayForwardDropReason::TargetQueueFull,
                                )
                                .await
                            {
                                Ok(()) => Ok(()),
                                Err(error) => {
                                    warn!(
                                        %error,
                                        path = %event.path,
                                        kind = ?event.kind,
                                        target_key,
                                        "failed to spool critical gateway output under target queue pressure"
                                    );
                                    self.drop_enqueue_event(
                                        &target_key,
                                        event,
                                        GatewayForwardDropReason::TargetQueueFull,
                                    )
                                    .await
                                }
                            };
                        }
                        self.drop_enqueue_event(
                            &target_key,
                            event,
                            GatewayForwardDropReason::TargetQueueFull,
                        )
                        .await
                    }
                    GatewayForwardQueueItem::Spooled {
                        path,
                        kind,
                        critical,
                        ..
                    } => {
                        self.metrics
                            .record_drop(kind, GatewayForwardDropReason::TargetQueueFull);
                        if critical {
                            self.record_critical_failure(GatewayForwardDropReason::TargetQueueFull);
                            self.notify_critical_failure(
                                &target_key,
                                GatewayForwardDropReason::TargetQueueFull,
                            );
                        }
                        warn!(
                            path = %path.display(),
                            kind = ?kind,
                            target_key,
                            enqueue_seq = event_enqueue_seq,
                            "target queue full while replaying spooled gateway event; preserving spool file for later replay"
                        );
                        anyhow::bail!("gateway_forwarder_event_replay_deferred:target_queue_full")
                    }
                    GatewayForwardQueueItem::Telemetry { .. } => {
                        Err(anyhow!("gateway_forwarder_target_queue_full"))
                    }
                }
            }
        }
    }

    async fn spool_event_for_later_replay(
        &self,
        target_key: &str,
        event: &GatewayForwardEvent,
        reason: GatewayForwardDropReason,
    ) -> Result<()> {
        spool_event_for_later_replay(&self.spool, target_key, event, reason).await
    }

    fn reap_idle_queues_locked(
        &self,
        queues: &mut HashMap<String, GatewayForwardQueue>,
        now_unix: u64,
    ) {
        queues.retain(|_, queue| {
            let idle = now_unix.saturating_sub(queue.last_enqueue_unix) >= QUEUE_IDLE_REAP_SECS;
            let empty = queue.sender.capacity() == queue.sender.max_capacity();
            !(idle && empty)
        });
    }

    async fn drop_enqueue_event(
        &self,
        target_key: &str,
        event: GatewayForwardEvent,
        reason: GatewayForwardDropReason,
    ) -> Result<()> {
        self.record_drop(&event, reason);
        if event.critical {
            self.record_critical_failure(reason);
            self.notify_critical_failure(target_key, reason);
            anyhow::bail!(
                "gateway_forwarder_critical_event_dropped:{}:{}",
                reason.as_str(),
                event.path
            );
        }
        warn!(
            path = %event.path,
            kind = ?event.kind,
            reason = reason.as_str(),
            "dropped gateway event before API forwarding"
        );
        Ok(())
    }

    fn record_drop(&self, event: &GatewayForwardEvent, reason: GatewayForwardDropReason) {
        self.metrics.record_drop(event.kind, reason);
    }

    fn record_critical_failure(&self, reason: GatewayForwardDropReason) {
        self.metrics.record_critical_failure(reason);
    }

    fn notify_critical_failure(&self, target_key: &str, reason: GatewayForwardDropReason) {
        if let Ok(slot) = self.critical_failure_handler.read() {
            if let Some(handler) = slot.as_ref() {
                handler(target_key.to_string(), reason.as_str());
            }
        }
    }
}

impl GatewayForwardSpool {
    fn new(config: GatewaySpoolConfig) -> Self {
        Self {
            config,
            ram_bytes: AtomicU64::new(0),
            disk_bytes: AtomicU64::new(0),
            accounted_spool_files: StdMutex::new(HashMap::new()),
            shutdown_requested: AtomicBool::new(false),
            shutdown_notify: Notify::new(),
        }
    }

    fn request_shutdown(&self) {
        self.shutdown_requested.store(true, Ordering::Relaxed);
        self.shutdown_notify.notify_waiters();
    }

    fn shutdown_requested(&self) -> bool {
        self.shutdown_requested.load(Ordering::Relaxed)
    }

    async fn notified_shutdown(&self) {
        if self.shutdown_requested() {
            return;
        }
        self.shutdown_notify.notified().await;
    }

    fn try_reserve_ram(&self, bytes: u64) -> bool {
        if !self.config.enabled {
            return true;
        }
        let bytes = bytes.max(1);
        let mut current = self.ram_bytes.load(Ordering::Relaxed);
        loop {
            let Some(next) = current.checked_add(bytes) else {
                return false;
            };
            if next > self.config.ram_max_bytes {
                return false;
            }
            match self.ram_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }

    fn reserve_ram_unchecked(&self, bytes: u64) {
        if self.config.enabled && bytes > 0 {
            self.ram_bytes.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    fn release_ram(&self, bytes: u64) {
        if self.config.enabled && bytes > 0 {
            self.ram_bytes.fetch_sub(bytes, Ordering::Relaxed);
        }
    }

    async fn spool_event(
        &self,
        target_key: &str,
        event: &GatewayForwardEvent,
    ) -> Result<GatewayForwardQueueItem> {
        anyhow::ensure!(self.config.enabled, "gateway spool is disabled");
        let pending_dir = self.pending_dir();
        ensure_private_dir_async(&self.config.dir)
            .await
            .with_context(|| {
                format!(
                    "failed to create gateway spool root {}",
                    self.config.dir.display()
                )
            })?;
        ensure_private_dir_async(&pending_dir)
            .await
            .with_context(|| {
                format!(
                    "failed to create gateway spool dir {}",
                    pending_dir.display()
                )
            })?;
        let header = SpooledGatewayForwardHeader {
            schema_version: SPOOL_SCHEMA_VERSION,
            api_url: event.api_url.clone(),
            path: event.path.clone(),
            internal_token: event.internal_token.clone(),
            kind: event.kind,
            critical: event.critical,
            created_unix: event.created_unix,
            enqueue_seq: event.enqueue_seq,
            body_sha256_hex: payload_hash(&event.body),
            command_output: event
                .command_output
                .clone()
                .or_else(|| command_output_replay_ref_from_body(&event.body)),
        };
        let header =
            serde_json::to_vec(&header).context("failed to encode gateway spool header")?;
        let mut bytes =
            Vec::with_capacity(SPOOL_MAGIC.len() + 24 + header.len() + event.body.len());
        bytes.extend_from_slice(SPOOL_MAGIC);
        bytes.extend_from_slice(header.len().to_string().as_bytes());
        bytes.push(b'\n');
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&event.body);
        let disk_bytes = bytes.len() as u64;
        self.try_reserve_disk(disk_bytes)?;

        let uuid = uuid::Uuid::new_v4();
        let target_hex = hex::encode(target_key.as_bytes());
        let final_path =
            pending_dir.join(format!("{}-{target_hex}-{uuid}.spool", event.created_unix));
        let temp_path = pending_dir.join(format!(".{uuid}.tmp"));
        let mut temp_file = match create_private_file_new_async(&temp_path).await {
            Ok(file) => file,
            Err(error) => {
                self.release_disk(disk_bytes);
                return Err(error).with_context(|| {
                    format!(
                        "failed to create gateway spool temp {}",
                        temp_path.display()
                    )
                });
            }
        };
        if let Err(error) = temp_file.write_all(&bytes).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            self.release_disk(disk_bytes);
            return Err(error).with_context(|| {
                format!("failed to write gateway spool temp {}", temp_path.display())
            });
        }
        if let Err(error) = temp_file.sync_all().await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            self.release_disk(disk_bytes);
            return Err(error).with_context(|| {
                format!("failed to fsync gateway spool temp {}", temp_path.display())
            });
        }
        drop(temp_file);
        if let Err(error) = tokio::fs::rename(&temp_path, &final_path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            self.release_disk(disk_bytes);
            return Err(error).with_context(|| {
                format!(
                    "failed to promote gateway spool file {}",
                    final_path.display()
                )
            });
        }
        fsync_dir_best_effort(&pending_dir, "gateway spool pending dir").await;
        if let Err(error) = self.account_spooled_file_after_reserve(&final_path, disk_bytes) {
            let _ = tokio::fs::remove_file(&final_path).await;
            self.release_disk(disk_bytes);
            return Err(error).with_context(|| {
                format!(
                    "failed to account gateway spool file {}",
                    final_path.display()
                )
            });
        }
        Ok(GatewayForwardQueueItem::Spooled {
            path: final_path,
            created_unix: event.created_unix,
            enqueue_seq: event.enqueue_seq,
            disk_bytes,
            kind: event.kind,
            critical: event.critical,
        })
    }

    async fn pending_items(&self) -> Vec<(String, GatewayForwardQueueItem)> {
        let mut candidates = Vec::new();
        let pending_dir = self.pending_dir();
        let Ok(mut entries) = tokio::fs::read_dir(&pending_dir).await else {
            return Vec::new();
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("spool") {
                continue;
            }
            let Ok(metadata) = tokio::fs::symlink_metadata(&path).await else {
                continue;
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                warn!(
                    path = %path.display(),
                    "removing unsafe gateway spool entry that is not a regular file"
                );
                let _ = tokio::fs::remove_file(&path).await;
                continue;
            }
            let Some((created_unix, target_key)) = parse_spool_filename(&path) else {
                warn!(path = %path.display(), "ignoring malformed gateway spool filename");
                continue;
            };
            let disk_bytes = metadata.len();
            let event = match self.load_spooled_event(&path).await {
                Ok(event) => event,
                Err(error) => {
                    warn!(
                        %error,
                        path = %path.display(),
                        "quarantining corrupt gateway spool file"
                    );
                    self.quarantine_spooled_file(&path).await;
                    continue;
                }
            };
            candidates.push(GatewayPendingSpoolCandidate {
                target_key,
                path,
                created_unix,
                enqueue_seq: event.enqueue_seq,
                disk_bytes,
                kind: event.kind,
                critical: event.critical,
            });
        }
        candidates.sort_by(|left, right| {
            (
                left.enqueue_seq,
                left.created_unix,
                left.target_key.as_str(),
                left.path.as_os_str(),
            )
                .cmp(&(
                    right.enqueue_seq,
                    right.created_unix,
                    right.target_key.as_str(),
                    right.path.as_os_str(),
                ))
        });
        let mut items = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            if let Err(error) =
                self.account_existing_spooled_file(&candidate.path, candidate.disk_bytes)
            {
                warn!(
                    %error,
                    path = %candidate.path.display(),
                    disk_bytes = candidate.disk_bytes,
                    "ignoring gateway spool file because disk accounting failed"
                );
                continue;
            }
            items.push((
                candidate.target_key,
                GatewayForwardQueueItem::Spooled {
                    path: candidate.path,
                    created_unix: candidate.created_unix,
                    enqueue_seq: candidate.enqueue_seq,
                    disk_bytes: candidate.disk_bytes,
                    kind: candidate.kind,
                    critical: candidate.critical,
                },
            ));
        }
        items
    }

    async fn load_spooled_event(&self, path: &Path) -> Result<GatewayForwardEvent> {
        let mut file = open_private_file_read_async(path)
            .await
            .with_context(|| format!("failed to open gateway spool file {}", path.display()))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .await
            .with_context(|| format!("failed to read gateway spool file {}", path.display()))?;
        decode_spooled_event(path, &bytes)
    }

    #[cfg(test)]
    async fn load_spooled_header(&self, path: &Path) -> Result<SpooledGatewayForwardHeader> {
        let mut file = open_private_file_read_async(path)
            .await
            .with_context(|| format!("failed to open gateway spool file {}", path.display()))?;
        let mut magic = vec![0_u8; SPOOL_MAGIC.len()];
        file.read_exact(&mut magic)
            .await
            .with_context(|| format!("failed to read gateway spool magic {}", path.display()))?;
        anyhow::ensure!(
            magic.as_slice() == SPOOL_MAGIC,
            "gateway spool file {} has invalid magic",
            path.display()
        );
        let mut header_len = Vec::with_capacity(24);
        loop {
            let mut byte = [0_u8; 1];
            file.read_exact(&mut byte).await.with_context(|| {
                format!(
                    "failed to read gateway spool header length {}",
                    path.display()
                )
            })?;
            if byte[0] == b'\n' {
                break;
            }
            anyhow::ensure!(
                header_len.len() < 32,
                "gateway spool file {} has oversized header length",
                path.display()
            );
            header_len.push(byte[0]);
        }
        let header_len = std::str::from_utf8(&header_len)
            .with_context(|| {
                format!(
                    "gateway spool file {} has invalid header length",
                    path.display()
                )
            })?
            .parse::<usize>()
            .with_context(|| {
                format!(
                    "gateway spool file {} has invalid header length",
                    path.display()
                )
            })?;
        let mut header = vec![0_u8; header_len];
        file.read_exact(&mut header)
            .await
            .with_context(|| format!("failed to read gateway spool header {}", path.display()))?;
        let header: SpooledGatewayForwardHeader = serde_json::from_slice(&header)
            .with_context(|| format!("failed to decode gateway spool header {}", path.display()))?;
        validate_spooled_header(path, &header)?;
        Ok(header)
    }

    async fn target_has_pending(&self, target_key: &str) -> bool {
        if !self.config.enabled {
            return false;
        }
        let pending_dir = self.pending_dir();
        let Ok(mut entries) = tokio::fs::read_dir(&pending_dir).await else {
            return false;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("spool") {
                continue;
            }
            let Some((_, pending_target_key)) = parse_spool_filename(&path) else {
                continue;
            };
            if pending_target_key == target_key {
                return true;
            }
        }
        false
    }

    async fn remove_spooled_file(&self, path: &Path, disk_bytes: u64) {
        match tokio::fs::remove_file(path).await {
            Ok(()) => self.unaccount_spooled_file(path, disk_bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.unaccount_spooled_file(path, disk_bytes);
            }
            Err(error) => {
                warn!(
                    %error,
                    path = %path.display(),
                    "failed to remove delivered gateway spool file"
                );
            }
        }
    }

    fn pending_dir(&self) -> PathBuf {
        self.config.dir.join("pending")
    }

    async fn quarantine_spooled_file(&self, path: &Path) {
        let quarantine_dir = self.config.dir.join("corrupt");
        if let Err(error) = ensure_private_dir_async(&self.config.dir).await {
            warn!(
                %error,
                path = %path.display(),
                "failed to create gateway spool root for quarantine"
            );
            return;
        }
        if let Err(error) = ensure_private_dir_async(&quarantine_dir).await {
            warn!(
                %error,
                path = %path.display(),
                "failed to create gateway spool quarantine dir"
            );
            return;
        }
        let Some(file_name) = path.file_name() else {
            return;
        };
        let quarantine_path = quarantine_dir.join(file_name);
        if let Err(error) = tokio::fs::rename(path, &quarantine_path).await {
            warn!(
                %error,
                path = %path.display(),
                quarantine_path = %quarantine_path.display(),
                "failed to quarantine corrupt gateway spool file"
            );
            return;
        }
        self.unaccount_spooled_file(path, 0);
        if let Err(error) = repair_private_file_permissions_async(&quarantine_path).await {
            warn!(
                %error,
                path = %quarantine_path.display(),
                "failed to repair gateway spool quarantine file permissions"
            );
        }
        fsync_dir_best_effort(&quarantine_dir, "gateway spool corrupt dir").await;
        if let Some(parent) = path.parent() {
            fsync_dir_best_effort(parent, "gateway spool pending dir").await;
        }
    }

    fn account_spooled_file_after_reserve(&self, path: &Path, disk_bytes: u64) -> Result<()> {
        let disk_bytes = disk_bytes.max(1);
        let mut accounted = self
            .accounted_spool_files
            .lock()
            .map_err(|_| anyhow!("gateway spool accounting lock poisoned"))?;
        if let Some(previous_bytes) = accounted.insert(path.to_path_buf(), disk_bytes) {
            self.release_disk(previous_bytes);
            warn!(
                path = %path.display(),
                previous_bytes,
                disk_bytes,
                "replaced existing gateway spool disk accounting entry"
            );
        }
        Ok(())
    }

    fn account_existing_spooled_file(&self, path: &Path, disk_bytes: u64) -> Result<()> {
        let disk_bytes = disk_bytes.max(1);
        let mut accounted = self
            .accounted_spool_files
            .lock()
            .map_err(|_| anyhow!("gateway spool accounting lock poisoned"))?;
        match accounted.get(path).copied() {
            Some(current_bytes) if current_bytes == disk_bytes => Ok(()),
            Some(current_bytes) => {
                if disk_bytes > current_bytes {
                    self.add_disk_accounting(disk_bytes - current_bytes)?;
                } else {
                    self.release_disk(current_bytes - disk_bytes);
                }
                accounted.insert(path.to_path_buf(), disk_bytes);
                warn!(
                    path = %path.display(),
                    previous_bytes = current_bytes,
                    disk_bytes,
                    "adjusted gateway spool disk accounting for changed pending file size"
                );
                Ok(())
            }
            None => {
                self.add_disk_accounting(disk_bytes)?;
                accounted.insert(path.to_path_buf(), disk_bytes);
                let accounted_bytes = self.disk_bytes.load(Ordering::Relaxed);
                if accounted_bytes > self.config.disk_max_bytes {
                    warn!(
                        path = %path.display(),
                        accounted_bytes,
                        disk_max_bytes = self.config.disk_max_bytes,
                        "existing gateway spool files exceed configured disk cap"
                    );
                }
                Ok(())
            }
        }
    }

    fn unaccount_spooled_file(&self, path: &Path, fallback_bytes: u64) {
        let accounted_bytes = match self.accounted_spool_files.lock() {
            Ok(mut accounted) => accounted.remove(path),
            Err(_) => {
                warn!(
                    path = %path.display(),
                    fallback_bytes,
                    "failed to unaccount gateway spool file because accounting lock is poisoned"
                );
                None
            }
        };
        if let Some(accounted_bytes) = accounted_bytes {
            self.release_disk(accounted_bytes);
        } else if fallback_bytes > 0 {
            warn!(
                path = %path.display(),
                fallback_bytes,
                "gateway spool file cleanup found no disk accounting entry"
            );
        }
    }

    fn add_disk_accounting(&self, bytes: u64) -> Result<()> {
        if bytes == 0 {
            return Ok(());
        }
        let mut current = self.disk_bytes.load(Ordering::Relaxed);
        loop {
            let next = current
                .checked_add(bytes)
                .context("gateway spool disk byte counter overflow")?;
            match self.disk_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
    }

    fn try_reserve_disk(&self, bytes: u64) -> Result<()> {
        let bytes = bytes.max(1);
        let mut current = self.disk_bytes.load(Ordering::Relaxed);
        loop {
            let next = current
                .checked_add(bytes)
                .context("gateway spool disk byte counter overflow")?;
            anyhow::ensure!(
                next <= self.config.disk_max_bytes,
                "gateway spool disk cap exceeded: {next} > {}",
                self.config.disk_max_bytes
            );
            match self.disk_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
    }

    fn release_disk(&self, bytes: u64) {
        if bytes > 0 {
            let mut current = self.disk_bytes.load(Ordering::Relaxed);
            loop {
                let next = current.saturating_sub(bytes);
                match self.disk_bytes.compare_exchange_weak(
                    current,
                    next,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return,
                    Err(actual) => current = actual,
                }
            }
        }
    }
}

impl GatewayForwardMetrics {
    pub(crate) fn snapshot(&self) -> GatewayForwardMetricsSnapshot {
        GatewayForwardMetricsSnapshot {
            queued_events: self.queued_events.load(Ordering::Relaxed),
            delivered_events: self.delivered_events.load(Ordering::Relaxed),
            retry_attempts: self.retry_attempts.load(Ordering::Relaxed),
            active_queues: self.active_queues.load(Ordering::Relaxed),
            current_queue_depth: self.current_queue_depth.load(Ordering::Relaxed),
            oldest_event_age_secs: oldest_event_age_secs(
                self.current_queue_depth.load(Ordering::Relaxed),
                self.oldest_event_unix.load(Ordering::Relaxed),
            ),
            dropped_events: self.dropped_events.load(Ordering::Relaxed),
            telemetry_dropped_events: self.telemetry_dropped_events.load(Ordering::Relaxed),
            expired_events: self.expired_events.load(Ordering::Relaxed),
            critical_failures: self.critical_failures.load(Ordering::Relaxed),
            dropped_by_kind: self.dropped_by_kind.snapshot(),
            dropped_by_reason: self.dropped_by_reason.snapshot(),
            critical_failures_by_reason: self.critical_failures_by_reason.snapshot(),
            retained_output_truncated_events: self
                .retained_output_truncated_events
                .load(Ordering::Relaxed),
            rejected_agent_connections: self.rejected_agent_connections.load(Ordering::Relaxed),
            unhealthy: self.unhealthy.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn record_retained_output_truncated(&self, count: u64) {
        self.retained_output_truncated_events
            .fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn record_rejected_agent_connection(&self) {
        self.rejected_agent_connections
            .fetch_add(1, Ordering::Relaxed);
    }

    fn record_drop(&self, kind: GatewayForwardEventKind, reason: GatewayForwardDropReason) {
        self.dropped_events.fetch_add(1, Ordering::Relaxed);
        if kind == GatewayForwardEventKind::Telemetry {
            self.telemetry_dropped_events
                .fetch_add(1, Ordering::Relaxed);
        }
        self.dropped_by_kind.increment(kind);
        self.dropped_by_reason.increment(reason);
        if reason == GatewayForwardDropReason::Expired {
            self.expired_events.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_critical_failure(&self, reason: GatewayForwardDropReason) {
        self.critical_failures.fetch_add(1, Ordering::Relaxed);
        self.critical_failures_by_reason.increment(reason);
        self.unhealthy.store(true, Ordering::Relaxed);
    }
}

impl GatewayForwardKindAtomicCounters {
    fn increment(&self, kind: GatewayForwardEventKind) {
        match kind {
            GatewayForwardEventKind::Telemetry => &self.telemetry,
            GatewayForwardEventKind::CommandOutput => &self.command_output,
            GatewayForwardEventKind::Lifecycle => &self.lifecycle,
            GatewayForwardEventKind::TerminalOutput => &self.terminal_output,
            GatewayForwardEventKind::Other => &self.other,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> GatewayForwardEventKindCounters {
        GatewayForwardEventKindCounters {
            telemetry: self.telemetry.load(Ordering::Relaxed),
            command_output: self.command_output.load(Ordering::Relaxed),
            lifecycle: self.lifecycle.load(Ordering::Relaxed),
            terminal_output: self.terminal_output.load(Ordering::Relaxed),
            other: self.other.load(Ordering::Relaxed),
        }
    }
}

impl GatewayForwardDropReasonAtomicCounters {
    fn increment(&self, reason: GatewayForwardDropReason) {
        match reason {
            GatewayForwardDropReason::GlobalQueueFull => &self.global_queue_full,
            GatewayForwardDropReason::TargetQueueFull => &self.target_queue_full,
            GatewayForwardDropReason::Expired => &self.expired,
            GatewayForwardDropReason::Coalesced => &self.coalesced,
            GatewayForwardDropReason::ProtocolConflict => &self.protocol_conflict,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> GatewayForwardDropReasonCounters {
        GatewayForwardDropReasonCounters {
            global_queue_full: self.global_queue_full.load(Ordering::Relaxed),
            target_queue_full: self.target_queue_full.load(Ordering::Relaxed),
            expired: self.expired.load(Ordering::Relaxed),
            coalesced: self.coalesced.load(Ordering::Relaxed),
            protocol_conflict: self.protocol_conflict.load(Ordering::Relaxed),
        }
    }
}

impl GatewayForwardCriticalFailureAtomicCounters {
    fn increment(&self, reason: GatewayForwardDropReason) {
        match reason {
            GatewayForwardDropReason::GlobalQueueFull => &self.global_queue_full,
            GatewayForwardDropReason::TargetQueueFull => &self.target_queue_full,
            GatewayForwardDropReason::Expired => &self.expired,
            GatewayForwardDropReason::Coalesced => return,
            GatewayForwardDropReason::ProtocolConflict => return,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> GatewayForwardCriticalFailureCounters {
        GatewayForwardCriticalFailureCounters {
            global_queue_full: self.global_queue_full.load(Ordering::Relaxed),
            target_queue_full: self.target_queue_full.load(Ordering::Relaxed),
            expired: self.expired.load(Ordering::Relaxed),
        }
    }
}

async fn run_forward_queue(
    target_key: String,
    mut receiver: mpsc::Receiver<GatewayForwardQueueItem>,
    telemetry_pending: Arc<Mutex<HashMap<String, GatewayForwardEvent>>>,
    metrics: Arc<GatewayForwardMetrics>,
    critical_failure_handler: Arc<StdRwLock<Option<CriticalForwardingFailureHandler>>>,
    spool: Arc<GatewayForwardSpool>,
    runtime_config: Arc<GatewayForwardRuntimeConfig>,
    timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
) {
    while let Some(item) = receiver.recv().await {
        let Some(handle) = queue_item_event(
            item,
            &target_key,
            &telemetry_pending,
            &metrics,
            &critical_failure_handler,
            &spool,
        )
        .await
        else {
            finish_forward_event(&metrics, &spool, None, false).await;
            continue;
        };
        let event = &handle.event;
        if telemetry_superseded(event, &target_key, &telemetry_pending).await {
            metrics.record_drop(event.kind, GatewayForwardDropReason::Coalesced);
            warn!(
                path = %event.path,
                kind = ?event.kind,
                target_key,
                "dropped superseded gateway telemetry before API forwarding"
            );
            finish_forward_event(&metrics, &spool, Some(&handle), false).await;
            continue;
        }
        if event.expired(&runtime_config) {
            metrics.record_drop(event.kind, GatewayForwardDropReason::Expired);
            if event.critical {
                metrics.record_critical_failure(GatewayForwardDropReason::Expired);
                notify_critical_failure(
                    &critical_failure_handler,
                    &target_key,
                    GatewayForwardDropReason::Expired,
                );
            }
            warn!(
                path = %event.path,
                kind = ?event.kind,
                target_key,
                "expired gateway event before API forwarding"
            );
            finish_forward_event(&metrics, &spool, Some(&handle), false).await;
            continue;
        }
        let outcome = post_json_retry_until_expired(
            event,
            &target_key,
            &metrics,
            &critical_failure_handler,
            &telemetry_pending,
            &spool,
            &runtime_config,
            &timeouts,
        )
        .await;
        match outcome {
            GatewayForwardOutcome::Delivered => {
                metrics.delivered_events.fetch_add(1, Ordering::Relaxed);
            }
            GatewayForwardOutcome::DeferredToSpool => {}
            GatewayForwardOutcome::DeferredForShutdown => {
                if handle.spool_path.is_none() {
                    if let Err(error) = spool.spool_event(&target_key, event).await {
                        metrics.record_drop(event.kind, GatewayForwardDropReason::GlobalQueueFull);
                        metrics.record_critical_failure(GatewayForwardDropReason::GlobalQueueFull);
                        notify_critical_failure(
                            &critical_failure_handler,
                            &target_key,
                            GatewayForwardDropReason::GlobalQueueFull,
                        );
                        warn!(
                            %error,
                            path = %event.path,
                            target_key,
                            "failed to spool gateway event during shutdown"
                        );
                    }
                }
            }
            GatewayForwardOutcome::NotDelivered => {}
        }
        finish_forward_event(
            &metrics,
            &spool,
            Some(&handle),
            outcome == GatewayForwardOutcome::DeferredForShutdown,
        )
        .await;
    }
    metrics.active_queues.fetch_sub(1, Ordering::Relaxed);
}

async fn queue_item_event(
    item: GatewayForwardQueueItem,
    target_key: &str,
    telemetry_pending: &Mutex<HashMap<String, GatewayForwardEvent>>,
    metrics: &GatewayForwardMetrics,
    critical_failure_handler: &StdRwLock<Option<CriticalForwardingFailureHandler>>,
    spool: &GatewayForwardSpool,
) -> Option<GatewayForwardEventHandle> {
    match item {
        GatewayForwardQueueItem::Event { event, ram_bytes } => Some(GatewayForwardEventHandle {
            event,
            ram_bytes,
            spool_path: None,
            spool_bytes: 0,
        }),
        GatewayForwardQueueItem::Spooled {
            path,
            disk_bytes,
            kind,
            critical,
            ..
        } => match spool.load_spooled_event(&path).await {
            Ok(mut event) => {
                if let Err(error) = mark_spooled_replay_event(&mut event) {
                    warn!(
                        %error,
                        path = %path.display(),
                        target_key,
                        "failed to mark spooled gateway event as replay"
                    );
                }
                Some(GatewayForwardEventHandle {
                    event,
                    ram_bytes: 0,
                    spool_path: Some(path),
                    spool_bytes: disk_bytes,
                })
            }
            Err(error) => {
                metrics.record_drop(kind, GatewayForwardDropReason::GlobalQueueFull);
                if critical {
                    metrics.record_critical_failure(GatewayForwardDropReason::GlobalQueueFull);
                    notify_critical_failure(
                        critical_failure_handler,
                        target_key,
                        GatewayForwardDropReason::GlobalQueueFull,
                    );
                }
                warn!(
                    %error,
                    path = %path.display(),
                    target_key,
                    "failed to load spooled gateway event"
                );
                spool.remove_spooled_file(&path, disk_bytes).await;
                None
            }
        },
        GatewayForwardQueueItem::Telemetry { .. } => telemetry_pending
            .lock()
            .await
            .remove(target_key)
            .map(|event| GatewayForwardEventHandle {
                event,
                ram_bytes: 0,
                spool_path: None,
                spool_bytes: 0,
            }),
    }
}

async fn telemetry_superseded(
    event: &GatewayForwardEvent,
    target_key: &str,
    telemetry_pending: &Mutex<HashMap<String, GatewayForwardEvent>>,
) -> bool {
    event.kind == GatewayForwardEventKind::Telemetry
        && telemetry_pending.lock().await.contains_key(target_key)
}

async fn finish_forward_event(
    metrics: &GatewayForwardMetrics,
    spool: &GatewayForwardSpool,
    handle: Option<&GatewayForwardEventHandle>,
    preserve_spool_file: bool,
) {
    if let Some(handle) = handle {
        spool.release_ram(handle.ram_bytes);
        if let Some(path) = handle
            .spool_path
            .as_deref()
            .filter(|_| !preserve_spool_file)
        {
            spool.remove_spooled_file(path, handle.spool_bytes).await;
        }
    }
    let previous = metrics.current_queue_depth.fetch_sub(1, Ordering::Relaxed);
    if previous <= 1 {
        metrics.oldest_event_unix.store(0, Ordering::Relaxed);
    }
}

async fn spool_event_for_later_replay(
    spool: &GatewayForwardSpool,
    target_key: &str,
    event: &GatewayForwardEvent,
    reason: GatewayForwardDropReason,
) -> Result<()> {
    let _ = spool.spool_event(target_key, event).await?;
    warn!(
        path = %event.path,
        kind = ?event.kind,
        target_key,
        reason = reason.as_str(),
        "spooled critical gateway output for replay"
    );
    Ok(())
}

async fn post_json_retry_until_expired(
    event: &GatewayForwardEvent,
    target_key: &str,
    metrics: &GatewayForwardMetrics,
    critical_failure_handler: &StdRwLock<Option<CriticalForwardingFailureHandler>>,
    telemetry_pending: &Mutex<HashMap<String, GatewayForwardEvent>>,
    spool: &GatewayForwardSpool,
    runtime_config: &GatewayForwardRuntimeConfig,
    timeouts: &StdRwLock<GatewayHttpTimeouts>,
) -> GatewayForwardOutcome {
    let mut attempt = 1_u64;
    loop {
        if spool.shutdown_requested() {
            return GatewayForwardOutcome::DeferredForShutdown;
        }
        if telemetry_superseded(event, target_key, telemetry_pending).await {
            metrics.record_drop(event.kind, GatewayForwardDropReason::Coalesced);
            warn!(
                path = %event.path,
                kind = ?event.kind,
                target_key,
                attempt,
                "stopped retrying superseded gateway telemetry"
            );
            return GatewayForwardOutcome::NotDelivered;
        }
        let post_result = tokio::select! {
            result = post_json_bytes(
                &event.api_url,
                &event.path,
                &event.body,
                event.internal_token.as_deref(),
                current_gateway_http_timeouts(timeouts),
            ) => result,
            _ = spool.notified_shutdown() => {
                return GatewayForwardOutcome::DeferredForShutdown;
            }
        };
        match post_result {
            Ok(_) => return GatewayForwardOutcome::Delivered,
            Err(error) => {
                metrics.retry_attempts.fetch_add(1, Ordering::Relaxed);
                let error_message = error.to_string();
                if error_is_gateway_session_not_active(&error_message)
                    && event_spools_under_pressure(event)
                    && !event_marked_spooled_replay(event)
                {
                    match spool_event_for_later_replay(
                        spool,
                        target_key,
                        event,
                        GatewayForwardDropReason::ProtocolConflict,
                    )
                    .await
                    {
                        Ok(()) => return GatewayForwardOutcome::DeferredToSpool,
                        Err(spool_error) => {
                            metrics.record_drop(
                                event.kind,
                                GatewayForwardDropReason::ProtocolConflict,
                            );
                            metrics
                                .record_critical_failure(GatewayForwardDropReason::GlobalQueueFull);
                            notify_critical_failure(
                                critical_failure_handler,
                                target_key,
                                GatewayForwardDropReason::GlobalQueueFull,
                            );
                            warn!(
                                error = %error_message,
                                spool_error = %spool_error,
                                path = %event.path,
                                target_key,
                                attempt,
                                "failed to spool critical gateway output after session conflict"
                            );
                            return GatewayForwardOutcome::NotDelivered;
                        }
                    }
                }
                if gateway_event_error_is_non_retryable(event, &error_message) {
                    metrics.record_drop(event.kind, GatewayForwardDropReason::ProtocolConflict);
                    warn!(
                        error = %error_message,
                        path = %event.path,
                        target_key,
                        attempt,
                        "dropping non-retryable gateway event"
                    );
                    return GatewayForwardOutcome::NotDelivered;
                }
                if spool.shutdown_requested() {
                    return GatewayForwardOutcome::DeferredForShutdown;
                }
                if event.expired(runtime_config) {
                    metrics.record_drop(event.kind, GatewayForwardDropReason::Expired);
                    if event.critical {
                        metrics.record_critical_failure(GatewayForwardDropReason::Expired);
                        notify_critical_failure(
                            critical_failure_handler,
                            target_key,
                            GatewayForwardDropReason::Expired,
                        );
                    }
                    warn!(
                        error = %error_message,
                        path = %event.path,
                        kind = ?event.kind,
                        target_key,
                        attempt,
                        "gateway event forwarding expired"
                    );
                    return GatewayForwardOutcome::NotDelivered;
                }
                warn!(
                    error = %error_message,
                    path = %event.path,
                    target_key,
                    attempt,
                    "failed to forward gateway event to API"
                );
                let backoff_ms =
                    250_u64.saturating_mul(2_u64.saturating_pow((attempt - 1).min(7) as u32));
                sleep(Duration::from_millis(backoff_ms.min(5_000))).await;
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

fn notify_critical_failure(
    handler_slot: &StdRwLock<Option<CriticalForwardingFailureHandler>>,
    target_key: &str,
    reason: GatewayForwardDropReason,
) {
    if let Ok(slot) = handler_slot.read() {
        if let Some(handler) = slot.as_ref() {
            handler(target_key.to_string(), reason.as_str());
        }
    }
}

fn gateway_event_error_is_non_retryable(event: &GatewayForwardEvent, error_message: &str) -> bool {
    if !error_message.contains("409 Conflict") {
        return false;
    }
    if error_message.contains("gateway_session_not_active") {
        return !event_spools_under_pressure(event) || event_marked_spooled_replay(event);
    }
    event.kind == GatewayForwardEventKind::CommandOutput
        && (error_message.contains("job_output_sequence_conflict")
            || error_message.contains("job_target_not_active")
            || error_message.contains("job_output_payload_hash_mismatch"))
}

async fn post_json<T: serde::Serialize>(
    base_url: &str,
    path: &str,
    value: &T,
    internal_token: Option<&str>,
    timeouts: GatewayHttpTimeouts,
) -> Result<String> {
    let body = serde_json::to_vec(value)?;
    post_json_bytes(base_url, path, &body, internal_token, timeouts).await
}

async fn post_json_bytes(
    base_url: &str,
    path: &str,
    body: &[u8],
    internal_token: Option<&str>,
    timeouts: GatewayHttpTimeouts,
) -> Result<String> {
    time::timeout(
        timeouts.event_post,
        post_json_bytes_inner(base_url, path, body, internal_token, timeouts),
    )
    .await
    .context("API event post timed out")?
}

async fn post_json_bytes_inner(
    base_url: &str,
    path: &str,
    body: &[u8],
    internal_token: Option<&str>,
    timeouts: GatewayHttpTimeouts,
) -> Result<String> {
    let without_scheme = base_url
        .strip_prefix("http://")
        .context("gateway internal API forwarding currently supports http:// URLs")?;
    let (host_port, prefix) = without_scheme
        .split_once('/')
        .map(|(host, rest)| (host, format!("/{rest}")))
        .unwrap_or((without_scheme, String::new()));
    let request_path = format!("{prefix}{path}");
    let mut stream = time::timeout(timeouts.connect, TcpStream::connect(host_port))
        .await
        .context("API connect timed out")?
        .with_context(|| format!("failed to connect to API at {host_port}"))?;
    let token = internal_token.context("gateway API internal token is not configured")?;
    let auth_header = format!("Authorization: Bearer {token}\r\n");
    let request = format!(
        "POST {request_path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n{auth_header}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    time::timeout(timeouts.write, stream.write_all(request.as_bytes()))
        .await
        .context("API request header write timed out")??;
    time::timeout(timeouts.write, stream.write_all(body))
        .await
        .context("API request body write timed out")??;

    let mut response = Vec::new();
    time::timeout(timeouts.read, stream.read_to_end(&mut response))
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
        return Err(anyhow!("API returned {status}: {}", body.trim()));
    }
    Ok(body.trim().to_string())
}

impl GatewayForwardEventKind {
    fn for_path(path: &str) -> Self {
        match path {
            "/internal/v1/gateway/telemetry" => Self::Telemetry,
            "/internal/v1/gateway/command-output" => Self::CommandOutput,
            "/internal/v1/gateway/session-started"
            | "/internal/v1/gateway/session-ended"
            | "/internal/v1/gateway/agent-hello" => Self::Lifecycle,
            "/internal/v1/gateway/terminal-output" => Self::TerminalOutput,
            _ => Self::Other,
        }
    }

    fn ttl(self, runtime_config: &GatewayForwardRuntimeConfig) -> Duration {
        match self {
            Self::Telemetry => TELEMETRY_EVENT_TTL,
            Self::CommandOutput => runtime_config.command_output_event_ttl(),
            Self::Lifecycle => CRITICAL_EVENT_TTL,
            Self::TerminalOutput | Self::Other => NONCRITICAL_EVENT_TTL,
        }
    }
}

fn gateway_event_critical(kind: GatewayForwardEventKind, body: &[u8]) -> bool {
    match kind {
        GatewayForwardEventKind::CommandOutput | GatewayForwardEventKind::Lifecycle => true,
        GatewayForwardEventKind::TerminalOutput => terminal_output_final_status(body),
        GatewayForwardEventKind::Telemetry | GatewayForwardEventKind::Other => false,
    }
}

fn event_spools_under_pressure(event: &GatewayForwardEvent) -> bool {
    event.critical
        && matches!(
            event.kind,
            GatewayForwardEventKind::CommandOutput | GatewayForwardEventKind::TerminalOutput
        )
}

fn terminal_output_final_status(body: &[u8]) -> bool {
    serde_json::from_slice::<GatewayTerminalOutputIngest>(body)
        .map(|event| event.output.output.stream == OutputStream::Status && event.output.output.done)
        .unwrap_or(false)
}

fn mark_spooled_replay_event(event: &mut GatewayForwardEvent) -> Result<()> {
    match event.kind {
        GatewayForwardEventKind::CommandOutput => {
            let mut ingest: GatewayCommandOutputIngest =
                serde_json::from_slice(&event.body).context("decode spooled command output")?;
            ingest.spooled_replay = true;
            event.command_output = Some(CommandOutputReplayRef::from(&ingest));
            event.body = serde_json::to_vec(&ingest).context("encode spooled command output")?;
        }
        GatewayForwardEventKind::TerminalOutput => {
            let mut ingest: GatewayTerminalOutputIngest =
                serde_json::from_slice(&event.body).context("decode spooled terminal output")?;
            ingest.spooled_replay = true;
            event.body = serde_json::to_vec(&ingest).context("encode spooled terminal output")?;
        }
        GatewayForwardEventKind::Telemetry
        | GatewayForwardEventKind::Lifecycle
        | GatewayForwardEventKind::Other => {}
    }
    Ok(())
}

fn event_marked_spooled_replay(event: &GatewayForwardEvent) -> bool {
    match event.kind {
        GatewayForwardEventKind::CommandOutput => {
            serde_json::from_slice::<GatewayCommandOutputIngest>(&event.body)
                .map(|event| event.spooled_replay)
                .unwrap_or(false)
        }
        GatewayForwardEventKind::TerminalOutput => {
            serde_json::from_slice::<GatewayTerminalOutputIngest>(&event.body)
                .map(|event| event.spooled_replay)
                .unwrap_or(false)
        }
        GatewayForwardEventKind::Telemetry
        | GatewayForwardEventKind::Lifecycle
        | GatewayForwardEventKind::Other => false,
    }
}

fn error_is_gateway_session_not_active(error_message: &str) -> bool {
    error_message.contains("409 Conflict") && error_message.contains("gateway_session_not_active")
}

impl GatewayForwardEvent {
    fn expired(&self, runtime_config: &GatewayForwardRuntimeConfig) -> bool {
        self.created_at.elapsed() >= self.ttl(runtime_config)
    }

    fn ttl(&self, runtime_config: &GatewayForwardRuntimeConfig) -> Duration {
        match self.kind {
            GatewayForwardEventKind::TerminalOutput if self.critical => {
                runtime_config.command_output_event_ttl()
            }
            kind => kind.ttl(runtime_config),
        }
    }
}

impl GatewayForwardQueueItem {
    fn created_unix(&self) -> u64 {
        match self {
            Self::Event { event, .. } => event.created_unix,
            Self::Spooled { created_unix, .. } => *created_unix,
            Self::Telemetry { created_unix } => *created_unix,
        }
    }

    fn enqueue_seq(&self) -> u64 {
        match self {
            Self::Event { event, .. } => event.enqueue_seq,
            Self::Spooled { enqueue_seq, .. } => *enqueue_seq,
            Self::Telemetry { .. } => 0,
        }
    }
}

fn parse_spool_filename(path: &Path) -> Option<(u64, String)> {
    let file_name = path.file_name()?.to_str()?;
    let stem = file_name.strip_suffix(".spool")?;
    let mut parts = stem.splitn(3, '-');
    let created_unix = parts.next()?.parse::<u64>().ok()?;
    let target_hex = parts.next()?;
    let _uuid = parts.next()?;
    let target_bytes = hex::decode(target_hex).ok()?;
    let target_key = String::from_utf8(target_bytes).ok()?;
    Some((created_unix, target_key))
}

fn decode_spooled_event(path: &Path, bytes: &[u8]) -> Result<GatewayForwardEvent> {
    let body = bytes
        .strip_prefix(SPOOL_MAGIC)
        .with_context(|| format!("gateway spool file {} has invalid magic", path.display()))?;
    let newline = body
        .iter()
        .position(|value| *value == b'\n')
        .with_context(|| format!("gateway spool file {} has no header length", path.display()))?;
    let header_len = std::str::from_utf8(&body[..newline])
        .with_context(|| {
            format!(
                "gateway spool file {} has invalid header length",
                path.display()
            )
        })?
        .parse::<usize>()
        .with_context(|| {
            format!(
                "gateway spool file {} has invalid header length",
                path.display()
            )
        })?;
    let header_start = newline + 1;
    let header_end = header_start.checked_add(header_len).with_context(|| {
        format!(
            "gateway spool file {} header length overflowed",
            path.display()
        )
    })?;
    anyhow::ensure!(
        header_end <= body.len(),
        "gateway spool file {} is truncated",
        path.display()
    );
    let header: SpooledGatewayForwardHeader =
        serde_json::from_slice(&body[header_start..header_end])
            .with_context(|| format!("failed to decode gateway spool header {}", path.display()))?;
    validate_spooled_header(path, &header)?;
    let event_body = &body[header_end..];
    anyhow::ensure!(
        payload_hash(event_body) == header.body_sha256_hex,
        "gateway spool file {} checksum mismatch",
        path.display()
    );
    let age_secs = unix_now().saturating_sub(header.created_unix);
    let now = time::Instant::now();
    let created_at = now
        .checked_sub(Duration::from_secs(age_secs))
        .unwrap_or(now);
    Ok(GatewayForwardEvent {
        api_url: header.api_url,
        path: header.path,
        body: event_body.to_vec(),
        internal_token: header.internal_token,
        kind: header.kind,
        critical: header.critical,
        command_output: header.command_output,
        created_at,
        created_unix: header.created_unix,
        enqueue_seq: header.enqueue_seq,
    })
}

fn validate_spooled_header(path: &Path, header: &SpooledGatewayForwardHeader) -> Result<()> {
    anyhow::ensure!(
        header.schema_version == SPOOL_SCHEMA_VERSION,
        "gateway spool file {} has unsupported schema version {}",
        path.display(),
        header.schema_version
    );
    anyhow::ensure!(
        header.body_sha256_hex.len() == 64
            && header
                .body_sha256_hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "gateway spool file {} has invalid body checksum",
        path.display()
    );
    Ok(())
}

async fn fsync_dir_best_effort(path: &Path, label: &'static str) {
    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        std::fs::File::open(&path).and_then(|file| file.sync_all())
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!(%error, label, "failed to fsync directory"),
        Err(error) => warn!(%error, label, "failed to join directory fsync task"),
    }
}

fn command_output_replay_ref_from_body(body: &[u8]) -> Option<CommandOutputReplayRef> {
    serde_json::from_slice::<GatewayCommandOutputIngest>(body)
        .ok()
        .map(|event| CommandOutputReplayRef::from(&event))
}

impl GatewayForwardDropReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::GlobalQueueFull => "global_queue_full",
            Self::TargetQueueFull => "target_queue_full",
            Self::Expired => "expired",
            Self::Coalesced => "coalesced",
            Self::ProtocolConflict => "protocol_conflict",
        }
    }
}

fn oldest_event_age_secs(current_depth: u64, oldest_unix: u64) -> Option<u64> {
    if current_depth == 0 || oldest_unix == 0 {
        None
    } else {
        Some(unix_now().saturating_sub(oldest_unix))
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
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

#[derive(Debug, Deserialize)]
pub(crate) struct GatewayIngestResponse {
    pub(crate) accepted: bool,
    pub(crate) message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tokio::sync::oneshot;

    static TEST_ENQUEUE_SEQ: AtomicU64 = AtomicU64::new(1);

    fn test_event(path: &str, body: &[u8]) -> GatewayForwardEvent {
        let kind = GatewayForwardEventKind::for_path(path);
        GatewayForwardEvent {
            api_url: "http://127.0.0.1:9".to_string(),
            path: path.to_string(),
            body: body.to_vec(),
            internal_token: Some("test-token".to_string()),
            kind,
            critical: gateway_event_critical(kind, body),
            command_output: None,
            created_at: time::Instant::now(),
            created_unix: unix_now(),
            enqueue_seq: TEST_ENQUEUE_SEQ.fetch_add(1, Ordering::Relaxed),
        }
    }

    fn terminal_output_event(
        job_id: uuid::Uuid,
        stream: vpsman_common::OutputStream,
        terminal_seq: Option<u64>,
        done: bool,
        data: Vec<u8>,
    ) -> GatewayTerminalOutputIngest {
        GatewayTerminalOutputIngest {
            gateway_id: "gateway-a".to_string(),
            gateway_session_id: uuid::Uuid::new_v4(),
            process_incarnation_id: uuid::Uuid::new_v4(),
            spooled_replay: false,
            client_id: "client-a".to_string(),
            output: vpsman_common::TerminalStreamOutput {
                job_id,
                session_id: uuid::Uuid::new_v4(),
                terminal_seq,
                output_first_seq: Some(1),
                output_next_seq: terminal_seq.unwrap_or(1).saturating_add(1),
                output_retained_first_seq: Some(1),
                output_retained_bytes: data.len() as u64,
                output_dropped_bytes: 0,
                output_dropped_chunks: 0,
                output_replay_truncated: false,
                output: vpsman_common::CommandOutput {
                    job_id,
                    stream,
                    data,
                    exit_code: done.then_some(0),
                    done,
                },
            },
        }
    }

    #[tokio::test]
    async fn shutdown_defers_non_command_events_to_spool() {
        let event = test_event("/internal/v1/gateway/session-started", br#"{}"#);
        let metrics = GatewayForwardMetrics::default();
        let critical_failure_handler = StdRwLock::new(None);
        let telemetry_pending = Mutex::new(HashMap::new());
        let spool = GatewayForwardSpool::new(GatewaySpoolConfig::default());
        let runtime_config = GatewayForwardRuntimeConfig::default();
        let timeouts = StdRwLock::new(GatewayHttpTimeouts::default());
        spool.request_shutdown();

        let outcome = post_json_retry_until_expired(
            &event,
            "client-a",
            &metrics,
            &critical_failure_handler,
            &telemetry_pending,
            &spool,
            &runtime_config,
            &timeouts,
        )
        .await;

        assert_eq!(outcome, GatewayForwardOutcome::DeferredForShutdown);
        assert_eq!(metrics.retry_attempts.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn shutdown_interrupts_blocked_api_forward_post() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = [0_u8; 1024];
            let _ = socket.read(&mut buffer).await;
            std::future::pending::<()>().await;
        });

        let mut event = test_event("/internal/v1/gateway/session-started", br#"{}"#);
        event.api_url = format!("http://{addr}");
        let metrics = GatewayForwardMetrics::default();
        let critical_failure_handler = StdRwLock::new(None);
        let telemetry_pending = Mutex::new(HashMap::new());
        let spool = GatewayForwardSpool::new(GatewaySpoolConfig::default());
        let runtime_config = GatewayForwardRuntimeConfig::default();
        let timeouts = StdRwLock::new(GatewayHttpTimeouts {
            connect: Duration::from_secs(1),
            write: Duration::from_secs(1),
            read: Duration::from_secs(60),
            event_post: Duration::from_secs(60),
        });
        let forward = post_json_retry_until_expired(
            &event,
            "client-a",
            &metrics,
            &critical_failure_handler,
            &telemetry_pending,
            &spool,
            &runtime_config,
            &timeouts,
        );
        tokio::pin!(forward);
        sleep(Duration::from_millis(50)).await;
        spool.request_shutdown();

        let outcome = time::timeout(Duration::from_secs(1), forward)
            .await
            .unwrap();
        assert_eq!(outcome, GatewayForwardOutcome::DeferredForShutdown);
    }

    #[tokio::test]
    async fn post_without_api_url_returns_error() {
        let client = GatewayControlClient::new(None, None, GatewayHttpTimeouts::default());
        let error = client
            .post(
                "client-a",
                "/internal/v1/gateway/session-started",
                &serde_json::json!({}),
            )
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("gateway API URL is required"));
    }

    #[tokio::test]
    async fn telemetry_enqueue_keeps_latest_pending_event() {
        let forwarder = GatewayEventForwarder::default();
        let (sender, _receiver) = mpsc::channel(PER_TARGET_QUEUE_CAPACITY);
        forwarder.queues.lock().await.insert(
            "client-a".to_string(),
            GatewayForwardQueue {
                sender,
                last_enqueue_unix: unix_now(),
            },
        );

        forwarder
            .enqueue(
                "client-a".to_string(),
                test_event("/internal/v1/gateway/telemetry", br#"{"seq":1}"#),
                test_timeouts(),
            )
            .await
            .unwrap();
        forwarder
            .enqueue(
                "client-a".to_string(),
                test_event("/internal/v1/gateway/telemetry", br#"{"seq":2}"#),
                test_timeouts(),
            )
            .await
            .unwrap();

        let pending = forwarder.telemetry_pending.lock().await;
        assert_eq!(
            pending.get("client-a").map(|event| event.body.as_slice()),
            Some(br#"{"seq":2}"#.as_slice())
        );
        let snapshot = forwarder.metrics.snapshot();
        assert_eq!(snapshot.queued_events, 1);
        assert_eq!(snapshot.current_queue_depth, 1);
        assert_eq!(snapshot.dropped_events, 1);
        assert_eq!(snapshot.telemetry_dropped_events, 1);
        assert_eq!(snapshot.dropped_by_kind.telemetry, 1);
        assert_eq!(snapshot.dropped_by_reason.coalesced, 1);
    }

    #[tokio::test]
    async fn idle_forward_queue_is_reaped_on_next_enqueue() {
        let forwarder = GatewayEventForwarder::default();
        let (sender, _receiver) = mpsc::channel(PER_TARGET_QUEUE_CAPACITY);
        forwarder.queues.lock().await.insert(
            "idle-client".to_string(),
            GatewayForwardQueue {
                sender,
                last_enqueue_unix: unix_now().saturating_sub(QUEUE_IDLE_REAP_SECS + 1),
            },
        );

        forwarder
            .enqueue(
                "active-client".to_string(),
                test_event("/internal/v1/gateway/session-started", br#"{}"#),
                test_timeouts(),
            )
            .await
            .unwrap();

        let queues = forwarder.queues.lock().await;
        assert!(!queues.contains_key("idle-client"));
        assert!(queues.contains_key("active-client"));
    }

    #[tokio::test]
    async fn critical_enqueue_overflow_marks_unhealthy_and_notifies_handler() {
        let forwarder = GatewayEventForwarder::default();
        forwarder
            .metrics
            .current_queue_depth
            .store(GLOBAL_QUEUE_CAPACITY, Ordering::Relaxed);
        let (sent, received) = oneshot::channel::<(String, &'static str)>();
        let sent = std::sync::Mutex::new(Some(sent));
        *forwarder.critical_failure_handler.write().unwrap() =
            Some(Arc::new(move |client_id, reason| {
                if let Some(sender) = sent.lock().unwrap().take() {
                    let _ = sender.send((client_id, reason));
                }
            }));

        let result = forwarder
            .enqueue(
                "client-a".to_string(),
                test_event("/internal/v1/gateway/command-output", br#"{}"#),
                test_timeouts(),
            )
            .await;

        assert!(result.is_err());
        let (client_id, reason) = received.await.unwrap();
        assert_eq!(client_id, "client-a");
        assert_eq!(reason, "global_queue_full");
        let snapshot = forwarder.metrics.snapshot();
        assert!(snapshot.unhealthy);
        assert_eq!(snapshot.critical_failures, 1);
        assert_eq!(snapshot.critical_failures_by_reason.global_queue_full, 1);
        assert_eq!(snapshot.dropped_by_kind.command_output, 1);
    }

    #[tokio::test]
    async fn command_output_over_ram_budget_spools_to_disk() {
        let dir =
            std::env::temp_dir().join(format!("vpsman-gateway-spool-{}", uuid::Uuid::new_v4()));
        let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
            dir.clone(),
            1024 * 1024,
            8 * 1024 * 1024,
            30,
        ));
        let body = vec![b'x'; 1024 * 1024 + 1];
        let event = test_event("/internal/v1/gateway/command-output", &body);

        let item = forwarder
            .prepare_queue_item("client-a", event)
            .await
            .unwrap();

        let GatewayForwardQueueItem::Spooled {
            path, disk_bytes, ..
        } = item
        else {
            panic!("command output above RAM budget should spool");
        };
        assert!(path.exists());
        assert!(disk_bytes > body.len() as u64);
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("pending")), 0o700);
        assert_eq!(mode(&path), 0o600);
        let decoded = forwarder.spool.load_spooled_event(&path).await.unwrap();
        assert_eq!(decoded.body, body);
        assert_eq!(decoded.kind, GatewayForwardEventKind::CommandOutput);
        assert!(decoded.critical);
        forwarder.spool.remove_spooled_file(&path, disk_bytes).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn terminal_output_over_ram_budget_spools_to_disk() {
        let dir = std::env::temp_dir().join(format!(
            "vpsman-gateway-terminal-spool-{}",
            uuid::Uuid::new_v4()
        ));
        let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
            dir.clone(),
            128,
            8 * 1024 * 1024,
            30,
        ));
        let job_id = uuid::Uuid::new_v4();
        let event = terminal_output_event(
            job_id,
            vpsman_common::OutputStream::Pty,
            Some(1),
            false,
            vec![0_u8; 1024 * 1024 + 1],
        );
        let body = serde_json::to_vec(&event).unwrap();
        assert!(body.len() as u64 > forwarder.spool.config.ram_max_bytes);
        let item = forwarder
            .prepare_queue_item(
                "client-a",
                test_event("/internal/v1/gateway/terminal-output", &body),
            )
            .await
            .unwrap();

        let GatewayForwardQueueItem::Spooled {
            path, disk_bytes, ..
        } = item
        else {
            panic!("terminal output above RAM budget should spool");
        };
        let decoded = forwarder.spool.load_spooled_event(&path).await.unwrap();
        assert_eq!(decoded.kind, GatewayForwardEventKind::TerminalOutput);
        assert!(!decoded.critical);
        forwarder.spool.remove_spooled_file(&path, disk_bytes).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn terminal_final_status_is_critical_with_command_output_ttl() {
        let job_id = uuid::Uuid::new_v4();
        let final_status = terminal_output_event(
            job_id,
            vpsman_common::OutputStream::Status,
            None,
            true,
            br#"{"type":"terminal_stream","status":"exited"}"#.to_vec(),
        );
        let body = serde_json::to_vec(&final_status).unwrap();
        let event = test_event("/internal/v1/gateway/terminal-output", &body);

        assert!(event.critical);
        assert_eq!(
            event.ttl(&GatewayForwardRuntimeConfig::new(
                GatewayForwardConfig::new(900)
            )),
            Duration::from_secs(900)
        );
    }

    #[tokio::test]
    async fn command_output_spool_header_preserves_command_output_replay_key() {
        let dir = std::env::temp_dir().join(format!(
            "vpsman-gateway-spool-header-{}",
            uuid::Uuid::new_v4()
        ));
        let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
            dir.clone(),
            1024 * 1024,
            8 * 1024 * 1024,
            30,
        ));
        let job_id = uuid::Uuid::new_v4();
        let ingest = GatewayCommandOutputIngest {
            gateway_id: "gateway-a".to_string(),
            gateway_session_id: uuid::Uuid::new_v4(),
            process_incarnation_id: uuid::Uuid::new_v4(),
            spooled_replay: false,
            client_id: "client-a".to_string(),
            job_id,
            payload_hash: "payload-a".to_string(),
            seq: 7,
            received_unix: Some(unix_now()),
            output: vpsman_common::CommandOutput {
                job_id,
                stream: vpsman_common::OutputStream::Status,
                data: br#"{"type":"ok"}"#.to_vec(),
                exit_code: Some(0),
                done: true,
            },
        };
        let replay_key = CommandOutputReplayRef::from(&ingest);
        let mut event = test_event(
            COMMAND_OUTPUT_PATH,
            &serde_json::to_vec(&ingest).expect("serialize ingest"),
        );
        event.command_output = Some(replay_key.clone());

        let GatewayForwardQueueItem::Spooled {
            path, disk_bytes, ..
        } = forwarder
            .spool
            .spool_event("client-a", &event)
            .await
            .unwrap()
        else {
            panic!("spool_event must return a spooled item");
        };

        let header = forwarder.spool.load_spooled_header(&path).await.unwrap();
        assert_eq!(header.command_output, Some(replay_key));
        assert!(header.critical);
        forwarder.spool.remove_spooled_file(&path, disk_bytes).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn command_output_spool_replay_marker_sets_body_field() {
        let job_id = uuid::Uuid::new_v4();
        let ingest = GatewayCommandOutputIngest {
            gateway_id: "gateway-a".to_string(),
            gateway_session_id: uuid::Uuid::new_v4(),
            process_incarnation_id: uuid::Uuid::new_v4(),
            spooled_replay: false,
            client_id: "client-a".to_string(),
            job_id,
            payload_hash: "a".repeat(64),
            seq: 7,
            received_unix: Some(unix_now()),
            output: vpsman_common::CommandOutput {
                job_id,
                stream: vpsman_common::OutputStream::Status,
                data: br#"{"type":"ok"}"#.to_vec(),
                exit_code: Some(0),
                done: true,
            },
        };
        let mut event = test_event(
            COMMAND_OUTPUT_PATH,
            &serde_json::to_vec(&ingest).expect("serialize ingest"),
        );

        mark_spooled_replay_event(&mut event).unwrap();

        let marked: GatewayCommandOutputIngest = serde_json::from_slice(&event.body).unwrap();
        assert!(marked.spooled_replay);
        assert!(event_marked_spooled_replay(&event));
        assert_eq!(
            event.command_output,
            Some(CommandOutputReplayRef::from(&marked))
        );
    }

    #[tokio::test]
    async fn full_target_queue_preserves_spooled_command_output_file() {
        let dir = std::env::temp_dir().join(format!(
            "vpsman-gateway-spool-pressure-{}",
            uuid::Uuid::new_v4()
        ));
        let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
            dir.clone(),
            1024 * 1024,
            8 * 1024 * 1024,
            30,
        ));
        let (sender, _receiver) = mpsc::channel(1);
        sender
            .try_send(GatewayForwardQueueItem::Telemetry {
                created_unix: unix_now(),
            })
            .unwrap();
        forwarder.queues.lock().await.insert(
            "client-a".to_string(),
            GatewayForwardQueue {
                sender,
                last_enqueue_unix: unix_now(),
            },
        );
        let event = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
        let item = forwarder
            .spool
            .spool_event("client-a", &event)
            .await
            .unwrap();
        let GatewayForwardQueueItem::Spooled {
            path, disk_bytes, ..
        } = &item
        else {
            panic!("spool_event must return a spooled item");
        };
        let path = path.clone();
        let disk_bytes = *disk_bytes;
        let accounted_before = forwarder.spool.disk_bytes.load(Ordering::Relaxed);

        let result = forwarder
            .enqueue_queue_item("client-a".to_string(), item, test_timeouts())
            .await;

        assert!(result.is_err());
        assert!(path.exists());
        assert_eq!(
            forwarder.spool.disk_bytes.load(Ordering::Relaxed),
            accounted_before
        );
        forwarder.spool.remove_spooled_file(&path, disk_bytes).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn pressure_spooled_output_counts_against_disk_cap() {
        let dir = std::env::temp_dir().join(format!(
            "vpsman-gateway-spool-disk-cap-{}",
            uuid::Uuid::new_v4()
        ));
        let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
            dir.clone(),
            1024 * 1024,
            1024 * 1024,
            30,
        ));
        let event = test_event(COMMAND_OUTPUT_PATH, &vec![b'x'; 700 * 1024]);

        spool_event_for_later_replay(
            &forwarder.spool,
            "client-a",
            &event,
            GatewayForwardDropReason::GlobalQueueFull,
        )
        .await
        .unwrap();
        assert_eq!(pending_spool_file_count(&dir), 1);
        assert!(forwarder.spool.disk_bytes.load(Ordering::Relaxed) > 0);

        let error = spool_event_for_later_replay(
            &forwarder.spool,
            "client-a",
            &event,
            GatewayForwardDropReason::GlobalQueueFull,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("gateway spool disk cap exceeded"));
        assert_eq!(pending_spool_file_count(&dir), 1);
        let replay = forwarder.spool.pending_items().await;
        assert_eq!(replay.len(), 1);
        if let GatewayForwardQueueItem::Spooled {
            path, disk_bytes, ..
        } = &replay[0].1
        {
            forwarder.spool.remove_spooled_file(path, *disk_bytes).await;
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn pending_spool_items_do_not_double_count_accounted_file() {
        let dir = std::env::temp_dir().join(format!(
            "vpsman-gateway-spool-accounted-replay-{}",
            uuid::Uuid::new_v4()
        ));
        let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
            dir.clone(),
            1024 * 1024,
            8 * 1024 * 1024,
            30,
        ));
        let event = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
        spool_event_for_later_replay(
            &forwarder.spool,
            "client-a",
            &event,
            GatewayForwardDropReason::TargetQueueFull,
        )
        .await
        .unwrap();
        let accounted_before = forwarder.spool.disk_bytes.load(Ordering::Relaxed);

        let first = forwarder.spool.pending_items().await;
        let after_first = forwarder.spool.disk_bytes.load(Ordering::Relaxed);
        let second = forwarder.spool.pending_items().await;
        let after_second = forwarder.spool.disk_bytes.load(Ordering::Relaxed);

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(after_first, accounted_before);
        assert_eq!(after_second, accounted_before);
        if let GatewayForwardQueueItem::Spooled {
            path, disk_bytes, ..
        } = &first[0].1
        {
            forwarder.spool.remove_spooled_file(path, *disk_bytes).await;
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn pending_spool_items_preserve_target_and_order() {
        let dir = std::env::temp_dir().join(format!(
            "vpsman-gateway-spool-replay-{}",
            uuid::Uuid::new_v4()
        ));
        let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
            dir.clone(),
            1024 * 1024,
            8 * 1024 * 1024,
            30,
        ));
        let event = test_event("/internal/v1/gateway/command-output", br#"{"seq":1}"#);
        let GatewayForwardQueueItem::Spooled { path, .. } = forwarder
            .spool
            .spool_event("client-a", &event)
            .await
            .unwrap()
        else {
            panic!("spool_event must return a spooled item");
        };

        let replay = forwarder.spool.pending_items().await;

        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].0, "client-a");
        assert!(matches!(
            replay[0].1,
            GatewayForwardQueueItem::Spooled {
                kind: GatewayForwardEventKind::CommandOutput,
                ..
            }
        ));
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn pending_spool_items_replay_by_durable_enqueue_sequence() {
        let dir = std::env::temp_dir().join(format!(
            "vpsman-gateway-spool-replay-order-{}",
            uuid::Uuid::new_v4()
        ));
        let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
            dir.clone(),
            1024 * 1024,
            8 * 1024 * 1024,
            30,
        ));
        let mut later = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":2}"#);
        later.created_unix = 100;
        later.enqueue_seq = 2;
        let mut earlier = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
        earlier.created_unix = 100;
        earlier.enqueue_seq = 1;

        let GatewayForwardQueueItem::Spooled {
            path: later_path, ..
        } = forwarder
            .spool
            .spool_event("client-a", &later)
            .await
            .unwrap()
        else {
            panic!("spool_event must return a spooled item");
        };
        let GatewayForwardQueueItem::Spooled {
            path: earlier_path, ..
        } = forwarder
            .spool
            .spool_event("client-a", &earlier)
            .await
            .unwrap()
        else {
            panic!("spool_event must return a spooled item");
        };

        let replay = forwarder.spool.pending_items().await;

        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0].0, "client-a");
        assert_eq!(replay[1].0, "client-a");
        assert!(matches!(
            replay[0].1,
            GatewayForwardQueueItem::Spooled { enqueue_seq: 1, .. }
        ));
        assert!(matches!(
            replay[1].1,
            GatewayForwardQueueItem::Spooled { enqueue_seq: 2, .. }
        ));
        let _ = std::fs::remove_file(earlier_path);
        let _ = std::fs::remove_file(later_path);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn forwarder_enqueue_sequence_starts_after_existing_spool() {
        let dir = std::env::temp_dir().join(format!(
            "vpsman-gateway-spool-seed-seq-{}",
            uuid::Uuid::new_v4()
        ));
        let config = GatewaySpoolConfig::enabled(dir.clone(), 1024 * 1024, 8 * 1024 * 1024, 30);
        let forwarder = GatewayEventForwarder::with_spool_config(config.clone());
        let mut existing = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
        existing.enqueue_seq = u64::MAX - 4;
        let GatewayForwardQueueItem::Spooled { path, .. } = forwarder
            .spool
            .spool_event("client-a", &existing)
            .await
            .unwrap()
        else {
            panic!("spool_event must return a spooled item");
        };

        let restarted = GatewayEventForwarder::with_spool_config(config);

        assert!(restarted.next_enqueue_seq() > existing.enqueue_seq);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn pending_spool_file_fences_later_critical_output_for_target() {
        let dir = std::env::temp_dir().join(format!(
            "vpsman-gateway-spool-fence-{}",
            uuid::Uuid::new_v4()
        ));
        let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
            dir.clone(),
            1024 * 1024,
            8 * 1024 * 1024,
            30,
        ));
        let first = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
        let GatewayForwardQueueItem::Spooled {
            path: first_path, ..
        } = forwarder
            .spool
            .spool_event("client-a", &first)
            .await
            .unwrap()
        else {
            panic!("spool_event must return a spooled item");
        };
        let second = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":2}"#);

        forwarder
            .enqueue("client-a".to_string(), second, test_timeouts())
            .await
            .unwrap();

        let replay = forwarder.spool.pending_items().await;
        assert_eq!(replay.len(), 2);
        assert_eq!(
            forwarder
                .metrics
                .current_queue_depth
                .load(Ordering::Relaxed),
            0
        );
        if let GatewayForwardQueueItem::Spooled {
            path, disk_bytes, ..
        } = &replay[0].1
        {
            forwarder.spool.remove_spooled_file(path, *disk_bytes).await;
        }
        if let GatewayForwardQueueItem::Spooled {
            path, disk_bytes, ..
        } = &replay[1].1
        {
            forwarder.spool.remove_spooled_file(path, *disk_bytes).await;
        }
        let _ = std::fs::remove_file(first_path);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn pending_spool_items_quarantine_corrupt_entries() {
        let dir = std::env::temp_dir().join(format!(
            "vpsman-gateway-spool-corrupt-{}",
            uuid::Uuid::new_v4()
        ));
        let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
            dir.clone(),
            1024 * 1024,
            8 * 1024 * 1024,
            30,
        ));
        let pending_dir = dir.join("pending");
        std::fs::create_dir_all(&pending_dir).unwrap();
        let target_hex = hex::encode("client-a".as_bytes());
        let corrupt_path = pending_dir.join(format!(
            "{}-{target_hex}-{}.spool",
            unix_now(),
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&corrupt_path, b"not-a-valid-spool-file").unwrap();

        let replay = forwarder.spool.pending_items().await;

        assert!(replay.is_empty());
        assert!(!corrupt_path.exists());
        let quarantined_path = dir
            .join("corrupt")
            .read_dir()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("corrupt")), 0o700);
        assert_eq!(mode(&quarantined_path), 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn pending_spool_file_count(dir: &Path) -> usize {
        std::fs::read_dir(dir.join("pending"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .path()
                            .extension()
                            .and_then(|extension| extension.to_str())
                            == Some("spool")
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn test_timeouts() -> Arc<StdRwLock<GatewayHttpTimeouts>> {
        Arc::new(StdRwLock::new(GatewayHttpTimeouts::default()))
    }
}
