use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
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
    sync::{mpsc, Mutex, Notify, OwnedSemaphorePermit, Semaphore},
    task::{Id, JoinSet},
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

type CriticalForwardingFailureFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
type CriticalForwardingFailureHandler =
    Arc<dyn Fn(String, &'static str) -> CriticalForwardingFailureFuture + Send + Sync + 'static>;
type GatewaySessionRejectionFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
type GatewaySessionRejectionHandler =
    Arc<dyn Fn(String, uuid::Uuid) -> GatewaySessionRejectionFuture + Send + Sync + 'static>;
type TelemetryRouteRefreshFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
type TelemetryRouteRefreshHandler =
    Arc<dyn Fn(String, uuid::Uuid) -> TelemetryRouteRefreshFuture + Send + Sync + 'static>;
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
        telemetry_in_flight: usize,
    ) -> Self {
        let timeouts = Arc::new(StdRwLock::new(timeouts));
        let forwarder = Arc::new(GatewayEventForwarder::with_config(
            spool_config,
            forward_config,
            telemetry_in_flight,
        ));
        let client = Self {
            api_url: api_url.map(|url| url.trim_end_matches('/').to_string()),
            internal_token: internal_token
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty()),
            forwarder,
            timeouts,
        };
        client
    }

    pub(crate) fn start_forward_consumers(&self) -> tokio::task::JoinHandle<()> {
        self.forwarder
            .start_forward_consumers(self.timeouts.clone())
    }

    pub(crate) fn forward_metrics(&self) -> Arc<GatewayForwardMetrics> {
        self.forwarder.metrics.clone()
    }

    pub(crate) fn set_critical_failure_handler<F, Fut>(&self, handler: F)
    where
        F: Fn(String, &'static str) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if let Ok(mut slot) = self.forwarder.critical_failure_handler.write() {
            *slot = Some(Arc::new(move |client_id, reason| {
                Box::pin(handler(client_id, reason))
            }));
        }
    }

    pub(crate) fn set_session_rejection_handler<F, Fut>(&self, handler: F)
    where
        F: Fn(String, uuid::Uuid) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if let Ok(mut slot) = self.forwarder.session_rejection_handler.write() {
            *slot = Some(Arc::new(move |client_id, gateway_session_id| {
                Box::pin(handler(client_id, gateway_session_id))
            }));
        }
    }

    pub(crate) fn set_telemetry_route_refresh_handler<F, Fut>(&self, handler: F)
    where
        F: Fn(String, uuid::Uuid) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if let Ok(mut slot) = self.forwarder.telemetry_route_refresh_handler.write() {
            *slot = Some(Arc::new(move |client_id, gateway_session_id| {
                Box::pin(handler(client_id, gateway_session_id))
            }));
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
        self.post_with_session_fence(target_key, path, value, None)
            .await
    }

    pub(crate) async fn post_for_session<T: serde::Serialize>(
        &self,
        target_key: &str,
        gateway_session_id: uuid::Uuid,
        path: &str,
        value: &T,
    ) -> Result<()> {
        self.post_with_session_fence(target_key, path, value, Some(gateway_session_id))
            .await
    }

    async fn post_with_session_fence<T: serde::Serialize>(
        &self,
        target_key: &str,
        path: &str,
        value: &T,
        gateway_session_id: Option<uuid::Uuid>,
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
                    gateway_session_id,
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
                    gateway_session_id: None,
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
    queues: Arc<Mutex<HashMap<String, GatewayForwardQueue>>>,
    telemetry_pending: Arc<Mutex<GatewayTelemetryPending>>,
    telemetry_http_owners: Arc<Semaphore>,
    critical_failure_handler: Arc<StdRwLock<Option<CriticalForwardingFailureHandler>>>,
    session_rejection_handler: Arc<StdRwLock<Option<GatewaySessionRejectionHandler>>>,
    telemetry_route_refresh_handler: Arc<StdRwLock<Option<TelemetryRouteRefreshHandler>>>,
    metrics: Arc<GatewayForwardMetrics>,
    spool: Arc<GatewayForwardSpool>,
    runtime_config: Arc<GatewayForwardRuntimeConfig>,
    enqueue_seq: AtomicU64,
    queue_owner_seq: AtomicU64,
    telemetry_drain_seq: AtomicU64,
    consumer_health: Arc<GatewayConsumerHealth>,
    consumer_commands: mpsc::UnboundedSender<GatewayConsumerCommand>,
    consumer_command_rx: StdMutex<Option<mpsc::UnboundedReceiver<GatewayConsumerCommand>>>,
    accepting: AtomicBool,
}

#[derive(Default)]
struct GatewayConsumerHealth {
    failed: AtomicBool,
    changed: Notify,
}

impl GatewayConsumerHealth {
    fn fail(&self) {
        if !self.failed.swap(true, Ordering::AcqRel) {
            self.changed.notify_waiters();
        }
    }

    fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    async fn failed(&self) {
        if self.is_failed() {
            return;
        }
        let changed = self.changed.notified();
        if self.is_failed() {
            return;
        }
        changed.await;
    }
}

