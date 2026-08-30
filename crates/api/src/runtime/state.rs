use std::{
    collections::{BTreeSet, HashMap},
    future::Future,
    net::SocketAddr,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering},
        Arc, Mutex, OnceLock, RwLock as StdRwLock, Weak,
    },
    time::Duration,
};

use anyhow::Result;
use axum::http::HeaderMap;
use futures_util::FutureExt;
use tokio::{
    sync::{broadcast, Mutex as AsyncMutex, Notify},
    time::Instant,
};
use tracing::warn;
use vpsman_common::{SuiteConfig, DEFAULT_MAX_JOB_TIMEOUT_SECS, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS};
use vpsman_server_core::{JOB_STATUS_QUEUED, JOB_STATUS_RUNNING, TARGET_STATUS_COMPLETED};

use crate::{
    backup_auto_artifacts::{
        backup_auto_artifact_error_is_permanent, backup_auto_artifact_request_failure_reason,
    },
    client_ip::TrustedProxyConfig,
    dashboard_telemetry_resident::DashboardTelemetryResident,
    error::ApiError,
    gateway_client::GatewayDispatchClient,
    model::{AuthContext, ClientMonitoringView, WsEvent},
    model_dashboard::{DashboardOverviewView, SystemDashboardView},
    model_fleet_snapshot::FleetSnapshotResponse,
    model_home_snapshot::HomeSnapshotResponse,
    model_monitoring::MonitoringCardsPageView,
    object_store::BackupObjectStore,
    repository::Repository,
    repository_artifact_deletions::ReviewedArtifactDeletionProducer,
    repository_jobs::{ClaimedJobTerminalEnrichment, TerminalizationBatch},
    routes_ingest::{
        record_network_routing_terminal_result, try_auto_record_backup_artifact_for_job_target,
    },
    security::{bearer_token, constant_time_eq, operator_has_scope, role_allows},
};

pub(crate) const DEFAULT_ARTIFACT_MAX_BYTES: usize = 128 * 1024 * 1024;
// One fleet-wide boundary serves both resident live-overlay collection and
// browser invalidation. Keeping the duration in one owner prevents the two
// stages from silently acquiring different freshness semantics.
pub(crate) const FLEET_TELEMETRY_INVALIDATION_WINDOW: Duration = Duration::from_secs(2);
const MIN_ARTIFACT_MAX_BYTES: usize = 1024 * 1024;
const MAX_ARTIFACT_MAX_BYTES: usize = 4 * 1024 * 1024 * 1024;
const DEFAULT_OPERATOR_AUTH_USERNAME_FAILED_ATTEMPT_LIMIT: i64 = 8;
const DEFAULT_OPERATOR_AUTH_IP_FAILED_ATTEMPT_LIMIT: i64 = 8;
const DEFAULT_OPERATOR_AUTH_FAILED_ATTEMPT_WINDOW_SECS: u64 = 15 * 60;
const DEFAULT_OPERATOR_AUTH_LOCKOUT_SECS: u64 = 15 * 60;
// Telemetry snapshots are deliberately short-lived.  This is long enough for
// a burst of browser tabs to reuse one completed read, but below the UI's
// normal refresh cadence and far below any retention/ACL mutation window.
// The WebSocket coalescer clears these entries at its telemetry boundary just
// before clients are told to refetch.  Per-sample notices only mark that
// boundary pending, so staggered agents cannot continuously defeat this TTL.
const MONITORING_READ_CACHE_TTL: Duration = Duration::from_secs(1);
const MAX_SINGLEFLIGHT_ENTRIES: usize = 512;
static SUITE_CONFIG_LAST_KNOWN_GOOD: OnceLock<StdRwLock<HashMap<PathBuf, SuiteConfig>>> =
    OnceLock::new();

#[derive(Default)]
struct PendingWsInvalidations {
    fleet_telemetry: bool,
    job_ids: BTreeSet<uuid::Uuid>,
}

#[derive(Clone)]
pub(crate) struct WsEventBus {
    public_events: broadcast::Sender<WsEvent>,
    invalidations: Weak<Mutex<PendingWsInvalidations>>,
    fleet_snapshot_singleflight: Singleflight<FleetSnapshotResponse>,
    monitoring_cards_singleflight: Singleflight<MonitoringCardsPageView>,
    client_monitoring_singleflight: Singleflight<ClientMonitoringView>,
    dashboard_overview_singleflight: Singleflight<DashboardOverviewView>,
    system_dashboard_singleflight: Singleflight<SystemDashboardView>,
    home_snapshot_singleflight: Singleflight<HomeSnapshotResponse>,
}

pub(crate) struct WsInvalidationDriver {
    pending: Arc<Mutex<PendingWsInvalidations>>,
}

