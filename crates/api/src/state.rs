use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{OnceLock, RwLock as StdRwLock},
    time::Duration,
};

use anyhow::{ensure, Result};
use axum::http::HeaderMap;
use serde_json::{json, Map, Value};
use tokio::sync::broadcast;
use vpsman_common::{SuiteConfig, DEFAULT_MAX_JOB_TIMEOUT_SECS, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS};
use vpsman_server_core::{JOB_STATUS_QUEUED, JOB_STATUS_RUNNING};

use crate::{
    client_ip::TrustedProxyConfig,
    error::ApiError,
    fleet_alerts::FleetAlertPolicy,
    gateway_client::GatewayDispatchClient,
    model::{
        AgentView, AuthContext, NetworkObservationTrendView, NetworkOspfRecommendationView,
        TunnelPlanView, WsEvent,
    },
    model_source_templates::SourceStatusView,
    object_store::BackupObjectStore,
    repository::Repository,
    repository_jobs::TerminalizationBatch,
    repository_source_status::{BackupSourceEvidenceCounts, UpdateSourceEvidenceCounts},
    security::{bearer_token, constant_time_eq, operator_has_scope, role_allows},
};

pub(crate) const DEFAULT_ARTIFACT_MAX_BYTES: usize = 128 * 1024 * 1024;
const MIN_ARTIFACT_MAX_BYTES: usize = 1024 * 1024;
const MAX_ARTIFACT_MAX_BYTES: usize = 4 * 1024 * 1024 * 1024;
const DEFAULT_OPERATOR_AUTH_USERNAME_FAILED_ATTEMPT_LIMIT: i64 = 8;
const DEFAULT_OPERATOR_AUTH_IP_FAILED_ATTEMPT_LIMIT: i64 = 64;
const DEFAULT_OPERATOR_AUTH_FAILED_ATTEMPT_WINDOW_SECS: u64 = 15 * 60;
const DEFAULT_OPERATOR_AUTH_LOCKOUT_SECS: u64 = 15 * 60;
static SUITE_CONFIG_LAST_KNOWN_GOOD: OnceLock<StdRwLock<HashMap<PathBuf, SuiteConfig>>> =
    OnceLock::new();

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SourceStatusNetworkEnrichmentNeeds {
    tunnel_plans: bool,
    observation_trends: bool,
    ospf_recommendations: bool,
}