#[derive(Default)]
struct GatewayTelemetryPending {
    // A target in `draining_targets` owns exactly one supervisor-joined drain
    // task (or its one queued launch token). Further samples replace its single
    // event slot instead of allocating more queue tokens or mutex waiters.
    events: HashMap<String, GatewayForwardEvent>,
    draining_targets: HashMap<String, GatewayTelemetryDrainOwner>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GatewayTelemetryDrainOwner {
    token: u64,
    phase: GatewayTelemetryDrainPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GatewayTelemetryDrainPhase {
    Queued,
    Running,
}

#[derive(Clone)]
struct GatewayTelemetryDrainContext {
    telemetry_pending: Arc<Mutex<GatewayTelemetryPending>>,
    telemetry_http_owners: Arc<Semaphore>,
    metrics: Arc<GatewayForwardMetrics>,
    critical_failure_handler: Arc<StdRwLock<Option<CriticalForwardingFailureHandler>>>,
    session_rejection_handler: Arc<StdRwLock<Option<GatewaySessionRejectionHandler>>>,
    telemetry_route_refresh_handler: Arc<StdRwLock<Option<TelemetryRouteRefreshHandler>>>,
    spool: Arc<GatewayForwardSpool>,
    runtime_config: Arc<GatewayForwardRuntimeConfig>,
    timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
    consumer_health: Arc<GatewayConsumerHealth>,
    consumer_commands: mpsc::UnboundedSender<GatewayConsumerCommand>,
}

enum GatewayConsumerCommand {
    StartForwardQueue {
        target_key: String,
        owner_token: u64,
        receiver: mpsc::Receiver<GatewayForwardQueueItem>,
        queues: Arc<Mutex<HashMap<String, GatewayForwardQueue>>>,
        context: GatewayTelemetryDrainContext,
    },
    StartTelemetryDrain {
        target_key: String,
        drain_token: u64,
        context: GatewayTelemetryDrainContext,
    },
}

#[derive(Clone, Debug)]
enum GatewayConsumerIdentity {
    SpoolReplay,
    ForwardQueue {
        target_key: String,
        owner_token: u64,
    },
    TelemetryDrain {
        target_key: String,
        drain_token: u64,
    },
}

struct GatewayForwardQueue {
    sender: mpsc::Sender<GatewayForwardQueueItem>,
    last_enqueue_unix: u64,
    owner_token: u64,
}

struct GatewayForwardSpool {
    config: GatewaySpoolConfig,
    ram_bytes: AtomicU64,
    disk_bytes: AtomicU64,
    accounted_spool_files: StdMutex<HashMap<PathBuf, u64>>,
    replay_owners: Arc<StdMutex<HashSet<PathBuf>>>,
    replay_blocked_targets: Arc<StdMutex<HashSet<String>>>,
    replay_ready: Arc<Notify>,
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
    telemetry_admission_limit: AtomicU64,
    telemetry_admission_active: AtomicU64,
    telemetry_admission_waiting: AtomicU64,
    unhealthy: AtomicBool,
}

struct GatewayTelemetryHttpOwner {
    _permit: OwnedSemaphorePermit,
    metrics: Arc<GatewayForwardMetrics>,
}

struct GatewayTelemetryHttpAdmission {
    owners: Arc<Semaphore>,
    metrics: Arc<GatewayForwardMetrics>,
    initial_owner: Option<GatewayTelemetryHttpOwner>,
}

enum GatewayTelemetryHttpOwnerWait {
    Acquired(GatewayTelemetryHttpOwner),
    Shutdown,
    Expired,
}

enum GatewayTelemetryRetryWait {
    Ready,
    Shutdown,
    Expired,
}

struct GatewayTelemetryHttpWaiter {
    metrics: Arc<GatewayForwardMetrics>,
}

impl Drop for GatewayTelemetryHttpOwner {
    fn drop(&mut self) {
        self.metrics
            .telemetry_admission_active
            .fetch_sub(1, Ordering::Relaxed);
    }
}

impl Drop for GatewayTelemetryHttpWaiter {
    fn drop(&mut self) {
        self.metrics
            .telemetry_admission_waiting
            .fetch_sub(1, Ordering::Relaxed);
    }
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
    gateway_session_id: Option<uuid::Uuid>,
    created_at: time::Instant,
    created_unix: u64,
    enqueue_seq: u64,
}

#[derive(Debug)]
struct GatewayHttpStatusError {
    status: String,
    status_code: u16,
    body: String,
}

impl fmt::Display for GatewayHttpStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "API returned {}: {}",
            self.status,
            self.body.trim()
        )
    }
}

impl std::error::Error for GatewayHttpStatusError {}

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
        _replay_owner: Option<GatewaySpoolReplayOwner>,
    },
    Telemetry {
        created_unix: u64,
        drain_token: u64,
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
    _replay_owner: Option<GatewaySpoolReplayOwner>,
}

#[derive(Debug)]
struct GatewaySpoolReplayOwner {
    path: PathBuf,
    target_key: String,
    owners: Arc<StdMutex<HashSet<PathBuf>>>,
    blocked_targets: Arc<StdMutex<HashSet<String>>>,
    ready: Arc<Notify>,
    wake_on_drop: bool,
}

impl Drop for GatewaySpoolReplayOwner {
    fn drop(&mut self) {
        let released = self
            .owners
            .lock()
            .map(|mut owners| owners.remove(&self.path))
            .unwrap_or(false);
        if released && self.wake_on_drop {
            let was_blocked = self
                .blocked_targets
                .lock()
                .map(|mut targets| targets.remove(&self.target_key))
                .unwrap_or(false);
            if was_blocked {
                self.ready.notify_one();
            }
        }
    }
}

impl GatewaySpoolReplayOwner {
    fn defer_until_target_space(&mut self) {
        self.wake_on_drop = false;
    }
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
pub(crate) const DEFAULT_TELEMETRY_IN_FLIGHT: usize = 8;
const MAX_TELEMETRY_IN_FLIGHT: usize = 512;
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
            DEFAULT_TELEMETRY_IN_FLIGHT,
        )
    }
}

impl GatewayEventForwarder {
    #[cfg(test)]
    fn with_spool_config(spool_config: GatewaySpoolConfig) -> Self {
        Self::with_config(
            spool_config,
            GatewayForwardConfig::default(),
            DEFAULT_TELEMETRY_IN_FLIGHT,
        )
    }