impl WsEventBus {
    pub(crate) fn new(capacity: usize) -> (Self, WsInvalidationDriver) {
        let (public_events, _) = broadcast::channel(capacity);
        let pending = Arc::new(Mutex::new(PendingWsInvalidations::default()));
        (
            Self {
                public_events,
                invalidations: Arc::downgrade(&pending),
                fleet_snapshot_singleflight: Singleflight::with_ttl(MONITORING_READ_CACHE_TTL),
                monitoring_cards_singleflight: Singleflight::with_ttl(MONITORING_READ_CACHE_TTL),
                client_monitoring_singleflight: Singleflight::with_ttl(MONITORING_READ_CACHE_TTL),
                dashboard_overview_singleflight: Singleflight::with_ttl(MONITORING_READ_CACHE_TTL),
                system_dashboard_singleflight: Singleflight::with_ttl(MONITORING_READ_CACHE_TTL),
                home_snapshot_singleflight: Singleflight::with_ttl(MONITORING_READ_CACHE_TTL),
            },
            WsInvalidationDriver { pending },
        )
    }

    pub(crate) async fn singleflight_fleet_snapshot<F, Fut>(
        &self,
        key: String,
        load: F,
    ) -> Result<FleetSnapshotResponse, ApiError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<FleetSnapshotResponse, ApiError>> + Send + 'static,
    {
        self.fleet_snapshot_singleflight
            .run(
                key,
                "fleet_snapshot_panicked",
                "The fleet snapshot could not be prepared.",
                load,
            )
            .await
    }

    pub(crate) async fn singleflight_monitoring_cards<F, Fut>(
        &self,
        key: String,
        load: F,
    ) -> Result<MonitoringCardsPageView, ApiError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<MonitoringCardsPageView, ApiError>> + Send + 'static,
    {
        self.monitoring_cards_singleflight
            .run(
                key,
                "monitoring_cards_panicked",
                "The VPS monitoring cards could not be prepared.",
                load,
            )
            .await
    }

    pub(crate) async fn singleflight_client_monitoring<F, Fut>(
        &self,
        key: String,
        load: F,
    ) -> Result<ClientMonitoringView, ApiError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<ClientMonitoringView, ApiError>> + Send + 'static,
    {
        self.client_monitoring_singleflight
            .run(
                key,
                "client_monitoring_panicked",
                "The VPS monitoring detail could not be prepared.",
                load,
            )
            .await
    }

    pub(crate) async fn singleflight_dashboard_overview<F, Fut>(
        &self,
        key: String,
        load: F,
    ) -> Result<DashboardOverviewView, ApiError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<DashboardOverviewView, ApiError>> + Send + 'static,
    {
        self.dashboard_overview_singleflight
            .run(
                key,
                "dashboard_overview_panicked",
                "The dashboard overview could not be prepared.",
                load,
            )
            .await
    }

    pub(crate) async fn singleflight_system_dashboard<F, Fut>(
        &self,
        key: String,
        load: F,
    ) -> Result<SystemDashboardView, ApiError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<SystemDashboardView, ApiError>> + Send + 'static,
    {
        self.system_dashboard_singleflight
            .run(
                key,
                "system_dashboard_panicked",
                "The system dashboard could not be prepared.",
                load,
            )
            .await
    }

    pub(crate) async fn singleflight_home_snapshot<F, Fut>(
        &self,
        key: String,
        load: F,
    ) -> Result<HomeSnapshotResponse, ApiError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<HomeSnapshotResponse, ApiError>> + Send + 'static,
    {
        self.home_snapshot_singleflight
            .run(
                key,
                "home_snapshot_panicked",
                "The Home snapshot could not be prepared.",
                load,
            )
            .await
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<WsEvent> {
        self.public_events.subscribe()
    }

    pub(crate) fn publish(&self, event: WsEvent) {
        let _ = self.public_events.send(event);
    }

    #[cfg(test)]
    pub(crate) fn invalidate_fleet_telemetry(&self) {
        self.invalidate_fleet_telemetry_read_cache();
        self.notify_fleet_telemetry();
    }

    pub(crate) fn invalidate_fleet_telemetry_read_cache(&self) {
        self.fleet_snapshot_singleflight.clear();
        self.monitoring_cards_singleflight.clear();
        self.client_monitoring_singleflight.clear();
        self.dashboard_overview_singleflight.clear();
        self.system_dashboard_singleflight.clear();
        self.home_snapshot_singleflight.clear();
    }

    pub(crate) fn notify_fleet_telemetry(&self) {
        let Some(pending) = self.invalidations.upgrade() else {
            return;
        };
        pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fleet_telemetry = true;
    }

    pub(crate) fn invalidate_job_details(&self, job_id: uuid::Uuid) {
        let Some(pending) = self.invalidations.upgrade() else {
            return;
        };
        pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .job_ids
            .insert(job_id);
    }
}

#[derive(Clone)]
struct SharedApiError {
    status: axum::http::StatusCode,
    code: &'static str,
    error: String,
    public_message: Option<String>,
}

impl SharedApiError {
    fn from_api_error(error: ApiError) -> Self {
        Self {
            status: error.status,
            code: error.code,
            error: error.error.to_string(),
            public_message: error.public_message,
        }
    }

    fn panicked(code: &'static str, public_message: &'static str) -> Self {
        Self {
            status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            code,
            error: code.to_string(),
            public_message: Some(public_message.to_string()),
        }
    }