fn source_status_network_enrichment_needs(
    rows: &[SourceStatusView],
) -> SourceStatusNetworkEnrichmentNeeds {
    let mut needs = SourceStatusNetworkEnrichmentNeeds::default();
    for row in rows {
        match row.domain.as_str() {
            "runtime_tunnel_adapter" => {
                needs.tunnel_plans = true;
                needs.observation_trends = true;
            }
            "routing_cost_adapter" => {
                needs.tunnel_plans = true;
                needs.ospf_recommendations = true;
            }
            "runtime_traffic_accounting_source" | "traffic_limit_status_source" => {
                needs.tunnel_plans = true;
            }
            _ => {}
        }
    }
    needs
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
    pub(crate) events: broadcast::Sender<WsEvent>,
    pub(crate) internal_token: Option<String>,
    pub(crate) gateway: GatewayDispatchClient,
    pub(crate) backup_object_store: Option<BackupObjectStore>,
    pub(crate) update_release_policy: UpdateReleasePolicy,
    pub(crate) fleet_alert_policy: FleetAlertPolicy,
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

    pub(crate) fn fleet_alert_policy(&self) -> FleetAlertPolicy {
        let mut policy = self.fleet_alert_policy.clone();
        if let Some(suite) = self.current_suite_config() {
            if env_absent("VPSMAN_ALERT_MEMORY_AVAILABLE_WARNING_RATIO") {
                if let Some(value) = suite.api.alert_memory_available_warning_ratio {
                    policy.memory_available_warning_ratio = value;
                }
            }
            if env_absent("VPSMAN_ALERT_MEMORY_AVAILABLE_CRITICAL_RATIO") {
                if let Some(value) = suite.api.alert_memory_available_critical_ratio {
                    policy.memory_available_critical_ratio = value;
                }
            }
            if env_absent("VPSMAN_ALERT_DISK_AVAILABLE_WARNING_RATIO") {
                if let Some(value) = suite.api.alert_disk_available_warning_ratio {
                    policy.disk_available_warning_ratio = value;
                }
            }
            if env_absent("VPSMAN_ALERT_DISK_AVAILABLE_CRITICAL_RATIO") {
                if let Some(value) = suite.api.alert_disk_available_critical_ratio {
                    policy.disk_available_critical_ratio = value;
                }
            }
            if env_absent("VPSMAN_ALERT_CPU_LOAD_WARNING") {
                if let Some(value) = suite.api.alert_cpu_load_warning {
                    policy.cpu_load_warning = value;
                }
            }
            if env_absent("VPSMAN_ALERT_CPU_LOAD_CRITICAL") {
                if let Some(value) = suite.api.alert_cpu_load_critical {
                    policy.cpu_load_critical = value;
                }
            }
        }
        FleetAlertPolicy::new(
            policy.memory_available_warning_ratio,
            policy.memory_available_critical_ratio,
            policy.disk_available_warning_ratio,
            policy.disk_available_critical_ratio,
            policy.cpu_load_warning,
            policy.cpu_load_critical,
        )
        .unwrap_or_else(|_| self.fleet_alert_policy.clone())
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
    pub(crate) async fn list_source_status(
        &self,
        client_id: Option<&str>,
        domain: Option<&str>,
    ) -> Result<Vec<SourceStatusView>> {
        let rows = self.repo.list_source_status(client_id, domain).await?;
        self.enrich_source_status_rows(rows).await
    }

    pub(crate) async fn list_source_status_for_agents(
        &self,
        agents: &[AgentView],
        domain: Option<&str>,
    ) -> Result<Vec<SourceStatusView>> {
        let rows = self
            .repo
            .list_source_status_for_agents(agents, domain)
            .await?;
        self.enrich_source_status_rows(rows).await
    }

    async fn enrich_source_status_rows(
        &self,
        mut rows: Vec<SourceStatusView>,
    ) -> Result<Vec<SourceStatusView>> {
        let mut client_ids = rows
            .iter()
            .map(|row| row.client_id.clone())
            .collect::<Vec<_>>();
        client_ids.sort();
        client_ids.dedup();
        if rows.iter().any(|row| {
            matches!(
                row.domain.as_str(),
                "backup_object_store" | "restore_path_mapping"
            )
        }) {
            let counts = self.repo.source_backup_evidence_counts(&client_ids).await?;
            enrich_backup_status_rows(&mut rows, self.backup_object_store.as_ref(), &counts);
        }
        if rows.iter().any(|row| {
            matches!(
                row.domain.as_str(),
                "update_artifact_source"
                    | "update_restart_policy"
                    | "update_rollback_heartbeat_source"
            )
        }) {
            let counts = self.repo.source_update_evidence_counts().await?;
            enrich_update_status_rows(&mut rows, counts);
        }
        let network_needs = source_status_network_enrichment_needs(&rows);
        if network_needs.tunnel_plans {
            let plans = self
                .repo
                .list_tunnel_plans()
                .await?
                .into_iter()
                .filter(|plan| {
                    client_ids.iter().any(|client_id| {
                        plan.left_client_id == *client_id || plan.right_client_id == *client_id
                    })
                })
                .collect::<Vec<_>>();
            // Runtime traffic readiness only consumes the current saved plans.
            // Keep retained observation history and OSPF evidence off that
            // default polling path unless an adapter row can consume it.
            if network_needs.observation_trends || network_needs.ospf_recommendations {
                let trends = if network_needs.observation_trends {
                    self.repo
                        .list_recent_network_observation_trends_for_clients(&client_ids)
                        .await?
                } else {
                    Vec::new()
                };
                let recommendations = if network_needs.ospf_recommendations {
                    self.repo
                        .list_network_ospf_recommendations_for_plans(&plans)
                        .await?
                } else {
                    Vec::new()
                };
                enrich_runtime_tunnel_status_rows(&mut rows, &plans, &trends, &recommendations);
            }
            enrich_runtime_traffic_status_rows(&mut rows, &plans);
        }
        for row in &rows {
            ensure!(
                vpsman_common::is_source_readiness_status(&row.status),
                "source template status contract drift: {}",
                row.status
            );
        }
        Ok(rows)
    }

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
        let _ = self.events.send(event);
    }

    pub(crate) async fn terminal_job_status_after_refresh(
        &self,
        job_id: uuid::Uuid,
        refreshed: Option<String>,
    ) -> Result<Option<String>> {
        if let Some(status) = refreshed {
            return Ok(Some(status));
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

fn enrich_backup_status_rows(
    rows: &mut [SourceStatusView],
    store: Option<&BackupObjectStore>,
    counts_by_client: &HashMap<String, BackupSourceEvidenceCounts>,
) {
    for row in rows.iter_mut().filter(|row| {
        matches!(
            row.domain.as_str(),
            "backup_object_store" | "restore_path_mapping"
        )
    }) {
        let counts = counts_by_client
            .get(&row.client_id)
            .cloned()
            .unwrap_or_default();
        let runtime_evidence = json!({
            "workflow": "backup_artifacts",
            "restore_workflow": "restore_migration",
            "server_object_store_configured": store.is_some(),
            "server_object_store_kind": store.map(BackupObjectStore::kind),
            "artifact_count": counts.artifact_count,
            "backup_request_count": counts.backup_request_count,
            "restore_source_count": counts.restore_source_count,
            "restore_target_count": counts.restore_target_count,
            "migration_source_count": counts.migration_source_count,
            "migration_target_count": counts.migration_target_count,
            "continuous_status": false,
        });
        row.evidence = merge_evidence(row.evidence.take(), runtime_evidence);
        if row.status == "agent_offline" {
            continue;
        }
        if row.domain == "restore_path_mapping" {
            row.status = "ready_on_demand".to_string();
            row.status_reason =
                "restore path-mapping preset is selected; restore plans and migration links provide concrete mappings"
                    .to_string();
            continue;
        }
        if store.is_some() {
            row.status = "ready".to_string();
            row.status_reason =
                "backup object store is configured; backup artifacts can be uploaded".to_string();
        } else {
            row.status = "selected_no_store".to_string();
            row.status_reason =
                "backup object-store preset is selected, but no server object store is configured"
                    .to_string();
        }
    }
}

fn enrich_update_status_rows(rows: &mut [SourceStatusView], counts: UpdateSourceEvidenceCounts) {
    for row in rows.iter_mut().filter(|row| {
        matches!(
            row.domain.as_str(),
            "update_artifact_source" | "update_restart_policy" | "update_rollback_heartbeat_source"
        )
    }) {
        let runtime_evidence = json!({
            "workflow": "agent_update_releases",
            "release_count": counts.release_count,
            "external_release_count": counts.external_release_count,
            "continuous_status": false,
        });
        row.evidence = merge_evidence(row.evidence.take(), runtime_evidence);
        if row.status == "agent_offline" {
            continue;
        }
        if row.domain == "update_restart_policy" {
            row.status = "ready_on_demand".to_string();
            row.status_reason =
                "update restart policy is selected; activation and rollback jobs report restart evidence"
                    .to_string();
            continue;
        }
        if row.domain == "update_rollback_heartbeat_source" {
            row.status = "ready_on_demand".to_string();
            row.status_reason =
                "rollback heartbeat source is selected; activation and rollback jobs report heartbeat evidence"
                    .to_string();
            continue;
        }
        if counts.external_release_count > 0 {
            row.status = "ready".to_string();
            row.status_reason =
                "external HTTPS update release metadata exists; agents download update artifacts outside the API"
                    .to_string();
        } else if update_source_accepts_external_url(row) {
            row.status = "selected_no_artifacts".to_string();
            row.status_reason =
                "update artifact source is selected, but no external HTTPS release metadata exists"
                    .to_string();
        } else {
            row.status = "selected_no_artifacts".to_string();
            row.status_reason =
                "update artifact-source preset is selected, but no external artifact URL preset or release metadata exists"
                    .to_string();
        }
    }
}

fn update_source_accepts_external_url(row: &SourceStatusView) -> bool {
    matches!(
        row.source_kind.as_str(),
        "external_https" | "github_release"
    )
}

fn enrich_runtime_tunnel_status_rows(
    rows: &mut [SourceStatusView],
    plans: &[TunnelPlanView],
    trends: &[NetworkObservationTrendView],
    recommendations: &[NetworkOspfRecommendationView],
) {
    for row in rows.iter_mut().filter(|row| {
        matches!(
            row.domain.as_str(),
            "runtime_tunnel_adapter" | "routing_cost_adapter"
        )
    }) {
        let client_plans = plans
            .iter()
            .filter(|plan| tunnel_plan_touches_client(plan, &row.client_id))
            .collect::<Vec<_>>();
        let client_trends = trends
            .iter()
            .filter(|trend| network_trend_touches_client(trend, &row.client_id))
            .collect::<Vec<_>>();
        let observation_sample_count: i64 =
            client_trends.iter().map(|trend| trend.sample_count).sum();
        let degraded_observation_count: i64 =
            client_trends.iter().map(|trend| trend.degraded_count).sum();
        let network_status_sample_count: i64 = client_trends
            .iter()
            .filter(|trend| trend.kind == "network_status")
            .map(|trend| trend.sample_count)
            .sum();
        let probe_sample_count: i64 = client_trends
            .iter()
            .filter(|trend| trend.kind == "network_probe")
            .map(|trend| trend.sample_count)
            .sum();
        let speed_sample_count: i64 = client_trends
            .iter()
            .filter(|trend| trend.kind == "network_speed_test")
            .map(|trend| trend.sample_count)
            .sum();
        let template_id = row.template_id.to_string();
        let routing_plans = client_plans
            .iter()
            .copied()
            .filter(|plan| {
                let Some(ospf) = plan.plan.ospf.as_ref() else {
                    return false;
                };
                plan.enabled
                    && if plan.left_client_id == row.client_id {
                        ospf.left_adapter_template_id == template_id
                    } else {
                        ospf.right_adapter_template_id == template_id
                    }
            })
            .collect::<Vec<_>>();
        let routing_recommendation_count = recommendations
            .iter()
            .filter(|recommendation| {
                routing_plans
                    .iter()
                    .any(|plan| plan.id == recommendation.plan_id)
            })
            .count();
        let ospf_update_candidate_count = recommendations
            .iter()
            .filter(|recommendation| {
                routing_plans
                    .iter()
                    .any(|plan| plan.id == recommendation.plan_id)
                    && recommendation.cost_delta != 0
            })
            .count();
        let routing_endpoint_issue_count = routing_plans
            .iter()
            .filter(|plan| {
                matches!(
                    tunnel_endpoint_ospf_status(plan, &row.client_id),
                    "failed" | "stale"
                )
            })
            .count();
        let routing_verified_count = routing_plans
            .iter()
            .filter(|plan| tunnel_endpoint_ospf_status(plan, &row.client_id) == "verified")
            .count();
        let runtime_evidence = if row.domain == "routing_cost_adapter" {
            json!({
                "matching_routing_plan_count": routing_plans.len(),
                "routing_recommendation_count": routing_recommendation_count,
                "ospf_update_candidate_count": ospf_update_candidate_count,
                "routing_endpoint_issue_count": routing_endpoint_issue_count,
                "routing_verified_count": routing_verified_count,
                "routing_status_source": "tunnel_plan_endpoint_ospf_state",
                "continuous_status": false,
            })
        } else {
            json!({
                "network_status_sample_count": network_status_sample_count,
                "network_observation_sample_count": observation_sample_count,
                "network_observation_degraded_count": degraded_observation_count,
                "probe_sample_count": probe_sample_count,
                "speed_sample_count": speed_sample_count,
                "saved_plan_count": client_plans.len(),
                "continuous_status": true,
            })
        };
        row.evidence = merge_evidence(row.evidence.take(), runtime_evidence);
        if row.domain == "routing_cost_adapter" && row.status != "agent_offline" {
            row.status = if routing_endpoint_issue_count > 0 {
                "degraded".to_string()
            } else if routing_verified_count > 0 {
                "ready".to_string()
            } else {
                "ready_on_demand".to_string()
            };
            row.status_reason = match row.status.as_str() {
                "degraded" => format!(
                    "{routing_endpoint_issue_count} matching tunnel endpoint routing-cost state(s) are failed or stale"
                ),
                "ready" => {
                    "routing cost adapter has a verified matching tunnel endpoint state".to_string()
                }
                _ => {
                    "routing cost adapter is available on demand for explicit tunnel endpoint status and apply jobs"
                        .to_string()
                }
            };
        }
    }
}

fn enrich_runtime_traffic_status_rows(rows: &mut [SourceStatusView], plans: &[TunnelPlanView]) {
    for row in rows.iter_mut().filter(|row| {
        matches!(
            row.domain.as_str(),
            "runtime_traffic_accounting_source" | "traffic_limit_status_source"
        )
    }) {
        let client_plans = plans
            .iter()
            .filter(|plan| tunnel_plan_touches_client(plan, &row.client_id))
            .collect::<Vec<_>>();
        let traffic_limit_plan_count = client_plans
            .iter()
            .filter(|plan| tunnel_plan_has_traffic_limit(plan))
            .count();
        let traffic_limit_apply_plan_count = client_plans
            .iter()
            .filter(|plan| {
                tunnel_plan_has_traffic_limit(plan)
                    && plan.plan.runtime_control.manager
                        != vpsman_common::RuntimeTunnelManager::ExternalObserved
            })
            .count();
        let runtime_evidence = json!({
            "traffic_shaping_status_source": "tunnel_plan_runtime_control",
            "saved_plan_count": client_plans.len(),
            "traffic_limit_plan_count": traffic_limit_plan_count,
            "traffic_limit_apply_plan_count": traffic_limit_apply_plan_count,
            "continuous_status": true,
        });
        row.evidence = merge_evidence(row.evidence.take(), runtime_evidence);
        if row.domain == "traffic_limit_status_source" && row.status != "agent_offline" {
            row.status = if traffic_limit_plan_count > 0 {
                "ready".to_string()
            } else {
                "selected_no_limits".to_string()
            };
            row.status_reason = if traffic_limit_plan_count > 0 {
                "traffic-limit status source is selected and tunnel plans contain limit intent"
                    .to_string()
            } else {
                "traffic-limit status source is selected, but no tunnel traffic limits are planned"
                    .to_string()
            };
        }
    }
}

fn tunnel_plan_touches_client(plan: &TunnelPlanView, client_id: &str) -> bool {
    plan.left_client_id == client_id || plan.right_client_id == client_id
}

fn tunnel_endpoint_ospf_status<'a>(plan: &'a TunnelPlanView, client_id: &str) -> &'a str {
    if plan.left_client_id == client_id {
        &plan.left_ospf_status
    } else {
        &plan.right_ospf_status
    }
}

fn tunnel_plan_has_traffic_limit(plan: &TunnelPlanView) -> bool {
    !plan.plan.runtime_control.traffic_limit.is_default()
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn network_trend_touches_client(trend: &NetworkObservationTrendView, client_id: &str) -> bool {
    trend.client_id == client_id || trend.peer_client_id.as_deref() == Some(client_id)
}

fn merge_evidence(base: Value, extra: Value) -> Value {
    let mut merged = match base {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if let Value::Object(extra) = extra {
        for (key, value) in extra {
            merged.insert(key, value);
        }
    }
    Value::Object(merged)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;
    use vpsman_common::{
        plan_tunnel, OspfControlMode, OspfCostPolicy, TunnelKind, TunnelOspfConfig, TunnelPlanInput,
    };

    use crate::{
        model::{NetworkObservationTrendView, TunnelPlanEndpointRuntimeConfigView, TunnelPlanView},
        model_source_templates::SourceStatusView,
    };

    #[test]
    fn invalid_hot_reload_keeps_the_last_known_good_suite_config() {
        let path = std::env::temp_dir().join(format!(
            "vpsman-suite-config-last-known-good-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "version = 1\n\n[capacity]\ndispatcher_batch = 17\n").unwrap();
        let initial = super::load_suite_config_last_known_good(&path).unwrap();
        assert_eq!(initial.capacity.dispatcher_batch, Some(17));

        std::fs::remove_file(&path).unwrap();
        let missing_fallback = super::load_suite_config_last_known_good(&path).unwrap();
        assert_eq!(missing_fallback.capacity.dispatcher_batch, Some(17));

        std::fs::write(&path, "version = 1\n\n[capacity\n").unwrap();
        let fallback = super::load_suite_config_last_known_good(&path).unwrap();
        assert_eq!(fallback.capacity.dispatcher_batch, Some(17));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn routing_adapter_readiness_uses_only_matching_endpoint_state() {
        let matching_template_id = Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap();
        let mismatched_template_id =
            Uuid::parse_str("55555555-5555-4555-8555-555555555555").unwrap();
        let mut rows = vec![
            test_source_status(
                "routing_cost_adapter",
                matching_template_id,
                "ready_on_demand",
            ),
            test_source_status(
                "routing_cost_adapter",
                mismatched_template_id,
                "ready_on_demand",
            ),
            test_source_status(
                "runtime_tunnel_adapter",
                Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
                "ready",
            ),
        ];
        let plans = vec![test_tunnel_plan(matching_template_id, "failed")];
        let trends = vec![NetworkObservationTrendView {
            kind: "network_probe".to_string(),
            plan_id: Some(plans[0].id),
            topology_identity_hash: None,
            plan_name: Some(plans[0].name.clone()),
            interface_name: Some(plans[0].plan.interface_name.clone()),
            client_id: "edge-a".to_string(),
            peer_client_id: Some("edge-b".to_string()),
            sample_count: 1,
            healthy_count: 0,
            degraded_count: 1,
            latency_avg_ms: Some(500.0),
            latency_min_ms: Some(500.0),
            latency_max_ms: Some(500.0),
            packet_loss_avg_ratio: Some(1.0),
            throughput_avg_mbps: None,
            throughput_max_mbps: None,
            bytes_total: 0,
            latest_observed_at: "2026-07-26T00:00:00Z".to_string(),
        }];

        super::enrich_runtime_tunnel_status_rows(&mut rows, &plans, &trends, &[]);

        assert_eq!(rows[0].status, "degraded");
        assert_eq!(rows[0].evidence["routing_endpoint_issue_count"], 1);
        assert_eq!(rows[0].evidence["continuous_status"], false);
        assert!(
            rows[0]
                .evidence
                .get("network_observation_degraded_count")
                .is_none(),
            "routing evidence must not expose unrelated data-plane failures"
        );

        assert_eq!(rows[1].status, "ready_on_demand");
        assert_eq!(rows[1].evidence["matching_routing_plan_count"], 0);
        assert_eq!(rows[1].evidence["routing_endpoint_issue_count"], 0);
        assert_eq!(rows[1].evidence["continuous_status"], false);

        assert_eq!(rows[2].status, "ready");
        assert_eq!(rows[2].evidence["network_observation_degraded_count"], 1);
        assert_eq!(rows[2].evidence["continuous_status"], true);
    }

    #[test]
    fn source_status_network_enrichment_keeps_history_queries_off_default_traffic_polling() {
        let default_rows = vec![
            test_source_status(
                "runtime_traffic_accounting_source",
                Uuid::new_v4(),
                "selected_no_samples",
            ),
            test_source_status(
                "traffic_limit_status_source",
                Uuid::new_v4(),
                "ready_on_demand",
            ),
        ];
        assert_eq!(
            super::source_status_network_enrichment_needs(&default_rows),
            super::SourceStatusNetworkEnrichmentNeeds {
                tunnel_plans: true,
                observation_trends: false,
                ospf_recommendations: false,
            }
        );

        let adapter_rows = vec![
            test_source_status("runtime_tunnel_adapter", Uuid::new_v4(), "ready"),
            test_source_status("routing_cost_adapter", Uuid::new_v4(), "ready_on_demand"),
        ];
        assert_eq!(
            super::source_status_network_enrichment_needs(&adapter_rows),
            super::SourceStatusNetworkEnrichmentNeeds {
                tunnel_plans: true,
                observation_trends: true,
                ospf_recommendations: true,
            }
        );

        assert_eq!(
            super::source_status_network_enrichment_needs(&[test_source_status(
                "routing_cost_adapter",
                Uuid::new_v4(),
                "ready_on_demand",
            )]),
            super::SourceStatusNetworkEnrichmentNeeds {
                tunnel_plans: true,
                observation_trends: false,
                ospf_recommendations: true,
            }
        );
    }

    fn test_source_status(domain: &str, template_id: Uuid, status: &str) -> SourceStatusView {
        SourceStatusView {
            client_id: "edge-a".to_string(),
            display_name: "Edge A".to_string(),
            client_status: "online".to_string(),
            domain: domain.to_string(),
            module: domain.to_string(),
            template_id,
            template_name: format!("{domain} test"),
            template_scope: "global".to_string(),
            source_kind: "external".to_string(),
            status: status.to_string(),
            status_reason: "test base state".to_string(),
            evidence: json!({}),
            assigned_at: "2026-07-26T00:00:00Z".to_string(),
        }
    }

    fn test_tunnel_plan(left_adapter_template_id: Uuid, left_status: &str) -> TunnelPlanView {
        let input = TunnelPlanInput {
            name: "edge-a-edge-b".to_string(),
            interface_name: "tunab".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "edge-a".to_string(),
            right_client_id: "edge-b".to_string(),
            left_remote_underlay: "198.51.100.10".to_string(),
            left_local_underlay: None,
            right_remote_underlay: "203.0.113.20".to_string(),
            right_local_underlay: None,
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
                left: "10.255.0.0".to_string(),
                right: "10.255.0.1".to_string(),
                prefix_len: 31,
            }),
            ipv6_address_pool_cidr: None,
            ipv6_tunnel: None,
            latency_primary_family: Default::default(),
            bandwidth_mbps: 100,
            ospf: Some(TunnelOspfConfig {
                mode: OspfControlMode::Reviewed,
                planned_latency_ms: 20.0,
                planned_packet_loss_ratio: 0.01,
                preference: 1.0,
                policy: OspfCostPolicy::default(),
                min_cost_delta: 5,
                healthy_windows: 2,
                left_adapter_template_id: left_adapter_template_id.to_string(),
                right_adapter_template_id: "44444444-4444-4444-8444-444444444444".to_string(),
            }),
        };
        let plan = plan_tunnel(&input).unwrap();
        TunnelPlanView {
            id: Uuid::new_v4(),
            name: plan.name.clone(),
            kind: plan.kind,
            enabled: true,
            revision: 1,
            left_client_id: plan.left_client_id.clone(),
            right_client_id: plan.right_client_id.clone(),
            recommended_ospf_cost: plan.recommended_ospf_cost.map(i32::from),
            ospf_status: left_status.to_string(),
            left_ospf_status: left_status.to_string(),
            right_ospf_status: "unverified".to_string(),
            desired_ospf_cost: None,
            left_current_ospf_cost: None,
            right_current_ospf_cost: None,
            left_ospf_job_id: None,
            right_ospf_job_id: None,
            connection_assessment: "automatic".to_string(),
            connection_assessment_note: None,
            connection_assessed_at: None,
            connection_assessed_by: None,
            left_runtime_config: test_runtime_config("edge-a"),
            right_runtime_config: test_runtime_config("edge-b"),
            input,
            plan,
            created_at: "2026-07-26T00:00:00Z".to_string(),
            updated_at: "2026-07-26T00:00:00Z".to_string(),
            deleted_at: None,
            deleted_by: None,
            deleted_reason: None,
        }
    }

    fn test_runtime_config(client_id: &str) -> TunnelPlanEndpointRuntimeConfigView {
        TunnelPlanEndpointRuntimeConfigView {
            client_id: client_id.to_string(),
            desired: "present".to_string(),
            status: "not_dispatched".to_string(),
            job_id: None,
            error: None,
            updated_at: None,
        }
    }
}