    fn with_config(
        spool_config: GatewaySpoolConfig,
        forward_config: GatewayForwardConfig,
        telemetry_in_flight: usize,
    ) -> Self {
        let spool = Arc::new(GatewayForwardSpool::new(spool_config));
        let enqueue_seq = initial_gateway_enqueue_seq_for_spool(&spool);
        let telemetry_in_flight = telemetry_in_flight.clamp(1, MAX_TELEMETRY_IN_FLIGHT);
        let metrics = Arc::new(GatewayForwardMetrics::default());
        metrics
            .telemetry_admission_limit
            .store(telemetry_in_flight as u64, Ordering::Relaxed);
        let (consumer_commands, consumer_command_rx) = mpsc::unbounded_channel();
        Self {
            queues: Arc::default(),
            telemetry_pending: Arc::default(),
            telemetry_http_owners: Arc::new(Semaphore::new(telemetry_in_flight)),
            critical_failure_handler: Arc::default(),
            session_rejection_handler: Arc::default(),
            telemetry_route_refresh_handler: Arc::default(),
            metrics,
            spool,
            runtime_config: Arc::new(GatewayForwardRuntimeConfig::new(forward_config)),
            enqueue_seq: AtomicU64::new(enqueue_seq),
            queue_owner_seq: AtomicU64::new(0),
            telemetry_drain_seq: AtomicU64::new(0),
            consumer_health: Arc::default(),
            consumer_commands,
            consumer_command_rx: StdMutex::new(Some(consumer_command_rx)),
            accepting: AtomicBool::new(true),
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

    fn start_forward_consumers(
        self: &Arc<Self>,
        timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
    ) -> tokio::task::JoinHandle<()> {
        let forwarder = self.clone();
        let command_rx = self
            .consumer_command_rx
            .lock()
            .expect("gateway forward consumer receiver mutex is available")
            .take()
            .expect("gateway forward consumers are started exactly once");
        tokio::spawn(run_gateway_forward_consumers(
            forwarder, timeouts, command_rx,
        ))
    }

    async fn replay_pending_spool_once(
        &self,
        timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
    ) -> bool {
        let mut blocked_targets = self.spool.replay_blocked_targets();
        let items = self.spool.pending_items_excluding(&blocked_targets).await;
        let mut accepted = false;
        for (target_key, mut item) in items {
            if blocked_targets.contains(&target_key) {
                item.defer_spool_replay_until_target_space();
                continue;
            }
            self.spool.block_replay_target(&target_key);
            match self
                .enqueue_queue_item(target_key.clone(), item, timeouts.clone())
                .await
            {
                Ok(()) => {
                    self.spool.clear_replay_target_block(&target_key);
                    accepted = true;
                }
                Err(error) => {
                    // Once an earlier durable event cannot enter this target's
                    // queue, later sequence values must not overtake it. Other
                    // targets remain independently eligible in this same scan.
                    blocked_targets.insert(target_key.clone());
                    warn!(
                        %error,
                        target_key,
                        "failed to enqueue spooled gateway event for replay"
                    );
                }
            }
        }
        accepted
    }

    async fn shutdown_flush(&self, timeout: Duration) {
        // Listener and connection owners are drained before this is called, so
        // closing admission cannot reject a legitimate producer. Keep replay
        // and every accepted exact-target consumer alive for the flush window.
        self.accepting.store(false, Ordering::Release);
        let deadline = time::Instant::now() + timeout;
        while (self.metrics.current_queue_depth.load(Ordering::Relaxed) > 0
            || self.spool.disk_bytes.load(Ordering::Relaxed) > 0)
            && time::Instant::now() < deadline
            && !self.consumer_health.is_failed()
        {
            sleep(Duration::from_millis(100)).await;
        }
        self.spool.request_shutdown();
    }

    async fn enqueue(
        &self,
        target_key: String,
        event: GatewayForwardEvent,
        timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
    ) -> Result<()> {
        if !self.accepting.load(Ordering::Acquire) {
            anyhow::bail!("gateway_forwarder_shutdown");
        }
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
        if let Some(previous) = pending.events.insert(target_key.clone(), event) {
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
        if pending.draining_targets.contains_key(&target_key) {
            drop(pending);
            self.record_telemetry_pending_without_queue_token(created_unix);
            return Ok(());
        }
        let drain_token = self
            .telemetry_drain_seq
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        pending.draining_targets.insert(
            target_key.clone(),
            GatewayTelemetryDrainOwner {
                token: drain_token,
                phase: GatewayTelemetryDrainPhase::Queued,
            },
        );
        drop(pending);

        if let Err(error) = self
            .enqueue_queue_item(
                target_key.clone(),
                GatewayForwardQueueItem::Telemetry {
                    created_unix,
                    drain_token,
                },
                timeouts,
            )
            .await
        {
            let removed = {
                let mut pending = self.telemetry_pending.lock().await;
                if telemetry_drain_owner_is(pending.draining_targets.get(&target_key), drain_token)
                {
                    pending.draining_targets.remove(&target_key);
                }
                pending.events.remove(&target_key)
            };
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

    fn record_telemetry_pending_without_queue_token(&self, created_unix: u64) {
        let previous_depth = self
            .metrics
            .current_queue_depth
            .fetch_add(1, Ordering::Relaxed);
        if previous_depth == 0 {
            self.metrics
                .oldest_event_unix
                .store(created_unix, Ordering::Relaxed);
        }
        self.metrics.queued_events.fetch_add(1, Ordering::Relaxed);
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
        let enqueue_unix = unix_now();
        let (sender, queue_owner_token) = {
            let mut queues = self.queues.lock().await;
            if !queues.contains_key(&target_key) {
                let (sender, receiver) = mpsc::channel(PER_TARGET_QUEUE_CAPACITY);
                let owner_token = self
                    .queue_owner_seq
                    .fetch_add(1, Ordering::Relaxed)
                    .saturating_add(1);
                let queues_owner = self.queues.clone();
                let metrics = self.metrics.clone();
                let telemetry_pending = self.telemetry_pending.clone();
                let telemetry_http_owners = self.telemetry_http_owners.clone();
                let critical_failure_handler = self.critical_failure_handler.clone();
                let session_rejection_handler = self.session_rejection_handler.clone();
                let telemetry_route_refresh_handler = self.telemetry_route_refresh_handler.clone();
                let spool = self.spool.clone();
                let runtime_config = self.runtime_config.clone();
                let telemetry_drain_context = GatewayTelemetryDrainContext {
                    telemetry_pending,
                    telemetry_http_owners,
                    metrics: metrics.clone(),
                    critical_failure_handler,
                    session_rejection_handler,
                    telemetry_route_refresh_handler,
                    spool: spool.clone(),
                    runtime_config,
                    timeouts,
                    consumer_health: self.consumer_health.clone(),
                    consumer_commands: self.consumer_commands.clone(),
                };
                queues.insert(
                    target_key.clone(),
                    GatewayForwardQueue {
                        sender: sender.clone(),
                        last_enqueue_unix: enqueue_unix,
                        owner_token,
                    },
                );
                if self
                    .consumer_commands
                    .send(GatewayConsumerCommand::StartForwardQueue {
                        target_key: target_key.clone(),
                        owner_token,
                        receiver,
                        queues: queues_owner,
                        context: telemetry_drain_context,
                    })
                    .is_err()
                {
                    queues.remove(&target_key);
                    self.consumer_health.fail();
                    anyhow::bail!("gateway_forward_consumer_unavailable");
                }
                self.metrics.active_queues.fetch_add(1, Ordering::Relaxed);
            }
            let queue = queues
                .get_mut(&target_key)
                .expect("queue sender exists after creation");
            queue.last_enqueue_unix = enqueue_unix;
            (queue.sender.clone(), queue.owner_token)
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
            Err(error) => {
                let queue_closed = matches!(&error, mpsc::error::TrySendError::Closed(_));
                let item = error.into_inner();
                if queue_closed {
                    remove_forward_queue_owner(&self.queues, &target_key, queue_owner_token).await;
                }
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
                        _replay_owner: mut replay_owner,
                        ..
                    } => {
                        if !queue_closed {
                            if let Some(owner) = replay_owner.as_mut() {
                                owner.defer_until_target_space();
                            }
                        }
                        self.metrics
                            .record_drop(kind, GatewayForwardDropReason::TargetQueueFull);
                        if critical {
                            self.record_critical_failure(GatewayForwardDropReason::TargetQueueFull);
                            self.notify_critical_failure(
                                &target_key,
                                GatewayForwardDropReason::TargetQueueFull,
                            )
                            .await;
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

    async fn drop_enqueue_event(
        &self,
        target_key: &str,
        event: GatewayForwardEvent,
        reason: GatewayForwardDropReason,
    ) -> Result<()> {
        self.record_drop(&event, reason);
        if event.critical {
            self.record_critical_failure(reason);
            self.notify_critical_failure(target_key, reason).await;
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

    async fn notify_critical_failure(&self, target_key: &str, reason: GatewayForwardDropReason) {
        let handler = self
            .critical_failure_handler
            .read()
            .ok()
            .and_then(|slot| slot.as_ref().cloned());
        if let Some(handler) = handler {
            handler(target_key.to_string(), reason.as_str()).await;
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
            replay_owners: Arc::new(StdMutex::new(HashSet::new())),
            replay_blocked_targets: Arc::new(StdMutex::new(HashSet::new())),
            replay_ready: Arc::new(Notify::new()),
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
        // New durable work wakes the consumer, but it does not override an
        // exact target that is waiting for queue space.
        self.replay_ready.notify_one();
        Ok(GatewayForwardQueueItem::Spooled {
            path: final_path,
            created_unix: event.created_unix,
            enqueue_seq: event.enqueue_seq,
            disk_bytes,
            kind: event.kind,
            critical: event.critical,
            _replay_owner: None,
        })
    }

    #[cfg(test)]
    async fn pending_items(&self) -> Vec<(String, GatewayForwardQueueItem)> {
        self.pending_items_excluding(&HashSet::new()).await
    }

    async fn pending_items_excluding(
        &self,
        blocked_targets: &HashSet<String>,
    ) -> Vec<(String, GatewayForwardQueueItem)> {
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
            if blocked_targets.contains(&target_key) {
                continue;
            }
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
            let Some(replay_owner) =
                self.claim_replay_owner(&candidate.path, &candidate.target_key)
            else {
                continue;
            };
            items.push((
                candidate.target_key,
                GatewayForwardQueueItem::Spooled {
                    path: candidate.path,
                    created_unix: candidate.created_unix,
                    enqueue_seq: candidate.enqueue_seq,
                    disk_bytes: candidate.disk_bytes,
                    kind: candidate.kind,
                    critical: candidate.critical,
                    _replay_owner: Some(replay_owner),
                },
            ));
        }
        items
    }

    fn claim_replay_owner(&self, path: &Path, target_key: &str) -> Option<GatewaySpoolReplayOwner> {
        let mut owners = self.replay_owners.lock().ok()?;
        if !owners.insert(path.to_path_buf()) {
            return None;
        }
        Some(GatewaySpoolReplayOwner {
            path: path.to_path_buf(),
            target_key: target_key.to_string(),
            owners: self.replay_owners.clone(),
            blocked_targets: self.replay_blocked_targets.clone(),
            ready: self.replay_ready.clone(),
            wake_on_drop: true,
        })
    }

    fn mark_replay_target_ready(&self, target_key: &str) {
        let was_blocked = self
            .replay_blocked_targets
            .lock()
            .map(|mut targets| targets.remove(target_key))
            .unwrap_or(false);
        if was_blocked {
            self.replay_ready.notify_one();
        }
    }

    fn replay_blocked_targets(&self) -> HashSet<String> {
        self.replay_blocked_targets
            .lock()
            .map(|targets| targets.clone())
            .unwrap_or_default()
    }

    fn block_replay_target(&self, target_key: &str) {
        if let Ok(mut targets) = self.replay_blocked_targets.lock() {
            targets.insert(target_key.to_string());
        }
    }

    fn clear_replay_target_block(&self, target_key: &str) {
        if let Ok(mut targets) = self.replay_blocked_targets.lock() {
            targets.remove(target_key);
        }
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
            telemetry_admission_limit: self.telemetry_admission_limit.load(Ordering::Relaxed),
            telemetry_admission_active: self.telemetry_admission_active.load(Ordering::Relaxed),
            telemetry_admission_waiting: self.telemetry_admission_waiting.load(Ordering::Relaxed),
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

async fn run_gateway_forward_consumers(
    forwarder: Arc<GatewayEventForwarder>,
    timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
    mut commands: mpsc::UnboundedReceiver<GatewayConsumerCommand>,
) {
    let mut consumers = JoinSet::new();
    let mut identities = HashMap::<Id, GatewayConsumerIdentity>::new();
    spawn_gateway_consumer(
        &mut consumers,
        &mut identities,
        GatewayConsumerCommandOrReplay::SpoolReplay {
            forwarder: forwarder.clone(),
            timeouts,
        },
    );

    loop {
        tokio::select! {
            biased;
            _ = forwarder.spool.notified_shutdown() => break,
            _ = forwarder.consumer_health.failed() => break,
            command = commands.recv() => {
                let Some(command) = command else {
                    forwarder.consumer_health.fail();
                    break;
                };
                spawn_gateway_consumer(
                    &mut consumers,
                    &mut identities,
                    GatewayConsumerCommandOrReplay::Command(command),
                );
            }
            completed = consumers.join_next_with_id(), if !consumers.is_empty() => {
                if observe_gateway_consumer_completion(
                    completed,
                    &mut identities,
                    &forwarder,
                    false,
                ).await {
                    break;
                }
            }
        }
    }

    commands.close();
    forwarder.accepting.store(false, Ordering::Release);
    forwarder.spool.request_shutdown();
    // Dropping the exact queue senders lets each queue owner consume work that
    // was already accepted and then finish. A queue/drain may still submit its
    // exact child while owners are draining, so consume those commands before
    // deciding that the owned task set is empty.
    forwarder.queues.lock().await.clear();
    loop {
        while let Ok(command) = commands.try_recv() {
            spawn_gateway_consumer(
                &mut consumers,
                &mut identities,
                GatewayConsumerCommandOrReplay::Command(command),
            );
        }
        if consumers.is_empty() {
            break;
        }
        let completed = consumers.join_next_with_id().await;
        observe_gateway_consumer_completion(completed, &mut identities, &forwarder, true).await;
    }
}

enum GatewayConsumerCommandOrReplay {
    Command(GatewayConsumerCommand),
    SpoolReplay {
        forwarder: Arc<GatewayEventForwarder>,
        timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
    },
}

fn spawn_gateway_consumer(
    consumers: &mut JoinSet<GatewayConsumerIdentity>,
    identities: &mut HashMap<Id, GatewayConsumerIdentity>,
    command: GatewayConsumerCommandOrReplay,
) {
    let (identity, abort_handle) = match command {
        GatewayConsumerCommandOrReplay::SpoolReplay {
            forwarder,
            timeouts,
        } => {
            let identity = GatewayConsumerIdentity::SpoolReplay;
            let completed_identity = identity.clone();
            let abort_handle = consumers.spawn(async move {
                run_gateway_spool_replay(forwarder, timeouts).await;
                completed_identity
            });
            (identity, abort_handle)
        }
        GatewayConsumerCommandOrReplay::Command(GatewayConsumerCommand::StartForwardQueue {
            target_key,
            owner_token,
            receiver,
            queues,
            context,
        }) => {
            let identity = GatewayConsumerIdentity::ForwardQueue {
                target_key: target_key.clone(),
                owner_token,
            };
            let completed_identity = identity.clone();
            let abort_handle = consumers.spawn(async move {
                run_forward_queue(target_key, owner_token, receiver, queues, context).await;
                completed_identity
            });
            (identity, abort_handle)
        }
        GatewayConsumerCommandOrReplay::Command(GatewayConsumerCommand::StartTelemetryDrain {
            target_key,
            drain_token,
            context,
        }) => {
            let identity = GatewayConsumerIdentity::TelemetryDrain {
                target_key: target_key.clone(),
                drain_token,
            };
            let completed_identity = identity.clone();
            let abort_handle = consumers.spawn(async move {
                run_telemetry_drain(target_key, drain_token, context).await;
                completed_identity
            });
            (identity, abort_handle)
        }
    };
    identities.insert(abort_handle.id(), identity);
}

async fn observe_gateway_consumer_completion(
    completed: Option<std::result::Result<(Id, GatewayConsumerIdentity), tokio::task::JoinError>>,
    identities: &mut HashMap<Id, GatewayConsumerIdentity>,
    forwarder: &GatewayEventForwarder,
    shutting_down: bool,
) -> bool {
    let Some(completed) = completed else {
        return !shutting_down;
    };
    let (identity, failure) = match completed {
        Ok((id, identity)) => {
            identities.remove(&id);
            (identity, None)
        }
        Err(error) => {
            let identity = identities.remove(&error.id());
            (
                identity.unwrap_or(GatewayConsumerIdentity::SpoolReplay),
                Some(error),
            )
        }
    };
    cleanup_gateway_consumer_identity(forwarder, &identity).await;

    if let Some(error) = failure {
        forwarder.consumer_health.fail();
        warn!(%error, ?identity, "gateway owned forwarding consumer failed");
        return !shutting_down;
    }
    if matches!(identity, GatewayConsumerIdentity::SpoolReplay) && !shutting_down {
        forwarder.consumer_health.fail();
        warn!("gateway spool replay consumer stopped unexpectedly");
        return true;
    }
    false
}

async fn cleanup_gateway_consumer_identity(
    forwarder: &GatewayEventForwarder,
    identity: &GatewayConsumerIdentity,
) {
    match identity {
        GatewayConsumerIdentity::SpoolReplay => {}
        GatewayConsumerIdentity::ForwardQueue {
            target_key,
            owner_token,
        } => {
            remove_forward_queue_owner(&forwarder.queues, target_key, *owner_token).await;
            forwarder
                .metrics
                .active_queues
                .fetch_sub(1, Ordering::Relaxed);
        }
        GatewayConsumerIdentity::TelemetryDrain {
            target_key,
            drain_token,
        } => {
            remove_telemetry_drain_owner(&forwarder.telemetry_pending, target_key, *drain_token)
                .await;
        }
    }
}

async fn run_gateway_spool_replay(
    forwarder: Arc<GatewayEventForwarder>,
    timeouts: Arc<StdRwLock<GatewayHttpTimeouts>>,
) {
    if !forwarder.spool.config.enabled {
        tokio::select! {
            _ = forwarder.spool.notified_shutdown() => {}
            _ = forwarder.consumer_health.failed() => {}
        }
        return;
    }
    loop {
        if forwarder.consumer_health.is_failed() {
            return;
        }
        // Register before scanning so a newly durable file or a released exact
        // replay owner cannot be lost between the scan and wait.
        let replay_ready = forwarder.spool.replay_ready.notified();
        let accepted = forwarder.replay_pending_spool_once(timeouts.clone()).await;
        if forwarder.spool.shutdown_requested() {
            break;
        }
        if accepted {
            tokio::task::yield_now().await;
            continue;
        }
        tokio::select! {
            _ = replay_ready => {}
            _ = forwarder.spool.notified_shutdown() => break,
            _ = forwarder.consumer_health.failed() => return,
        }
    }
}

async fn run_forward_queue(
    target_key: String,
    owner_token: u64,
    mut receiver: mpsc::Receiver<GatewayForwardQueueItem>,
    queues: Arc<Mutex<HashMap<String, GatewayForwardQueue>>>,
    context: GatewayTelemetryDrainContext,
) {
    loop {
        let Some(idle_wait) = forward_queue_idle_wait(&queues, &target_key, owner_token).await
        else {
            break;
        };
        let item = tokio::select! {
            biased;
            item = receiver.recv() => item,
            _ = sleep(idle_wait) => {
                if retire_forward_queue_if_idle(
                    &queues,
                    &target_key,
                    owner_token,
                    unix_now(),
                )
                .await
                {
                    break;
                }
                continue;
            }
        };
        let Some(item) = item else {
            break;
        };
        context.spool.mark_replay_target_ready(&target_key);
        if let GatewayForwardQueueItem::Telemetry { drain_token, .. } = &item {
            if mark_telemetry_drain_running(&context.telemetry_pending, &target_key, *drain_token)
                .await
            {
                if context
                    .consumer_commands
                    .send(GatewayConsumerCommand::StartTelemetryDrain {
                        target_key: target_key.clone(),
                        drain_token: *drain_token,
                        context: context.clone(),
                    })
                    .is_err()
                {
                    context.consumer_health.fail();
                    let event = {
                        let mut pending = context.telemetry_pending.lock().await;
                        if telemetry_drain_owner_is(
                            pending.draining_targets.get(&target_key),
                            *drain_token,
                        ) {
                            pending.draining_targets.remove(&target_key);
                            pending.events.remove(&target_key)
                        } else {
                            None
                        }
                    };
                    if let Some(event) = event {
                        warn!(
                            path = %event.path,
                            target_key,
                            drain_token,
                            "discarded telemetry because its owned consumer is unavailable"
                        );
                    }
                    finish_forward_event(&context.metrics, &context.spool, None, false).await;
                }
            } else {
                finish_forward_event(&context.metrics, &context.spool, None, false).await;
            }
            continue;
        }
        let Some(handle) = queue_item_event(
            item,
            &target_key,
            &context.telemetry_pending,
            &context.metrics,
            &context.critical_failure_handler,
            &context.spool,
        )
        .await
        else {
            finish_forward_event(&context.metrics, &context.spool, None, false).await;
            continue;
        };
        forward_event_handle(
            &target_key,
            handle,
            &context.metrics,
            &context.critical_failure_handler,
            &context.session_rejection_handler,
            &context.telemetry_route_refresh_handler,
            &context.spool,
            &context.runtime_config,
            &context.timeouts,
            None,
        )
        .await;
    }
}

async fn forward_queue_idle_wait(
    queues: &Mutex<HashMap<String, GatewayForwardQueue>>,
    target_key: &str,
    owner_token: u64,
) -> Option<Duration> {
    let queues = queues.lock().await;
    let queue = queues
        .get(target_key)
        .filter(|queue| queue.owner_token == owner_token)?;
    let idle_at = queue.last_enqueue_unix.saturating_add(QUEUE_IDLE_REAP_SECS);
    Some(Duration::from_secs(
        idle_at.saturating_sub(unix_now()).max(1),
    ))
}

async fn retire_forward_queue_if_idle(
    queues: &Mutex<HashMap<String, GatewayForwardQueue>>,
    target_key: &str,
    owner_token: u64,
    now_unix: u64,
) -> bool {
    let mut queues = queues.lock().await;
    let removable = queues.get(target_key).is_some_and(|queue| {
        queue.owner_token == owner_token
            && now_unix.saturating_sub(queue.last_enqueue_unix) >= QUEUE_IDLE_REAP_SECS
            && queue.sender.capacity() == queue.sender.max_capacity()
    });
    if removable {
        queues.remove(target_key);
    }
    removable
}

async fn remove_forward_queue_owner(
    queues: &Mutex<HashMap<String, GatewayForwardQueue>>,
    target_key: &str,
    owner_token: u64,
) -> bool {
    let mut queues = queues.lock().await;
    if !queues
        .get(target_key)
        .is_some_and(|queue| queue.owner_token == owner_token)
    {
        return false;
    }
    queues.remove(target_key);
    true
}

async fn run_telemetry_drain(
    target_key: String,
    drain_token: u64,
    context: GatewayTelemetryDrainContext,
) {
    loop {
        if !telemetry_drain_owner_is(
            context
                .telemetry_pending
                .lock()
                .await
                .draining_targets
                .get(&target_key),
            drain_token,
        ) {
            return;
        }
        // Wait before taking the coalesced slot so newer samples can continue
        // replacing it while this target waits for gateway-wide HTTP ownership.
        let initial_http_owner = tokio::select! {
            biased;
            _ = context.spool.notified_shutdown() => None,
            owner = acquire_telemetry_http_owner(
                context.telemetry_http_owners.clone(),
                context.metrics.clone(),
            ) => Some(owner),
        };
        let Some(handle) = queue_item_event(
            GatewayForwardQueueItem::Telemetry {
                created_unix: unix_now(),
                drain_token,
            },
            &target_key,
            &context.telemetry_pending,
            &context.metrics,
            &context.critical_failure_handler,
            &context.spool,
        )
        .await
        else {
            finish_forward_event(&context.metrics, &context.spool, None, false).await;
            remove_telemetry_drain_owner(&context.telemetry_pending, &target_key, drain_token)
                .await;
            return;
        };
        forward_event_handle(
            &target_key,
            handle,
            &context.metrics,
            &context.critical_failure_handler,
            &context.session_rejection_handler,
            &context.telemetry_route_refresh_handler,
            &context.spool,
            &context.runtime_config,
            &context.timeouts,
            initial_http_owner.map(|owner| GatewayTelemetryHttpAdmission {
                owners: context.telemetry_http_owners.clone(),
                metrics: context.metrics.clone(),
                initial_owner: Some(owner),
            }),
        )
        .await;

        let mut pending = context.telemetry_pending.lock().await;
        if !telemetry_drain_owner_is(pending.draining_targets.get(&target_key), drain_token) {
            return;
        }
        if pending.events.contains_key(&target_key) {
            continue;
        }
        pending.draining_targets.remove(&target_key);
        return;
    }
}

async fn acquire_telemetry_http_owner(
    telemetry_http_owners: Arc<Semaphore>,
    metrics: Arc<GatewayForwardMetrics>,
) -> GatewayTelemetryHttpOwner {
    metrics
        .telemetry_admission_waiting
        .fetch_add(1, Ordering::Relaxed);
    let waiting = GatewayTelemetryHttpWaiter {
        metrics: metrics.clone(),
    };
    let permit = telemetry_http_owners
        .acquire_owned()
        .await
        .expect("telemetry HTTP ownership semaphore remains open");
    drop(waiting);
    metrics
        .telemetry_admission_active
        .fetch_add(1, Ordering::Relaxed);
    GatewayTelemetryHttpOwner {
        _permit: permit,
        metrics,
    }
}

async fn remove_telemetry_drain_owner(
    telemetry_pending: &Mutex<GatewayTelemetryPending>,
    target_key: &str,
    drain_token: u64,
) -> bool {
    let mut pending = telemetry_pending.lock().await;
    if !telemetry_drain_owner_is(pending.draining_targets.get(target_key), drain_token) {
        return false;
    }
    pending.draining_targets.remove(target_key);
    true
}

fn telemetry_drain_owner_is(owner: Option<&GatewayTelemetryDrainOwner>, drain_token: u64) -> bool {
    owner.is_some_and(|owner| owner.token == drain_token)
}

async fn mark_telemetry_drain_running(
    telemetry_pending: &Mutex<GatewayTelemetryPending>,
    target_key: &str,
    drain_token: u64,
) -> bool {
    let mut pending = telemetry_pending.lock().await;
    let Some(owner) = pending.draining_targets.get_mut(target_key) else {
        return false;
    };
    if owner.token != drain_token || owner.phase != GatewayTelemetryDrainPhase::Queued {
        return false;
    }
    owner.phase = GatewayTelemetryDrainPhase::Running;
    true
}

async fn record_expired_gateway_event(
    event: &GatewayForwardEvent,
    target_key: &str,
    metrics: &GatewayForwardMetrics,
    critical_failure_handler: &StdRwLock<Option<CriticalForwardingFailureHandler>>,
) {
    metrics.record_drop(event.kind, GatewayForwardDropReason::Expired);
    if event.critical {
        metrics.record_critical_failure(GatewayForwardDropReason::Expired);
        notify_critical_failure(
            critical_failure_handler,
            target_key,
            GatewayForwardDropReason::Expired,
        )
        .await;
    }
}

async fn forward_event_handle(
    target_key: &str,
    handle: GatewayForwardEventHandle,
    metrics: &GatewayForwardMetrics,
    critical_failure_handler: &StdRwLock<Option<CriticalForwardingFailureHandler>>,
    session_rejection_handler: &StdRwLock<Option<GatewaySessionRejectionHandler>>,
    telemetry_route_refresh_handler: &StdRwLock<Option<TelemetryRouteRefreshHandler>>,
    spool: &GatewayForwardSpool,
    runtime_config: &GatewayForwardRuntimeConfig,
    timeouts: &StdRwLock<GatewayHttpTimeouts>,
    telemetry_http_admission: Option<GatewayTelemetryHttpAdmission>,
) {
    let event = &handle.event;
    if event.expired(runtime_config) {
        record_expired_gateway_event(event, target_key, metrics, critical_failure_handler).await;
        warn!(
            path = %event.path,
            kind = ?event.kind,
            target_key,
            "expired gateway event before API forwarding"
        );
        finish_forward_event(metrics, spool, Some(&handle), false).await;
        return;
    }
    let outcome = post_json_retry_until_expired(
        event,
        target_key,
        metrics,
        critical_failure_handler,
        session_rejection_handler,
        telemetry_route_refresh_handler,
        spool,
        runtime_config,
        timeouts,
        telemetry_http_admission,
    )
    .await;
    match outcome {
        GatewayForwardOutcome::Delivered => {
            metrics.delivered_events.fetch_add(1, Ordering::Relaxed);
        }
        GatewayForwardOutcome::DeferredToSpool => {}
        GatewayForwardOutcome::DeferredForShutdown => {
            if handle.spool_path.is_none() {
                if let Err(error) = spool.spool_event(target_key, event).await {
                    metrics.record_drop(event.kind, GatewayForwardDropReason::GlobalQueueFull);
                    metrics.record_critical_failure(GatewayForwardDropReason::GlobalQueueFull);
                    notify_critical_failure(
                        critical_failure_handler,
                        target_key,
                        GatewayForwardDropReason::GlobalQueueFull,
                    )
                    .await;
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
        metrics,
        spool,
        Some(&handle),
        outcome == GatewayForwardOutcome::DeferredForShutdown,
    )
    .await;
}

async fn queue_item_event(
    item: GatewayForwardQueueItem,
    target_key: &str,
    telemetry_pending: &Mutex<GatewayTelemetryPending>,
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
            _replay_owner: None,
        }),
        GatewayForwardQueueItem::Spooled {
            path,
            disk_bytes,
            kind,
            critical,
            _replay_owner: replay_owner,
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
                    _replay_owner: replay_owner,
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
                    )
                    .await;
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
            .events
            .remove(target_key)
            .map(|event| GatewayForwardEventHandle {
                event,
                ram_bytes: 0,
                spool_path: None,
                spool_bytes: 0,
                _replay_owner: None,
            }),
    }
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

async fn acquire_telemetry_retry_http_owner(
    admission: &GatewayTelemetryHttpAdmission,
    event: &GatewayForwardEvent,
    runtime_config: &GatewayForwardRuntimeConfig,
    spool: &GatewayForwardSpool,
) -> GatewayTelemetryHttpOwnerWait {
    let Some(remaining_ttl) = event.remaining_ttl(runtime_config) else {
        return GatewayTelemetryHttpOwnerWait::Expired;
    };
    tokio::select! {
        biased;
        _ = spool.notified_shutdown() => GatewayTelemetryHttpOwnerWait::Shutdown,
        _ = time::sleep(remaining_ttl) => GatewayTelemetryHttpOwnerWait::Expired,
        owner = acquire_telemetry_http_owner(
            admission.owners.clone(),
            admission.metrics.clone(),
        ) => GatewayTelemetryHttpOwnerWait::Acquired(owner),
    }
}

async fn wait_telemetry_retry_backoff(
    delay: Duration,
    event: &GatewayForwardEvent,
    runtime_config: &GatewayForwardRuntimeConfig,
    spool: &GatewayForwardSpool,
) -> GatewayTelemetryRetryWait {
    let Some(remaining_ttl) = event.remaining_ttl(runtime_config) else {
        return GatewayTelemetryRetryWait::Expired;
    };
    tokio::select! {
        biased;
        _ = spool.notified_shutdown() => GatewayTelemetryRetryWait::Shutdown,
        _ = time::sleep(remaining_ttl) => GatewayTelemetryRetryWait::Expired,
        _ = time::sleep(delay) => GatewayTelemetryRetryWait::Ready,
    }
}

async fn post_json_retry_until_expired(
    event: &GatewayForwardEvent,
    target_key: &str,
    metrics: &GatewayForwardMetrics,
    critical_failure_handler: &StdRwLock<Option<CriticalForwardingFailureHandler>>,
    session_rejection_handler: &StdRwLock<Option<GatewaySessionRejectionHandler>>,
    telemetry_route_refresh_handler: &StdRwLock<Option<TelemetryRouteRefreshHandler>>,
    spool: &GatewayForwardSpool,
    runtime_config: &GatewayForwardRuntimeConfig,
    timeouts: &StdRwLock<GatewayHttpTimeouts>,
    mut telemetry_http_admission: Option<GatewayTelemetryHttpAdmission>,
) -> GatewayForwardOutcome {
    let mut attempt = 1_u64;
    loop {
        if spool.shutdown_requested() {
            return GatewayForwardOutcome::DeferredForShutdown;
        }
        let http_owner = if let Some(admission) = telemetry_http_admission.as_mut() {
            if let Some(owner) = admission.initial_owner.take() {
                Some(owner)
            } else {
                match acquire_telemetry_retry_http_owner(admission, event, runtime_config, spool)
                    .await
                {
                    GatewayTelemetryHttpOwnerWait::Acquired(owner) => Some(owner),
                    GatewayTelemetryHttpOwnerWait::Shutdown => {
                        return GatewayForwardOutcome::DeferredForShutdown;
                    }
                    GatewayTelemetryHttpOwnerWait::Expired => {
                        record_expired_gateway_event(
                            event,
                            target_key,
                            metrics,
                            critical_failure_handler,
                        )
                        .await;
                        warn!(
                            path = %event.path,
                            kind = ?event.kind,
                            target_key,
                            attempt,
                            "gateway event forwarding expired while awaiting HTTP ownership"
                        );
                        return GatewayForwardOutcome::NotDelivered;
                    }
                }
            }
        } else {
            None
        };
        if event.expired(runtime_config) {
            drop(http_owner);
            record_expired_gateway_event(event, target_key, metrics, critical_failure_handler)
                .await;
            warn!(
                path = %event.path,
                kind = ?event.kind,
                target_key,
                attempt,
                "gateway event forwarding expired before an HTTP attempt"
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
        // Telemetry admission bounds only active socket attempts. Route
        // refresh, session callbacks, logging, and retry delay never retain a
        // gateway-wide permit.
        drop(http_owner);
        match post_result {
            Ok(body) => {
                refresh_telemetry_route_after_commit(
                    telemetry_route_refresh_handler,
                    target_key,
                    event,
                    &body,
                )
                .await;
                return GatewayForwardOutcome::Delivered;
            }
            Err(error) => {
                metrics.retry_attempts.fetch_add(1, Ordering::Relaxed);
                let session_not_active = error_is_gateway_session_not_active(&error);
                let error_message = error.to_string();
                if session_not_active {
                    notify_session_rejection(
                        session_rejection_handler,
                        target_key,
                        event.gateway_session_id,
                    )
                    .await;
                }
                if session_not_active
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
                            )
                            .await;
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
                if gateway_event_error_is_non_retryable(event, &error, session_not_active) {
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
                    record_expired_gateway_event(
                        event,
                        target_key,
                        metrics,
                        critical_failure_handler,
                    )
                    .await;
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
                let backoff = Duration::from_millis(backoff_ms.min(5_000));
                if telemetry_http_admission.is_some() {
                    match wait_telemetry_retry_backoff(backoff, event, runtime_config, spool).await
                    {
                        GatewayTelemetryRetryWait::Ready => {}
                        GatewayTelemetryRetryWait::Shutdown => {
                            return GatewayForwardOutcome::DeferredForShutdown;
                        }
                        GatewayTelemetryRetryWait::Expired => {
                            record_expired_gateway_event(
                                event,
                                target_key,
                                metrics,
                                critical_failure_handler,
                            )
                            .await;
                            warn!(
                                error = %error_message,
                                path = %event.path,
                                kind = ?event.kind,
                                target_key,
                                attempt,
                                "gateway event forwarding expired during retry backoff"
                            );
                            return GatewayForwardOutcome::NotDelivered;
                        }
                    }
                } else {
                    sleep(backoff).await;
                }
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

async fn notify_critical_failure(
    handler_slot: &StdRwLock<Option<CriticalForwardingFailureHandler>>,
    target_key: &str,
    reason: GatewayForwardDropReason,
) {
    let handler = handler_slot
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().cloned());
    if let Some(handler) = handler {
        handler(target_key.to_string(), reason.as_str()).await;
    }
}

async fn notify_session_rejection(
    handler_slot: &StdRwLock<Option<GatewaySessionRejectionHandler>>,
    target_key: &str,
    gateway_session_id: Option<uuid::Uuid>,
) {
    let Some(gateway_session_id) = gateway_session_id else {
        return;
    };
    let handler = handler_slot
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().cloned());
    if let Some(handler) = handler {
        handler(target_key.to_string(), gateway_session_id).await;
    }
}

async fn refresh_telemetry_route_after_commit(
    handler_slot: &StdRwLock<Option<TelemetryRouteRefreshHandler>>,
    target_key: &str,
    event: &GatewayForwardEvent,
    response_body: &str,
) {
    if event.kind != GatewayForwardEventKind::Telemetry {
        return;
    }
    let Some(gateway_session_id) = event.gateway_session_id else {
        return;
    };
    let Ok(response) = serde_json::from_str::<GatewayIngestResponse>(response_body) else {
        return;
    };
    if !response.accepted || !response.refresh_route {
        return;
    }
    let handler = handler_slot
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().cloned());
    if let Some(handler) = handler {
        handler(target_key.to_string(), gateway_session_id).await;
    }
}

fn gateway_event_error_is_non_retryable(
    event: &GatewayForwardEvent,
    error: &anyhow::Error,
    session_not_active: bool,
) -> bool {
    let Some(http_error) = error.downcast_ref::<GatewayHttpStatusError>() else {
        return false;
    };
    if http_error.status_code != 409 {
        return false;
    }
    if session_not_active {
        return !event_spools_under_pressure(event) || event_marked_spooled_replay(event);
    }
    event.kind == GatewayForwardEventKind::CommandOutput
        && (http_error.body.contains("job_output_sequence_conflict")
            || http_error.body.contains("job_target_not_active")
            || http_error.body.contains("job_output_payload_hash_mismatch"))
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
        let status_code = status
            .split_ascii_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or_default();
        return Err(GatewayHttpStatusError {
            status: status.to_string(),
            status_code,
            body: body.trim().to_string(),
        }
        .into());
    }
    Ok(body.trim().to_string())
}

impl GatewayForwardEventKind {
    fn for_path(path: &str) -> Self {
        match path {
            "/internal/v1/gateway/telemetry" => Self::Telemetry,
            "/internal/v1/gateway/command-output" => Self::CommandOutput,
            "/internal/v1/gateway/session-ended"
            | "/internal/v1/gateway/agent-hello"
            | "/internal/v1/gateway/runtime-config-reload" => Self::Lifecycle,
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

fn error_is_gateway_session_not_active(error: &anyhow::Error) -> bool {
    let Some(error) = error.downcast_ref::<GatewayHttpStatusError>() else {
        return false;
    };
    error.status_code == 409
        && serde_json::from_str::<serde_json::Value>(&error.body).is_ok_and(|body| {
            body.get("error").and_then(serde_json::Value::as_str)
                == Some("gateway_session_not_active")
        })
}

impl GatewayForwardEvent {
    fn expired(&self, runtime_config: &GatewayForwardRuntimeConfig) -> bool {
        self.created_at.elapsed() >= self.ttl(runtime_config)
    }

    fn remaining_ttl(&self, runtime_config: &GatewayForwardRuntimeConfig) -> Option<Duration> {
        self.ttl(runtime_config)
            .checked_sub(self.created_at.elapsed())
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
            Self::Telemetry { created_unix, .. } => *created_unix,
        }
    }

    fn enqueue_seq(&self) -> u64 {
        match self {
            Self::Event { event, .. } => event.enqueue_seq,
            Self::Spooled { enqueue_seq, .. } => *enqueue_seq,
            Self::Telemetry { .. } => 0,
        }
    }

    fn defer_spool_replay_until_target_space(&mut self) {
        if let Self::Spooled {
            _replay_owner: Some(owner),
            ..
        } = self
        {
            owner.defer_until_target_space();
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
        gateway_session_id: None,
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
    #[serde(default)]
    pub(crate) refresh_route: bool,
}

#[cfg(test)]
#[path = "tests_api_client.rs"]
mod tests;