    fn into_api_error(self) -> ApiError {
        ApiError {
            status: self.status,
            code: self.code,
            error: anyhow::anyhow!(self.error),
            public_message: self.public_message,
        }
    }
}

struct SingleflightEntry<T> {
    generation: u64,
    result: AsyncMutex<Option<CachedSingleflightResult<T>>>,
    completed: AtomicBool,
    ready: Notify,
    #[cfg(test)]
    participants: std::sync::atomic::AtomicUsize,
}

struct CachedSingleflightResult<T> {
    value: std::result::Result<T, SharedApiError>,
    expires_at: Option<Instant>,
}

impl<T> SingleflightEntry<T> {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            result: AsyncMutex::new(None),
            completed: AtomicBool::new(false),
            ready: Notify::new(),
            #[cfg(test)]
            participants: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

struct Singleflight<T> {
    entries: Arc<AsyncMutex<HashMap<String, Arc<SingleflightEntry<T>>>>>,
    generation: Arc<AtomicU64>,
    cache_ttl: Duration,
}

impl<T> Clone for Singleflight<T> {
    fn clone(&self) -> Self {
        Self {
            entries: Arc::clone(&self.entries),
            generation: Arc::clone(&self.generation),
            cache_ttl: self.cache_ttl,
        }
    }
}

impl<T> Default for Singleflight<T> {
    fn default() -> Self {
        Self {
            entries: Arc::new(AsyncMutex::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
            cache_ttl: Duration::ZERO,
        }
    }
}

impl<T: Send + 'static> Singleflight<T> {
    fn with_ttl(cache_ttl: Duration) -> Self {
        Self {
            entries: Arc::new(AsyncMutex::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
            cache_ttl,
        }
    }

    fn clear(&self) {
        // Keep an in-flight entry discoverable so invalidation cannot fan one
        // expensive read out into duplicate leaders.  The generation makes
        // every completed value from before this point stale immediately;
        // callers that started afterward join the old leader, then coalesce
        // behind one trailing computation instead of consuming its result.
        self.generation.fetch_add(1, AtomicOrdering::AcqRel);
    }
}

impl<T> Singleflight<T>
where
    T: Clone + Send + 'static,
{
    async fn invoke<F, Fut>(
        panic_code: &'static str,
        panic_public_message: &'static str,
        load: F,
    ) -> std::result::Result<T, SharedApiError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = std::result::Result<T, ApiError>> + Send + 'static,
    {
        match std::panic::catch_unwind(AssertUnwindSafe(load)) {
            Ok(future) => AssertUnwindSafe(future)
                .catch_unwind()
                .await
                .map_err(|_| SharedApiError::panicked(panic_code, panic_public_message))
                .and_then(|result| result.map_err(SharedApiError::from_api_error)),
            Err(_) => Err(SharedApiError::panicked(panic_code, panic_public_message)),
        }
    }

    async fn run<F, Fut>(
        &self,
        key: String,
        panic_code: &'static str,
        panic_public_message: &'static str,
        load: F,
    ) -> std::result::Result<T, ApiError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = std::result::Result<T, ApiError>> + Send + 'static,
    {
        let request_generation = self.generation.load(AtomicOrdering::Acquire);
        let mut load = Some(load);
        'request: loop {
            let (entry, leader) = loop {
                let existing = {
                    let entries = self.entries.lock().await;
                    entries.get(&key).cloned()
                };
                if let Some(entry) = existing {
                    // Completion publication is the zero-TTL singleflight
                    // boundary.  A caller that observed the entry before it
                    // was published is an overlapping follower even if the
                    // result lands before it acquires the result mutex.
                    if !entry.completed.load(AtomicOrdering::Acquire) {
                        break (entry, false);
                    }
                    let now = Instant::now();
                    let cached = entry.result.lock().await;
                    match cached.as_ref() {
                        None => {
                            drop(cached);
                            break (entry, false);
                        }
                        Some(result)
                            if entry.generation >= request_generation
                                && result.expires_at.is_some_and(|expires_at| expires_at > now) =>
                        {
                            return result.value.clone().map_err(SharedApiError::into_api_error);
                        }
                        Some(_) => {
                            drop(cached);
                            let mut entries = self.entries.lock().await;
                            if entries
                                .get(&key)
                                .is_some_and(|current| Arc::ptr_eq(current, &entry))
                            {
                                entries.remove(&key);
                            }
                            drop(entries);
                            continue;
                        }
                    }
                }

                let mut entries = self.entries.lock().await;
                // Opportunistically discard stale or expired completed entries.
                // In-flight entries remain discoverable regardless of their
                // generation so invalidation never creates parallel leaders.
                let now = Instant::now();
                let generation = self.generation.load(AtomicOrdering::Acquire);
                entries.retain(|_, candidate| {
                    if !candidate.completed.load(AtomicOrdering::Acquire) {
                        return true;
                    }
                    candidate.result.try_lock().map_or(true, |cached| {
                        cached.as_ref().is_some_and(|result| {
                            candidate.generation >= generation
                                && result.expires_at.is_some_and(|expires_at| expires_at > now)
                        })
                    })
                });
                // Recheck the requested key before capacity eviction. Another
                // caller may have inserted and even completed it between the
                // first lookup and this map lock; evicting that exact result
                // would defeat same-key coalescing at the hard bound.
                if entries.contains_key(&key) {
                    drop(entries);
                    continue;
                }
                if entries.len() >= MAX_SINGLEFLIGHT_ENTRIES {
                    // Any completed entry is safe to evict at the hard bound:
                    // attached callers retain its Arc and can still consume
                    // the result.  The atomic completion flag avoids treating
                    // a briefly busy result mutex as an in-flight load.
                    if let Some(evict_key) =
                        entries.iter().find_map(|(candidate_key, candidate)| {
                            candidate
                                .completed
                                .load(AtomicOrdering::Acquire)
                                .then(|| candidate_key.clone())
                        })
                    {
                        entries.remove(&evict_key);
                    }
                }
                if entries.len() >= MAX_SINGLEFLIGHT_ENTRIES {
                    // The hard bound limits only deduplication metadata. If all
                    // registered keys are genuinely in flight, a new distinct
                    // request executes directly instead of waiting forever or
                    // returning 429. Same-key callers joined above and still
                    // share their existing leader.
                    drop(entries);
                    let load = load
                        .take()
                        .expect("an unregistered singleflight caller loads only once");
                    return Self::invoke(panic_code, panic_public_message, load)
                        .await
                        .map_err(SharedApiError::into_api_error);
                }
                let entry = Arc::new(SingleflightEntry::new(generation));
                entries.insert(key.clone(), Arc::clone(&entry));
                break (entry, true);
            };
            #[cfg(test)]
            entry
                .participants
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

            if leader {
                let entries = Arc::clone(&self.entries);
                let generation = Arc::clone(&self.generation);
                let task_entry = Arc::clone(&entry);
                let task_key = key.clone();
                let cache_ttl = self.cache_ttl;
                let load = load.take().expect("singleflight caller can lead only once");
                tokio::spawn(async move {
                    let result = Self::invoke(panic_code, panic_public_message, load).await;
                    let cache_result = result.is_ok()
                        && !cache_ttl.is_zero()
                        && generation.load(AtomicOrdering::Acquire) == task_entry.generation;
                    *task_entry.result.lock().await = Some(CachedSingleflightResult {
                        value: result,
                        expires_at: cache_result.then(|| Instant::now() + cache_ttl),
                    });
                    task_entry.completed.store(true, AtomicOrdering::Release);
                    if !cache_result
                        || generation.load(AtomicOrdering::Acquire) != task_entry.generation
                    {
                        let mut entries = entries.lock().await;
                        if entries
                            .get(&task_key)
                            .is_some_and(|current| Arc::ptr_eq(current, &task_entry))
                        {
                            entries.remove(&task_key);
                        }
                    }
                    task_entry.ready.notify_waiters();
                });
            }

            loop {
                let ready = entry.ready.notified();
                tokio::pin!(ready);
                ready.as_mut().enable();
                if let Some(result) = entry.result.lock().await.as_ref() {
                    if entry.generation < request_generation {
                        continue 'request;
                    }
                    return result.value.clone().map_err(SharedApiError::into_api_error);
                }
                ready.await;
            }
        }
    }

