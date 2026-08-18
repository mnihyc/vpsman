use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, RwLock as StdRwLock, Weak},
    time::Duration,
};

use anyhow::Result;
use axum::http::HeaderMap;
use tokio::sync::broadcast;
use vpsman_common::{SuiteConfig, DEFAULT_MAX_JOB_TIMEOUT_SECS, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS};
use vpsman_server_core::{JOB_STATUS_QUEUED, JOB_STATUS_RUNNING};

use crate::{
    client_ip::TrustedProxyConfig,
    error::ApiError,
    gateway_client::GatewayDispatchClient,
    model::{AuthContext, WsEvent},
    object_store::BackupObjectStore,
    repository::Repository,
    repository_jobs::TerminalizationBatch,
    security::{bearer_token, constant_time_eq, operator_has_scope, role_allows},
};

pub(crate) const DEFAULT_ARTIFACT_MAX_BYTES: usize = 128 * 1024 * 1024;
const MIN_ARTIFACT_MAX_BYTES: usize = 1024 * 1024;
const MAX_ARTIFACT_MAX_BYTES: usize = 4 * 1024 * 1024 * 1024;
const DEFAULT_OPERATOR_AUTH_USERNAME_FAILED_ATTEMPT_LIMIT: i64 = 8;
const DEFAULT_OPERATOR_AUTH_IP_FAILED_ATTEMPT_LIMIT: i64 = 8;
const DEFAULT_OPERATOR_AUTH_FAILED_ATTEMPT_WINDOW_SECS: u64 = 15 * 60;
const DEFAULT_OPERATOR_AUTH_LOCKOUT_SECS: u64 = 15 * 60;
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
            },
            WsInvalidationDriver { pending },
        )
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<WsEvent> {
        self.public_events.subscribe()
    }

    pub(crate) fn publish(&self, event: WsEvent) {
        let _ = self.public_events.send(event);
    }

    pub(crate) fn invalidate_fleet_telemetry(&self) {
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
    pub(crate) internal_token: Option<String>,
    pub(crate) gateway: GatewayDispatchClient,
    pub(crate) backup_object_store: Option<BackupObjectStore>,
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
            internal_http_read_secs: 15,
            control_deadline_grace_secs: 30,
            max_job_timeout_secs: DEFAULT_MAX_JOB_TIMEOUT_SECS,
        }
    }
}

impl DispatcherRuntimeConfig {
    pub(crate) fn control_deadline_extra_secs(&self) -> u64 {
        self.dispatch_ack_secs
            .max(self.internal_http_read_secs)
            .saturating_add(self.event_post_secs)
            .saturating_add(self.control_deadline_grace_secs)
    }

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
        let configured = self.current_suite_config().and_then(|suite| {
            suite
                .worker
                .schedule_job_max_timeout_secs
                .or(suite.timeout.worker_schedule_job_max_timeout_secs)
        });
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

    pub(crate) fn invalidate_fleet_telemetry(&self) {
        self.events.invalidate_fleet_telemetry();
    }

    pub(crate) fn invalidate_job_details(&self, job_id: uuid::Uuid) {
        self.events.invalidate_job_details(job_id);
    }

    pub(crate) async fn terminal_job_status_after_refresh(
        &self,
        job_id: uuid::Uuid,
        refreshed: Option<String>,
    ) -> Result<Option<String>> {
        if let Some(status) = refreshed {
            return Ok(
                (!matches!(status.as_str(), JOB_STATUS_QUEUED | JOB_STATUS_RUNNING))
                    .then_some(status),
            );
        }
        let Some(job) = self.repo.get_job(job_id).await? else {
            return Ok(None);
        };
        if job.completed_at.is_some()
            && !matches!(job.status.as_str(), JOB_STATUS_QUEUED | JOB_STATUS_RUNNING)
        {
            Ok(Some(job.status))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn publish_job_finished_after_refresh(
        &self,
        job_id: uuid::Uuid,
        refreshed: Option<String>,
    ) -> Result<()> {
        if let Some(status) = self
            .terminal_job_status_after_refresh(job_id, refreshed)
            .await?
        {
            self.publish(WsEvent::JobFinished { job_id, status });
        }
        Ok(())
    }

    pub(crate) async fn process_job_terminal_events(
        &self,
        limit: i64,
    ) -> Result<TerminalizationBatch> {
        let batch = self
            .repo
            .process_pending_job_terminal_events(limit, 60)
            .await?;
        for event in &batch.targets {
            debug_assert_ne!(event.job_id, uuid::Uuid::nil());
            debug_assert!(!event.client_id.is_empty());
            debug_assert!(!event.outcome.status.is_empty());
        }
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
        Ok(batch)
    }

    pub(crate) async fn process_job_terminal_events_or_publish_refresh(
        &self,
        limit: i64,
        job_id: uuid::Uuid,
        refreshed: Option<String>,
    ) -> Result<()> {
        let batch = self.process_job_terminal_events(limit).await?;
        if !batch.jobs.iter().any(|event| event.job_id == job_id) {
            self.publish_job_finished_after_refresh(job_id, refreshed)
                .await?;
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