    #[cfg(test)]
    async fn wait_for_participants(&self, key: &str, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let participants = {
                    let entries = self.entries.lock().await;
                    entries.get(key).map_or(0, |entry| {
                        entry.participants.load(std::sync::atomic::Ordering::SeqCst)
                    })
                };
                if participants >= expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("singleflight callers did not join the held computation");
    }
}

pub(crate) fn read_singleflight_auth_key(operator_id: uuid::Uuid, scopes: &[String]) -> String {
    let mut scopes = scopes.to_vec();
    scopes.sort();
    scopes.dedup();
    format!("{operator_id}|{}", scopes.join("\u{1f}"))
}

impl WsInvalidationDriver {
    pub(crate) fn take_fleet_telemetry(&self) -> bool {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut pending.fleet_telemetry)
    }

    pub(crate) fn take_job_ids(&self) -> Vec<uuid::Uuid> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut pending.job_ids).into_iter().collect()
    }
}

pub(crate) fn remember_suite_config(path: &Path, config: &SuiteConfig) {
    let cache = SUITE_CONFIG_LAST_KNOWN_GOOD.get_or_init(|| StdRwLock::new(HashMap::new()));
    if let Ok(mut cache) = cache.write() {
        cache.insert(path.to_path_buf(), config.clone());
    }
}

fn load_suite_config_last_known_good(path: &Path) -> Option<SuiteConfig> {
    let cached = || {
        SUITE_CONFIG_LAST_KNOWN_GOOD
            .get()
            .and_then(|cache| cache.read().ok())
            .and_then(|cache| cache.get(path).cloned())
    };
    // `SuiteConfig::load_optional` intentionally treats a missing file as an
    // empty/default config. That is correct at startup, but during hot reload
    // an atomic replace can make the path briefly absent. Preserve the last
    // valid runtime values across that gap.
    if !path.exists() {
        return cached().or_else(|| SuiteConfig::load_optional(path).ok());
    }
    match SuiteConfig::load_optional(path) {
        Ok(config) => {
            remember_suite_config(path, &config);
            Some(config)
        }
        Err(_) => cached(),
    }
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) repo: Repository,
    pub(crate) events: WsEventBus,
    pub(crate) dashboard_telemetry: DashboardTelemetryResident,
    pub(crate) internal_token: Option<String>,
    pub(crate) gateway: GatewayDispatchClient,
    pub(crate) backup_object_store: Option<BackupObjectStore>,
    pub(crate) reviewed_artifact_deletions: ReviewedArtifactDeletionProducer,
    pub(crate) update_release_policy: UpdateReleasePolicy,
    pub(crate) job_output_artifact_min_bytes: usize,
    pub(crate) artifact_max_bytes: usize,
    pub(crate) require_registered_agent_updates: bool,
    pub(crate) suite_config_path: PathBuf,
    pub(crate) dispatcher_config: DispatcherRuntimeConfig,
}

#[derive(Clone, Debug)]
pub(crate) struct DispatcherRuntimeConfig {
    pub(crate) batch_limit: i64,
    pub(crate) in_flight: usize,
    pub(crate) dispatch_ack_secs: u64,
    pub(crate) event_post_secs: u64,
    pub(crate) internal_http_connect_secs: u64,
    pub(crate) internal_http_write_secs: u64,
    pub(crate) internal_http_read_secs: u64,
    pub(crate) control_deadline_grace_secs: u64,
    pub(crate) max_job_timeout_secs: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct OperatorAuthThrottleConfig {
    pub(crate) username_failed_attempt_limit: i64,
    pub(crate) ip_failed_attempt_limit: i64,
    pub(crate) failed_attempt_window_secs: u64,
    pub(crate) lockout_secs: u64,
}

impl Default for OperatorAuthThrottleConfig {
    fn default() -> Self {
        Self {
            username_failed_attempt_limit: DEFAULT_OPERATOR_AUTH_USERNAME_FAILED_ATTEMPT_LIMIT,
            ip_failed_attempt_limit: DEFAULT_OPERATOR_AUTH_IP_FAILED_ATTEMPT_LIMIT,
            failed_attempt_window_secs: DEFAULT_OPERATOR_AUTH_FAILED_ATTEMPT_WINDOW_SECS,
            lockout_secs: DEFAULT_OPERATOR_AUTH_LOCKOUT_SECS,
        }
    }
}

impl Default for DispatcherRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 128,
            in_flight: 64,
            dispatch_ack_secs: 30,
            event_post_secs: 15,
            internal_http_connect_secs: 10,
            internal_http_write_secs: 10,
            internal_http_read_secs: 15,
            control_deadline_grace_secs: 30,
            max_job_timeout_secs: DEFAULT_MAX_JOB_TIMEOUT_SECS,
        }
    }
}

impl DispatcherRuntimeConfig {
    /// A durable owner may be claimed only when this process can start it in
    /// the current wave. `batch_limit` still bounds one database claim when
    /// operators configure a smaller transaction than the execution pool.
    pub(crate) fn immediate_claim_limit(&self) -> i64 {
        self.batch_limit.min(self.in_flight as i64).max(1)
    }

    pub(crate) fn control_deadline_extra_secs(&self) -> u64 {
        self.gateway_dispatch_attempt_timeout_secs()
            .saturating_add(self.event_post_secs)
            .saturating_add(self.control_deadline_grace_secs)
    }

    /// The gateway client performs these four independently bounded I/O
    /// phases in sequence. A durable dispatch owner must therefore cover the
    /// sum, not only the longest individual timeout.
    pub(crate) fn gateway_dispatch_attempt_timeout_secs(&self) -> u64 {
        self.internal_http_connect_secs
            .saturating_add(self.internal_http_write_secs.saturating_mul(2))
            .saturating_add(self.dispatch_ack_secs.max(self.internal_http_read_secs))
    }

    /// Five seconds is reserved only for local response decoding and the
    /// token-fenced database completion after the bounded gateway I/O. It is
    /// not a throughput delay and is never slept.
    pub(crate) fn gateway_dispatch_attempt_lease_secs(&self) -> u64 {
        self.gateway_dispatch_attempt_timeout_secs()
            .saturating_add(5)
            .clamp(1, 7200)
    }

    /// Internal durable lanes renew this lease while external work is active;
    /// they do not need to inherit the gateway's connect/write phases.
    pub(crate) fn dispatch_lease_secs(&self) -> u64 {
        self.dispatch_ack_secs
            .max(self.internal_http_read_secs)
            .saturating_add(5)
            .clamp(1, 7200)
    }
}

impl AppState {
    fn current_suite_config(&self) -> Option<SuiteConfig> {
        load_suite_config_last_known_good(&self.suite_config_path)
    }

    pub(crate) fn dispatcher_runtime_config(&self) -> DispatcherRuntimeConfig {
        let mut config = self.dispatcher_config.clone();
        if let Some(suite) = self.current_suite_config() {
            if env_absent("VPSMAN_DISPATCHER_BATCH") {
                if let Some(value) = suite.capacity.dispatcher_batch {
                    config.batch_limit = value;
                }
            }
            if env_absent("VPSMAN_DISPATCHER_IN_FLIGHT") {
                if let Some(value) = suite.capacity.dispatcher_in_flight {
                    config.in_flight = value;
                }
            }
            if env_absent("VPSMAN_DISPATCH_ACK_SECS") {
                if let Some(value) = suite.timeout.dispatch_ack_secs {
                    config.dispatch_ack_secs = value;
                }
            }
            if env_absent("VPSMAN_EVENT_POST_SECS") {
                if let Some(value) = suite.timeout.event_post_secs {
                    config.event_post_secs = value;
                }
            }
            if env_absent("VPSMAN_INTERNAL_HTTP_READ_SECS") {
                if let Some(value) = suite.timeout.internal_http_read_secs {
                    config.internal_http_read_secs = value;
                }
            }
            if env_absent("VPSMAN_CONTROL_DEADLINE_GRACE_SECS") {
                if let Some(value) = suite.timeout.control_deadline_grace_secs {
                    config.control_deadline_grace_secs = value;
                }
            }
            if env_absent("VPSMAN_MAX_JOB_TIMEOUT_SECS") {
                if let Some(value) = suite.timeout.max_job_timeout_secs {
                    config.max_job_timeout_secs = value;
                }
            }
        }
        config.batch_limit = config.batch_limit.clamp(1, 500);
        config.in_flight = config.in_flight.clamp(1, 512);
        config.dispatch_ack_secs = config.dispatch_ack_secs.clamp(1, 3600);
        config.event_post_secs = config.event_post_secs.clamp(1, 3600);
        config.internal_http_connect_secs = config.internal_http_connect_secs.clamp(1, 300);
        config.internal_http_write_secs = config.internal_http_write_secs.clamp(1, 300);
        config.internal_http_read_secs = config.internal_http_read_secs.clamp(1, 3600);
        config.control_deadline_grace_secs = config.control_deadline_grace_secs.clamp(0, 3600);
        config.max_job_timeout_secs = config
            .max_job_timeout_secs
            .clamp(1, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS);
        config
    }

    pub(crate) fn refresh_gateway_dispatch_timeouts(&self) {
        let config = self.dispatcher_runtime_config();
        self.gateway.set_read_timeout(Duration::from_secs(
            config
                .internal_http_read_secs
                .max(config.dispatch_ack_secs)
                .clamp(1, 3600),
        ));
    }

    pub(crate) async fn disconnect_gateway_session_for_lifecycle(
        &self,
        client_id: &str,
        reason: &str,
    ) -> Result<(), ApiError> {
        if !self.gateway.configured() {
            #[cfg(test)]
            if self.gateway.test_privilege_auto_approves() {
                return Ok(());
            }
            return Err(ApiError::conflict("gateway_control_url_missing"));
        }
        self.refresh_gateway_dispatch_timeouts();
        let result = self
            .gateway
            .disconnect_session(client_id, reason)
            .await
            .map_err(|_| ApiError::conflict("gateway_session_disconnect_failed"))?;
        if result.accepted {
            Ok(())
        } else {
            Err(ApiError::conflict("gateway_session_disconnect_failed"))
        }
    }

    pub(crate) fn job_output_artifact_min_bytes(&self) -> usize {
        if let Some(suite) = self.current_suite_config() {
            if env_absent("VPSMAN_JOB_OUTPUT_ARTIFACT_MIN_BYTES") {
                if let Some(value) = suite.api.job_output_artifact_min_bytes {
                    return value;
                }
            }
        }
        self.job_output_artifact_min_bytes
    }

    pub(crate) fn artifact_max_bytes(&self) -> usize {
        let mut value = self.artifact_max_bytes;
        if let Some(suite) = self.current_suite_config() {
            if env_absent("VPSMAN_ARTIFACT_MAX_BYTES") {
                if let Some(configured) = suite.api.artifact_max_bytes {
                    value = configured;
                }
            }
        }
        value.clamp(MIN_ARTIFACT_MAX_BYTES, MAX_ARTIFACT_MAX_BYTES)
    }

    pub(crate) fn max_job_timeout_secs(&self) -> u64 {
        self.dispatcher_runtime_config().max_job_timeout_secs
    }

    pub(crate) fn tunnel_allocation_pool_cidrs(&self) -> (Option<String>, Option<String>) {
        let Some(suite) = self.current_suite_config() else {
            return (None, None);
        };
        (
            normalize_optional_text(suite.network.tunnel_ipv4_allocation_pool_cidr),
            normalize_optional_text(suite.network.tunnel_ipv6_allocation_pool_cidr),
        )
    }

    pub(crate) fn require_registered_agent_updates(&self) -> bool {
        if let Some(suite) = self.current_suite_config() {
            if env_absent("VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES") {
                if let Some(value) = suite.api.require_registered_agent_updates {
                    return value;
                }
            }
        }
        self.require_registered_agent_updates
    }

    pub(crate) fn operator_auth_throttle_config(&self) -> OperatorAuthThrottleConfig {
        let mut config = OperatorAuthThrottleConfig::default();
        if let Some(suite) = self.current_suite_config() {
            if env_absent("VPSMAN_OPERATOR_AUTH_USERNAME_FAILED_ATTEMPT_LIMIT") {
                if let Some(value) = suite.api.operator_auth_username_failed_attempt_limit {
                    config.username_failed_attempt_limit = value;
                }
            }
            if env_absent("VPSMAN_OPERATOR_AUTH_IP_FAILED_ATTEMPT_LIMIT") {
                if let Some(value) = suite.api.operator_auth_ip_failed_attempt_limit {
                    config.ip_failed_attempt_limit = value;
                }
            }
            if env_absent("VPSMAN_OPERATOR_AUTH_FAILED_ATTEMPT_WINDOW_SECS") {
                if let Some(value) = suite.api.operator_auth_failed_attempt_window_secs {
                    config.failed_attempt_window_secs = value;
                }
            }
            if env_absent("VPSMAN_OPERATOR_AUTH_LOCKOUT_SECS") {
                if let Some(value) = suite.api.operator_auth_lockout_secs {
                    config.lockout_secs = value;
                }
            }
        }
        if let Ok(value) = std::env::var("VPSMAN_OPERATOR_AUTH_USERNAME_FAILED_ATTEMPT_LIMIT") {
            if let Ok(parsed) = value.parse::<i64>() {
                config.username_failed_attempt_limit = parsed;
            }
        }
        if let Ok(value) = std::env::var("VPSMAN_OPERATOR_AUTH_IP_FAILED_ATTEMPT_LIMIT") {
            if let Ok(parsed) = value.parse::<i64>() {
                config.ip_failed_attempt_limit = parsed;
            }
        }
        if let Ok(value) = std::env::var("VPSMAN_OPERATOR_AUTH_FAILED_ATTEMPT_WINDOW_SECS") {
            if let Ok(parsed) = value.parse::<u64>() {
                config.failed_attempt_window_secs = parsed;
            }
        }
        if let Ok(value) = std::env::var("VPSMAN_OPERATOR_AUTH_LOCKOUT_SECS") {
            if let Ok(parsed) = value.parse::<u64>() {
                config.lockout_secs = parsed;
            }
        }
        config.username_failed_attempt_limit = config.username_failed_attempt_limit.clamp(1, 1000);
        config.ip_failed_attempt_limit = config.ip_failed_attempt_limit.clamp(1, 1000);
        config.failed_attempt_window_secs = config
            .failed_attempt_window_secs
            .clamp(60, 30 * 24 * 60 * 60);
        config.lockout_secs = config.lockout_secs.clamp(60, 30 * 24 * 60 * 60);
        config
    }

    pub(crate) fn operator_client_ip(&self, peer: SocketAddr, headers: &HeaderMap) -> String {
        self.trusted_proxy_config()
            .resolve_client_ip(peer, headers)
            .to_string()
    }

    fn trusted_proxy_config(&self) -> TrustedProxyConfig {
        if let Ok(value) = std::env::var("VPSMAN_TRUSTED_PROXY_CIDRS") {
            return TrustedProxyConfig::from_env_csv(&value)
                .unwrap_or_else(|_| TrustedProxyConfig::trust_none());
        }
        let entries = self
            .current_suite_config()
            .and_then(|suite| suite.api.trusted_proxy_cidrs);
        TrustedProxyConfig::from_optional_entries(entries.as_deref()).unwrap_or_default()
    }

    pub(crate) fn schedule_apply_now_max_timeout_secs(&self) -> u64 {
        if let Ok(value) = std::env::var("VPSMAN_WORKER_SCHEDULE_JOB_MAX_TIMEOUT_SECS") {
            if let Ok(parsed) = value.parse::<u64>() {
                return parsed.clamp(1, self.max_job_timeout_secs());
            }
        }
        let configured = self
            .current_suite_config()
            .and_then(|suite| suite.worker.schedule_job_max_timeout_secs);
        configured
            .unwrap_or(DEFAULT_MAX_JOB_TIMEOUT_SECS)
            .clamp(1, self.max_job_timeout_secs())
    }
}

fn env_absent(name: &str) -> bool {
    std::env::var_os(name).is_none()
}

#[derive(Clone, Debug, Default)]
pub(crate) struct UpdateReleasePolicy {
    allowed_channels: Vec<String>,
}

impl UpdateReleasePolicy {
    pub(crate) fn new(allowed_channels: Vec<String>) -> Result<Self> {
        let mut normalized_channels = Vec::new();
        for channel in allowed_channels {
            let channel = channel.trim().to_ascii_lowercase();
            if channel.is_empty() {
                continue;
            }
            if !is_safe_release_token(&channel, 32) {
                anyhow::bail!("update release channel {channel:?} is invalid");
            }
            if !normalized_channels.iter().any(|stored| stored == &channel) {
                normalized_channels.push(channel);
            }
        }

        Ok(Self {
            allowed_channels: normalized_channels,
        })
    }

    pub(crate) fn validate_channel(&self, channel: &str) -> Result<(), ApiError> {
        if self.allowed_channels.is_empty() {
            return Ok(());
        }
        let channel = channel.trim().to_ascii_lowercase();
        if self
            .allowed_channels
            .iter()
            .any(|allowed| allowed == &channel)
        {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "agent_update_release_channel_not_allowed",
            ))
        }
    }
}

impl AppState {
    pub(crate) fn require_internal_gateway(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        let Some(expected_token) = self.internal_token.as_deref() else {
            return Err(ApiError::unauthorized("missing_internal_token"));
        };
        let provided = bearer_token(headers)
            .ok_or_else(|| ApiError::unauthorized("missing_internal_token"))?;
        if constant_time_eq(provided.as_bytes(), expected_token.as_bytes()) {
            Ok(())
        } else {
            Err(ApiError::unauthorized("invalid_internal_token"))
        }
    }

    pub(crate) async fn require_operator(
        &self,
        headers: &HeaderMap,
    ) -> Result<AuthContext, ApiError> {
        let token =
            bearer_token(headers).ok_or_else(|| ApiError::unauthorized("missing_bearer_token"))?;
        self.repo
            .authenticate_access_token(token)
            .await?
            .ok_or_else(|| ApiError::unauthorized("invalid_bearer_token"))
    }

    pub(crate) async fn require_operator_role(
        &self,
        headers: &HeaderMap,
        required_role: &'static str,
    ) -> Result<AuthContext, ApiError> {
        let operator = self.require_operator(headers).await?;
        if role_allows(&operator.operator.role, required_role) {
            Ok(operator)
        } else {
            Err(ApiError::forbidden("operator_role_insufficient"))
        }
    }

    pub(crate) async fn require_operator_role_and_scope(
        &self,
        headers: &HeaderMap,
        required_role: &'static str,
        required_scope: &'static str,
    ) -> Result<AuthContext, ApiError> {
        let operator = self.require_operator_role(headers, required_role).await?;
        if operator_has_scope(&operator.operator.scopes, required_scope) {
            Ok(operator)
        } else {
            Err(ApiError::forbidden("operator_scope_insufficient"))
        }
    }

    pub(crate) async fn require_operator_scope(
        &self,
        headers: &HeaderMap,
        required_scope: &'static str,
    ) -> Result<AuthContext, ApiError> {
        let operator = self.require_operator(headers).await?;
        if operator_has_scope(&operator.operator.scopes, required_scope) {
            Ok(operator)
        } else {
            Err(ApiError::forbidden("operator_scope_insufficient"))
        }
    }

    pub(crate) fn publish(&self, event: WsEvent) {
        self.events.publish(event);
    }

    pub(crate) fn invalidate_job_details(&self, job_id: uuid::Uuid) {
        self.events.invalidate_job_details(job_id);
    }

    pub(crate) async fn process_job_terminal_events(
        &self,
        limit: i64,
    ) -> Result<TerminalizationBatch> {
        let lease_secs = self.dispatcher_runtime_config().dispatch_lease_secs() as i64;
        let mut remaining = limit.clamp(1, 1000);
        let mut processed = TerminalizationBatch::default();
        loop {
            let batch = self
                .repo
                .process_pending_job_terminal_events(remaining, lease_secs)
                .await?;
            let handled = batch.targets.len().saturating_add(batch.jobs.len());
            if handled == 0 {
                break;
            }
            debug_assert!(batch.targets.iter().all(|event| {
                event.job_id != uuid::Uuid::nil()
                    && !event.client_id.is_empty()
                    && !event.outcome.status.is_empty()
            }));
            for event in &batch.jobs {
                if !matches!(
                    event.status.as_str(),
                    JOB_STATUS_QUEUED | JOB_STATUS_RUNNING
                ) {
                    self.publish(WsEvent::JobFinished {
                        job_id: event.job_id,
                        status: event.status.clone(),
                    });
                }
            }
            remaining = remaining.saturating_sub(i64::try_from(handled).unwrap_or(i64::MAX));
            processed.extend(batch);
            if remaining <= 0 {
                break;
            }
        }
        Ok(processed)
    }

    pub(crate) async fn enrich_job_terminal_target(
        &self,
        work: &ClaimedJobTerminalEnrichment,
    ) -> Result<()> {
        if let Err(error) = record_network_routing_terminal_result(
            self,
            work.job_id,
            &work.client_id,
            &work.status,
            None,
        )
        .await
        {
            if network_routing_terminal_error_is_permanent(error.code)
                || error
                    .error
                    .to_string()
                    .starts_with("invalid_job_operation:")
            {
                warn!(
                    code = error.code,
                    error = ?error.error,
                    event_id = %work.event_id,
                    job_id = %work.job_id,
                    client_id = %work.client_id,
                    "discarding invalid durable network-routing terminal result"
                );
            } else {
                return Err(error.error);
            }
        }
        if work.status == TARGET_STATUS_COMPLETED {
            if let Err(error) =
                try_auto_record_backup_artifact_for_job_target(self, work.job_id, &work.client_id)
                    .await
            {
                if let Some(reason) = backup_auto_artifact_request_failure_reason(&error.error) {
                    self.repo
                        .mark_open_backup_request_artifact_validation_failed(
                            work.job_id,
                            &work.client_id,
                            reason,
                        )
                        .await?;
                }
                if backup_auto_artifact_error_is_permanent(&error.error) {
                    warn!(
                        error = ?error.error,
                        event_id = %work.event_id,
                        job_id = %work.job_id,
                        client_id = %work.client_id,
                        "discarding invalid durable backup-artifact enrichment"
                    );
                } else {
                    return Err(error.error);
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn fleet_snapshot(&self) -> Result<WsEvent> {
        Ok(WsEvent::FleetSnapshot {
            summary: self.repo.fleet_summary().await?,
            agents: self.repo.list_agents().await?,
        })
    }
}

fn network_routing_terminal_error_is_permanent(code: &str) -> bool {
    matches!(
        code,
        "network_routing_result_plan_id_invalid"
            | "network_routing_result_missing"
            | "network_routing_result_invalid"
            | "network_routing_result_contract_mismatch"
    )
}

fn is_safe_release_token(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && !value
            .chars()
            .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "tests_state.rs"]
mod tests;
