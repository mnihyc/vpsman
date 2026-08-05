use std::collections::{BTreeMap, HashMap, HashSet};

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use crate::{
    error::ApiError,
    fleet_alerts::{FleetAlertPolicy, FleetAlertSelector},
    model::{
        AgentView, BackupRequestStatus, BackupRequestView, DashboardAgentSummaryView,
        DashboardAlertSummaryView, DashboardAvailableFiltersView, DashboardDrilldownView,
        DashboardFilterOptionView, DashboardGroupByOptionView, DashboardLabelClusterView,
        DashboardNetworkClientView, DashboardNetworkPointView, DashboardNetworkView,
        DashboardOperationsView, DashboardOverviewView, DashboardResourceCurveView,
        DashboardResourcePointView, DashboardResourceSeriesView, DashboardResourcesView,
        DashboardScopeView, DashboardSummaryView, DashboardTimeRangeView,
        DashboardTrafficClientView, DashboardTrafficPointView, DashboardTrafficSeriesView,
        DashboardWindowOptionView, FleetAlertQuery, FleetAlertView, JobHistoryView,
        OperatorPreferences, TelemetryNetworkRateView, TelemetryRollupView,
    },
    model_alert_policies::NetworkRateInterfaceSelection,
    state::AppState,
    unix_now,
    util::timestamp_in_optional_bounds,
};

const DASHBOARD_LIMIT: i64 = 200;
const DASHBOARD_MIN_CHART_STEP_SECS: u64 = 60;
const DASHBOARD_DEFAULT_CHART_POINTS: u32 = 240;
const DASHBOARD_MAX_CHART_POINTS: u32 = 1_440;
const DASHBOARD_MAX_CUSTOM_RANGE_SECS: u64 = 365 * 24 * 60 * 60;
const DASHBOARD_TOP_CLUSTERS: usize = 8;
const DASHBOARD_TOP_ALERTS: usize = 5;
const DASHBOARD_TOP_DEGRADED: usize = 5;
const NETWORK_SNAPSHOT_COHERENCE_SECS: u64 = 180;
const DASHBOARD_MAX_NETWORK_POINTS: usize = 80;

#[derive(Debug, Deserialize)]
pub(crate) struct DashboardOverviewQuery {
    pub(crate) window: Option<String>,
    pub(crate) start_unix: Option<u64>,
    pub(crate) end_unix: Option<u64>,
    pub(crate) start_at: Option<String>,
    pub(crate) end_at: Option<String>,
    pub(crate) scope_kind: Option<String>,
    pub(crate) scope_value: Option<String>,
    pub(crate) group_by: Option<String>,
    pub(crate) resource_metric: Option<String>,
    pub(crate) chart_points: Option<u32>,
}

#[derive(Clone, Copy)]
struct DashboardWindow {
    label: &'static str,
    display_label: &'static str,
    seconds: Option<u64>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DashboardGroupBy {
    Labels,
    Tags,
    Countries,
    Providers,
    Clients,
    Status,
    Date,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DashboardScopeKind {
    All,
    Tag,
    Country,
    Provider,
    Client,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DashboardResourceMetric {
    CpuLoad,
    MemoryUsed,
    DiskFree,
}

struct DashboardScope {
    kind: DashboardScopeKind,
    value: Option<String>,
}

#[derive(Clone, Copy)]
struct DashboardRange {
    mode: &'static str,
    window: Option<DashboardWindow>,
    start_unix: u64,
    end_unix: u64,
}

#[derive(Default)]
struct NetworkClientAggregate {
    rx_bps: f64,
    tx_bps: f64,
    interfaces: HashSet<String>,
}

#[derive(Default)]
struct TrafficClientAggregate {
    rx_bytes: i64,
    tx_bytes: i64,
    interfaces: HashSet<String>,
    points: BTreeMap<u64, TrafficBucketAggregate>,
}

#[derive(Default)]
struct TrafficBucketAggregate {
    rx_bytes: i64,
    tx_bytes: i64,
}

#[derive(Default)]
struct NetworkBucketAggregate {
    rx_bps: f64,
    tx_bps: f64,
}

#[derive(Default)]
struct ResourceBucketAggregate {
    value_total: f64,
    value_count: usize,
    peak: Option<f64>,
}

struct ResourceClientSeries {
    agent: AgentView,
    points: Vec<DashboardResourcePointView>,
    current: Option<f64>,
    peak: Option<f64>,
    latest_sample_at: String,
    risk_score: f64,
    policy: FleetAlertPolicy,
}

struct DashboardGroupingContext<'a> {
    range: &'a DashboardRange,
    agents: &'a [AgentView],
    alerts: &'a [FleetAlertView],
    backups: &'a [BackupRequestView],
    running_jobs: &'a [JobHistoryView],
    network_rates: &'a [TelemetryNetworkRateView],
    alert_counts_by_client: &'a HashMap<String, usize>,
    running_job_targets: &'a HashMap<String, usize>,
    network_by_client: &'a HashMap<String, NetworkClientAggregate>,
    counts_truncated: bool,
}

impl DashboardGroupBy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Labels => "labels",
            Self::Tags => "tags",
            Self::Countries => "countries",
            Self::Providers => "providers",
            Self::Clients => "clients",
            Self::Status => "status",
            Self::Date => "date",
        }
    }
}

impl DashboardResourceMetric {
    fn as_str(self) -> &'static str {
        match self {
            Self::CpuLoad => "cpu_load",
            Self::MemoryUsed => "memory_used",
            Self::DiskFree => "disk_free",
        }
    }

    fn threshold_direction(self) -> &'static str {
        match self {
            Self::CpuLoad | Self::MemoryUsed => "above",
            Self::DiskFree => "below",
        }
    }

    fn value_for_rollup(self, rollup: &TelemetryRollupView) -> Option<f64> {
        match self {
            Self::CpuLoad => Some(rollup.cpu_load_1_avg.max(0.0)),
            Self::MemoryUsed => (rollup.memory_total_bytes_max > 0)
                .then_some(rollup.memory_used_ratio_avg.clamp(0.0, 1.0)),
            Self::DiskFree => (rollup.disk_total_bytes_max > 0)
                .then_some((1.0 - rollup.disk_used_ratio_avg).clamp(0.0, 1.0)),
        }
    }

    fn peak_for_rollup(self, rollup: &TelemetryRollupView) -> Option<f64> {
        match self {
            Self::CpuLoad => Some(rollup.cpu_load_1_max.max(rollup.cpu_load_1_avg).max(0.0)),
            Self::MemoryUsed => (rollup.memory_total_bytes_max > 0)
                .then_some(rollup.memory_used_ratio_max.clamp(0.0, 1.0)),
            Self::DiskFree => (rollup.disk_total_bytes_max > 0)
                .then_some((1.0 - rollup.disk_used_ratio_max).clamp(0.0, 1.0)),
        }
    }

    fn thresholds(self, policy: &FleetAlertPolicy) -> (Option<f64>, Option<f64>) {
        match self {
            Self::CpuLoad => (
                Some(policy.cpu_load_warning),
                Some(policy.cpu_load_critical),
            ),
            Self::MemoryUsed => (
                Some(1.0 - policy.memory_available_warning_ratio),
                Some(1.0 - policy.memory_available_critical_ratio),
            ),
            Self::DiskFree => (
                Some(policy.disk_available_warning_ratio),
                Some(policy.disk_available_critical_ratio),
            ),
        }
    }
}

impl DashboardScope {
    fn matches(&self, agent: &AgentView) -> bool {
        match self.kind {
            DashboardScopeKind::All => true,
            DashboardScopeKind::Tag => self
                .value
                .as_deref()
                .is_some_and(|value| agent.tags.iter().any(|tag| tag == value)),
            DashboardScopeKind::Country => self.value.as_deref().is_some_and(|value| {
                let expected = normalized_namespaced_value("country", value);
                agent.tags.iter().any(|tag| tag == &expected)
            }),
            DashboardScopeKind::Provider => self.value.as_deref().is_some_and(|value| {
                let expected = normalized_namespaced_value("provider", value);
                agent.tags.iter().any(|tag| tag == &expected)
            }),
            DashboardScopeKind::Client => self
                .value
                .as_deref()
                .is_some_and(|value| agent.id == value || agent.display_name == value),
        }
    }

    fn to_view(&self, matched_clients: usize) -> DashboardScopeView {
        let kind = self.kind.as_str().to_string();
        let value = self.value.clone();
        let query = self.query();
        DashboardScopeView {
            kind: kind.clone(),
            value: value.clone(),
            label: match (&self.kind, &value) {
                (DashboardScopeKind::All, _) => "All VPS".to_string(),
                (DashboardScopeKind::Country, Some(value)) => {
                    normalized_namespaced_value("country", value)
                }
                (DashboardScopeKind::Provider, Some(value)) => {
                    normalized_namespaced_value("provider", value)
                }
                (_, Some(value)) => value.clone(),
                _ => kind,
            },
            query,
            matched_clients,
        }
    }

    fn query(&self) -> Option<String> {
        match (self.kind, self.value.as_deref()) {
            (DashboardScopeKind::All, _) => None,
            (DashboardScopeKind::Tag, Some(value)) => Some(tag_query(value)),
            (DashboardScopeKind::Country, Some(value)) => {
                Some(normalized_namespaced_value("country", value))
            }
            (DashboardScopeKind::Provider, Some(value)) => {
                Some(normalized_namespaced_value("provider", value))
            }
            (DashboardScopeKind::Client, Some(value)) => Some(format!("id:{value}")),
            _ => None,
        }
    }
}

impl DashboardScopeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Tag => "tag",
            Self::Country => "country",
            Self::Provider => "provider",
            Self::Client => "client",
        }
    }
}

impl DashboardRange {
    fn to_view(self) -> DashboardTimeRangeView {
        DashboardTimeRangeView {
            mode: self.mode.to_string(),
            window: self.window.map(|window| window.label.to_string()),
            start_unix: self.start_unix,
            end_unix: self.end_unix,
            start_at: unix_to_rfc3339(self.start_unix),
            end_at: unix_to_rfc3339(self.end_unix),
        }
    }
}

pub(crate) async fn dashboard_overview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DashboardOverviewQuery>,
) -> Result<Json<DashboardOverviewView>, ApiError> {
    let now = unix_now();
    let range = validate_dashboard_range(&query, now)?;
    let group_by = validate_dashboard_group_by(query.group_by.as_deref())?;
    let resource_metric = validate_dashboard_resource_metric(query.resource_metric.as_deref())?;
    let scope = validate_dashboard_scope(&query)?;
    let operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let chart_points = validate_dashboard_chart_points(query.chart_points)?;
    Ok(Json(
        build_dashboard_overview(
            &state,
            range,
            scope,
            group_by,
            resource_metric,
            chart_points,
            now,
            &operator.operator.preferences,
        )
        .await?,
    ))
}

async fn build_dashboard_overview(
    state: &AppState,
    range: DashboardRange,
    scope: DashboardScope,
    group_by: DashboardGroupBy,
    resource_metric: DashboardResourceMetric,
    chart_points: u32,
    snapshot_unix: u64,
    preferences: &OperatorPreferences,
) -> Result<DashboardOverviewView, ApiError> {
    let now = snapshot_unix;
    let agents = state.repo.list_agents().await?;
    let available_filters = build_available_filters(&agents);
    let scoped_agents = agents
        .iter()
        .filter(|agent| scope.matches(agent))
        .cloned()
        .collect::<Vec<_>>();
    let scoped_client_ids = scoped_agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect::<HashSet<_>>();
    let mut scoped_client_id_list = scoped_client_ids.iter().cloned().collect::<Vec<_>>();
    scoped_client_id_list.sort();
    let agents_by_id = scoped_agents
        .iter()
        .map(|agent| (agent.id.clone(), agent.clone()))
        .collect::<HashMap<_, _>>();
    let alert_selection = state
        .list_fleet_alerts_selected(
            FleetAlertQuery {
                limit: Some(DASHBOARD_LIMIT),
                client_id: None,
                severity: None,
                category: None,
                operator_state: None,
                include_muted: Some(false),
            },
            Some(FleetAlertSelector {
                allowed_client_ids: &scoped_client_ids,
                start_unix: range.start_unix,
                end_unix: range.end_unix,
                snapshot_unix,
                include_global: scope.kind == DashboardScopeKind::All,
            }),
        )
        .await?;
    let alerts = alert_selection.alerts;
    let alerts_truncated = alert_selection.truncated;
    let telemetry_range = if range.mode == "all" {
        DashboardRange {
            start_unix: state
                .repo
                .dashboard_telemetry_start_unix(&scoped_client_id_list)
                .await?
                .unwrap_or(range.end_unix),
            ..range
        }
    } else {
        range
    };
    let chart_step_secs = dashboard_chart_step_secs(&telemetry_range, chart_points);
    let rollups = load_dashboard_rollups(
        state,
        &telemetry_range,
        chart_step_secs,
        chart_points,
        &scoped_client_id_list,
    )
    .await?;
    let network_rate_selection = state
        .repo
        .network_rate_interface_selection_for_clients(&scoped_client_id_list)
        .await?;
    let network_rates = load_dashboard_network_rates(
        state,
        &telemetry_range,
        chart_step_secs,
        chart_points,
        &network_rate_selection,
    )
    .await?;
    let mut running_jobs = state
        .repo
        .list_dashboard_running_jobs(&scoped_client_id_list, DASHBOARD_LIMIT + 1)
        .await?;
    let running_jobs_truncated = running_jobs.len() > DASHBOARD_LIMIT as usize;
    running_jobs.truncate(DASHBOARD_LIMIT as usize);
    let running_job_targets =
        running_job_targets_by_client(state, &running_jobs, &scoped_client_id_list).await?;
    let mut backup_requests = state
        .repo
        .list_dashboard_backup_requests(
            &scoped_client_id_list,
            range.start_unix,
            range.end_unix,
            DASHBOARD_LIMIT + 1,
        )
        .await?;
    let backups_truncated = backup_requests.len() > DASHBOARD_LIMIT as usize;
    backup_requests.truncate(DASHBOARD_LIMIT as usize);
    let latest_rollups = latest_rollups_by_client(&rollups);
    let latest_rates = coherent_latest_rates(latest_rates_by_client_interface(&network_rates));
    let latest_rates_by_client = network_by_client(latest_rates.values());
    let alert_counts_by_client = alert_counts_by_client(&alerts);
    let stale_agents = scoped_agents
        .iter()
        .filter(|agent| is_degraded_agent_status(&agent.status))
        .count();

    let effective_range = effective_dashboard_range(
        range,
        &rollups,
        &network_rates,
        &alerts,
        &backup_requests,
        &running_jobs,
    );
    let chart_step_secs = dashboard_chart_step_secs(&effective_range, chart_points);

    let operations = build_operations(
        &alerts,
        &scoped_agents,
        &agents_by_id,
        &backup_requests,
        running_jobs.len(),
        alerts_truncated,
        running_jobs_truncated,
        backups_truncated,
        effective_range,
    );
    let resources = build_resources(latest_rollups.values());
    let base_alert_policy = state.fleet_alert_policy();
    let resource_curve = build_resource_curve(ResourceCurveInput {
        metric: resource_metric,
        rollups: &rollups,
        agents: &scoped_agents,
        range: &effective_range,
        chart_step_secs,
        preferences,
        base_policy: &base_alert_policy,
    })?;
    let network = build_network(
        &network_rates,
        &latest_rates_by_client,
        &agents_by_id,
        preferences.dashboard_network_top_limit as usize,
        &effective_range,
        chart_step_secs,
    );
    let grouping_context = DashboardGroupingContext {
        range: &effective_range,
        agents: &scoped_agents,
        alerts: &alerts,
        backups: &backup_requests,
        running_jobs: &running_jobs,
        network_rates: &network_rates,
        alert_counts_by_client: &alert_counts_by_client,
        running_job_targets: &running_job_targets,
        network_by_client: &latest_rates_by_client,
        counts_truncated: alerts_truncated || running_jobs_truncated || backups_truncated,
    };
    let label_clusters = build_grouped_statistics(group_by, &grouping_context);

    Ok(DashboardOverviewView {
        window: range
            .window
            .map(|window| window.label.to_string())
            .unwrap_or_else(|| "custom".to_string()),
        generated_at: unix_to_rfc3339(now),
        group_by: group_by.as_str().to_string(),
        scope: scope.to_view(scoped_agents.len()),
        time_range: effective_range.to_view(),
        available_filters,
        summary: DashboardSummaryView {
            total: scoped_agents.len(),
            online: scoped_agents
                .iter()
                .filter(|agent| agent.status == "online")
                .count(),
            offline: scoped_agents
                .iter()
                .filter(|agent| {
                    matches!(agent.status.as_str(), "offline" | "disconnected" | "never")
                })
                .count(),
            revoked: scoped_agents
                .iter()
                .filter(|agent| agent.status == "revoked")
                .count(),
            stale: stale_agents,
            warnings: stale_agents.max(alerts.len()),
            warnings_truncated: alerts_truncated,
            running_jobs: running_jobs.len(),
            running_jobs_truncated,
        },
        operations,
        resources,
        resource_curve,
        network,
        label_clusters,
        drilldowns: vec![
            drilldown("Open fleet instances", "Fleet", "instances", None),
            drilldown("Review active alerts", "Fleet", "alerts", None),
            drilldown("Inspect topology evidence", "Topology", "evidence", None),
            drilldown("Review backups", "Backups", "requests", None),
            drilldown("Review job history", "Jobs", "history", None),
        ],
    })
}

fn validate_dashboard_range(
    query: &DashboardOverviewQuery,
    now: u64,
) -> Result<DashboardRange, ApiError> {
    let start = match (query.start_unix, query.start_at.as_deref()) {
        (Some(timestamp), _) => Some(timestamp),
        (None, Some(value)) => Some(
            parse_timestamp_unix(value)
                .ok_or_else(|| ApiError::bad_request("invalid_dashboard_start"))?,
        ),
        (None, None) => None,
    };
    let end = match (query.end_unix, query.end_at.as_deref()) {
        (Some(timestamp), _) => Some(timestamp),
        (None, Some(value)) => Some(
            parse_timestamp_unix(value)
                .ok_or_else(|| ApiError::bad_request("invalid_dashboard_end"))?,
        ),
        (None, None) => None,
    };

    if start.is_some() || end.is_some() {
        let start_unix = start.ok_or_else(|| ApiError::bad_request("missing_dashboard_start"))?;
        let end_unix = end.unwrap_or(now).min(now);
        if start_unix >= end_unix {
            return Err(ApiError::bad_request("invalid_dashboard_time_range"));
        }
        if end_unix.saturating_sub(start_unix) > DASHBOARD_MAX_CUSTOM_RANGE_SECS {
            return Err(ApiError::bad_request("dashboard_time_range_too_large"));
        }
        return Ok(DashboardRange {
            mode: "custom",
            window: None,
            start_unix,
            end_unix,
        });
    }

    let window = validate_dashboard_window(query.window.as_deref())?;
    let Some(seconds) = window.seconds else {
        return Ok(DashboardRange {
            mode: "all",
            window: Some(window),
            start_unix: 0,
            end_unix: now,
        });
    };
    Ok(DashboardRange {
        mode: "window",
        window: Some(window),
        start_unix: now.saturating_sub(seconds),
        end_unix: now,
    })
}

fn validate_dashboard_window(value: Option<&str>) -> Result<DashboardWindow, ApiError> {
    let requested = value.unwrap_or("1d").trim();
    dashboard_windows()
        .into_iter()
        .find(|window| window.label == requested)
        .ok_or_else(|| ApiError::bad_request("invalid_dashboard_window"))
}

fn validate_dashboard_group_by(value: Option<&str>) -> Result<DashboardGroupBy, ApiError> {
    match value.unwrap_or("labels").trim() {
        "labels" => Ok(DashboardGroupBy::Labels),
        "tags" => Ok(DashboardGroupBy::Tags),
        "countries" | "country" => Ok(DashboardGroupBy::Countries),
        "providers" | "provider" => Ok(DashboardGroupBy::Providers),
        "clients" | "vps" => Ok(DashboardGroupBy::Clients),
        "status" => Ok(DashboardGroupBy::Status),
        "date" | "time" => Ok(DashboardGroupBy::Date),
        _ => Err(ApiError::bad_request("invalid_dashboard_group_by")),
    }
}

fn validate_dashboard_resource_metric(
    value: Option<&str>,
) -> Result<DashboardResourceMetric, ApiError> {
    match value.unwrap_or("cpu_load").trim() {
        "" | "cpu" | "cpu_load" => Ok(DashboardResourceMetric::CpuLoad),
        "memory" | "memory_used" => Ok(DashboardResourceMetric::MemoryUsed),
        "disk" | "disk_free" => Ok(DashboardResourceMetric::DiskFree),
        _ => Err(ApiError::bad_request("invalid_dashboard_resource_metric")),
    }
}

fn validate_dashboard_scope(query: &DashboardOverviewQuery) -> Result<DashboardScope, ApiError> {
    let kind = match query.scope_kind.as_deref().map(str::trim).unwrap_or("all") {
        "" | "all" => DashboardScopeKind::All,
        "tag" | "tags" => DashboardScopeKind::Tag,
        "country" | "countries" => DashboardScopeKind::Country,
        "provider" | "providers" => DashboardScopeKind::Provider,
        "client" | "vps" => DashboardScopeKind::Client,
        _ => return Err(ApiError::bad_request("invalid_dashboard_scope")),
    };
    let value = query
        .scope_value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    if kind != DashboardScopeKind::All && value.is_none() {
        return Err(ApiError::bad_request("missing_dashboard_scope_value"));
    }
    Ok(DashboardScope { kind, value })
}

fn validate_dashboard_chart_points(value: Option<u32>) -> Result<u32, ApiError> {
    let Some(points) = value else {
        return Ok(DASHBOARD_DEFAULT_CHART_POINTS);
    };
    if points == 0 {
        return Err(ApiError::bad_request("invalid_dashboard_chart_points"));
    }
    Ok(points.clamp(2, DASHBOARD_MAX_CHART_POINTS))
}

fn dashboard_windows() -> Vec<DashboardWindow> {
    vec![
        DashboardWindow {
            label: "15m",
            display_label: "Realtime · last 15 minutes",
            seconds: Some(15 * 60),
        },
        DashboardWindow {
            label: "1h",
            display_label: "1 hour",
            seconds: Some(60 * 60),
        },
        DashboardWindow {
            label: "8h",
            display_label: "8 hours",
            seconds: Some(8 * 60 * 60),
        },
        DashboardWindow {
            label: "1d",
            display_label: "1 day",
            seconds: Some(24 * 60 * 60),
        },
        DashboardWindow {
            label: "7d",
            display_label: "7 days",
            seconds: Some(7 * 24 * 60 * 60),
        },
        DashboardWindow {
            label: "30d",
            display_label: "30 days",
            seconds: Some(30 * 24 * 60 * 60),
        },
        DashboardWindow {
            label: "90d",
            display_label: "90 days",
            seconds: Some(90 * 24 * 60 * 60),
        },
        DashboardWindow {
            label: "180d",
            display_label: "180 days",
            seconds: Some(180 * 24 * 60 * 60),
        },
        DashboardWindow {
            label: "1y",
            display_label: "1 year",
            seconds: Some(365 * 24 * 60 * 60),
        },
        DashboardWindow {
            label: "all",
            display_label: "All",
            seconds: None,
        },
    ]
}

fn build_available_filters(agents: &[AgentView]) -> DashboardAvailableFiltersView {
    let mut providers = BTreeMap::<String, usize>::new();
    let mut countries = BTreeMap::<String, usize>::new();
    let mut tags = BTreeMap::<String, usize>::new();
    for agent in agents {
        for tag in &agent.tags {
            if let Some(value) = tag
                .strip_prefix("provider:")
                .filter(|value| !value.is_empty())
            {
                *providers.entry(value.to_string()).or_default() += 1;
            } else if let Some(value) = tag
                .strip_prefix("country:")
                .filter(|value| !value.is_empty())
            {
                *countries.entry(value.to_string()).or_default() += 1;
            } else {
                *tags.entry(tag.clone()).or_default() += 1;
            }
        }
    }

    DashboardAvailableFiltersView {
        windows: dashboard_windows()
            .into_iter()
            .map(|window| DashboardWindowOptionView {
                value: window.label.to_string(),
                label: window.display_label.to_string(),
                seconds: window.seconds.unwrap_or(0),
            })
            .collect(),
        group_by_options: dashboard_group_options(),
        providers: providers
            .into_iter()
            .map(|(value, count)| DashboardFilterOptionView {
                kind: "provider".to_string(),
                label: format!("provider:{value}"),
                query: format!("provider:{value}"),
                value,
                count,
            })
            .collect(),
        countries: countries
            .into_iter()
            .map(|(value, count)| DashboardFilterOptionView {
                kind: "country".to_string(),
                label: format!("country:{value}"),
                query: format!("country:{value}"),
                value,
                count,
            })
            .collect(),
        tags: tags
            .into_iter()
            .map(|(value, count)| DashboardFilterOptionView {
                kind: "tag".to_string(),
                label: value.clone(),
                query: tag_query(&value),
                value,
                count,
            })
            .collect(),
    }
}

fn dashboard_group_options() -> Vec<DashboardGroupByOptionView> {
    [
        (
            "labels",
            "Labels",
            "Provider, country, and custom tags together",
        ),
        ("tags", "Custom tags", "Non-provider and non-country tags"),
        ("countries", "Countries", "country:* tag distribution"),
        ("providers", "Providers", "provider:* tag distribution"),
        (
            "clients",
            "VPS clients",
            "One group per VPS in the selected scope",
        ),
        (
            "status",
            "Status",
            "Online, offline, and stale client states",
        ),
        (
            "date",
            "Date buckets",
            "Time buckets across the selected range",
        ),
    ]
    .into_iter()
    .map(|(value, label, description)| DashboardGroupByOptionView {
        value: value.to_string(),
        label: label.to_string(),
        description: description.to_string(),
    })
    .collect()
}

async fn load_dashboard_rollups(
    state: &AppState,
    range: &DashboardRange,
    chart_step_secs: u64,
    chart_points: u32,
    client_ids: &[String],
) -> Result<Vec<TelemetryRollupView>, ApiError> {
    if dashboard_uses_raw_samples(range) {
        return Ok(state
            .repo
            .list_dashboard_raw_telemetry_rollups(
                i64::from(chart_points),
                range.start_unix,
                range.end_unix,
                chart_step_secs as i32,
                client_ids,
            )
            .await?);
    }
    let bounded_range = telemetry_query_bounds(range);
    Ok(state
        .repo
        .list_dashboard_telemetry_rollups(
            i64::from(chart_points),
            bounded_range.0,
            bounded_range.1,
            None,
            chart_step_secs as i32,
            client_ids,
        )
        .await?)
}

async fn load_dashboard_network_rates(
    state: &AppState,
    range: &DashboardRange,
    chart_step_secs: u64,
    chart_points: u32,
    selection: &NetworkRateInterfaceSelection,
) -> Result<Vec<TelemetryNetworkRateView>, ApiError> {
    if dashboard_uses_raw_samples(range) {
        return Ok(state
            .repo
            .list_dashboard_raw_telemetry_network_rates_selected(
                i64::from(chart_points),
                range.start_unix,
                range.end_unix,
                chart_step_secs as i32,
                selection,
            )
            .await?);
    }
    let bounded_range = telemetry_query_bounds(range);
    Ok(state
        .repo
        .list_dashboard_telemetry_network_rates_selected(
            i64::from(chart_points),
            bounded_range.0,
            bounded_range.1,
            None,
            chart_step_secs as i32,
            selection,
        )
        .await?)
}

fn dashboard_uses_raw_samples(range: &DashboardRange) -> bool {
    range.window.is_some_and(|window| window.label == "15m")
}

fn telemetry_query_bounds(range: &DashboardRange) -> (Option<u64>, Option<u64>) {
    if range.mode == "all" && range.start_unix == 0 {
        (None, Some(range.end_unix))
    } else {
        (Some(range.start_unix), Some(range.end_unix))
    }
}

fn dashboard_chart_step_secs(range: &DashboardRange, chart_points: u32) -> u64 {
    let span = range.end_unix.saturating_sub(range.start_unix);
    let points = u64::from(chart_points.clamp(2, DASHBOARD_MAX_CHART_POINTS));
    let intervals = points.saturating_sub(1);
    let raw_step = span.saturating_add(intervals.saturating_sub(1)) / intervals;
    round_up_to_minute(raw_step.max(DASHBOARD_MIN_CHART_STEP_SECS))
}

fn round_up_to_minute(value: u64) -> u64 {
    let minute = DASHBOARD_MIN_CHART_STEP_SECS;
    value.saturating_add(minute - 1) / minute * minute
}

fn effective_dashboard_range(
    range: DashboardRange,
    rollups: &[TelemetryRollupView],
    network_rates: &[TelemetryNetworkRateView],
    alerts: &[FleetAlertView],
    backups: &[BackupRequestView],
    jobs: &[JobHistoryView],
) -> DashboardRange {
    if range.mode != "all" {
        return range;
    }

    let earliest = rollups
        .iter()
        .filter_map(|rollup| parse_timestamp_unix(&rollup.bucket_start))
        .chain(
            network_rates
                .iter()
                .filter_map(|rate| parse_timestamp_unix(&rate.bucket_start)),
        )
        .chain(
            alerts
                .iter()
                .filter_map(|alert| parse_timestamp_unix(&alert.observed_at)),
        )
        .chain(
            backups
                .iter()
                .filter_map(|backup| parse_timestamp_unix(&backup.created_at)),
        )
        .chain(
            jobs.iter()
                .filter_map(|job| parse_timestamp_unix(&job.created_at)),
        )
        .min()
        .unwrap_or(range.end_unix);

    DashboardRange {
        start_unix: earliest.min(range.end_unix),
        ..range
    }
}

async fn running_job_targets_by_client(
    state: &AppState,
    jobs: &[JobHistoryView],
    client_ids: &[String],
) -> Result<HashMap<String, usize>, ApiError> {
    let mut counts = HashMap::new();
    let job_ids = jobs.iter().map(|job| job.id).collect::<Vec<_>>();
    for target in state
        .repo
        .list_dashboard_job_targets(&job_ids, client_ids)
        .await?
    {
        *counts.entry(target.client_id).or_insert(0) += 1;
    }
    Ok(counts)
}

fn build_operations(
    alerts: &[FleetAlertView],
    agents: &[AgentView],
    agents_by_id: &HashMap<String, AgentView>,
    backups: &[BackupRequestView],
    running_jobs: usize,
    alerts_truncated: bool,
    running_jobs_truncated: bool,
    backups_truncated: bool,
    range: DashboardRange,
) -> DashboardOperationsView {
    let backup_window = backups
        .iter()
        .filter(|backup| timestamp_in_range(&backup.created_at, &range))
        .collect::<Vec<_>>();
    let recent_alerts = recent_alerts(alerts, agents_by_id);
    let degraded_agents = agents
        .iter()
        .filter(|agent| is_degraded_agent_status(&agent.status))
        .take(DASHBOARD_TOP_DEGRADED)
        .map(|agent| DashboardAgentSummaryView {
            client_id: agent.id.clone(),
            label: agent.display_name.clone(),
            status: agent.status.clone(),
            tags: agent.tags.clone(),
            drilldown: client_drilldown(&agent.id),
        })
        .collect();

    DashboardOperationsView {
        active_alerts: alerts.len(),
        critical_alerts: alerts
            .iter()
            .filter(|alert| alert.severity == "critical")
            .count(),
        warning_alerts: alerts
            .iter()
            .filter(|alert| alert.severity == "warning")
            .count(),
        stale_agents: agents
            .iter()
            .filter(|agent| is_degraded_agent_status(&agent.status))
            .count(),
        running_jobs,
        backup_pending: backup_window
            .iter()
            .filter(|backup| backup.status == BackupRequestStatus::RequestedMetadataOnly.as_str())
            .count(),
        backup_completed: backup_window
            .iter()
            .filter(|backup| {
                backup.status == BackupRequestStatus::ArtifactMetadataRecorded.as_str()
            })
            .count(),
        backup_failed: backup_window
            .iter()
            .filter(|backup| {
                let status = backup.status.to_ascii_lowercase();
                status.contains("failed") || status.contains("error")
            })
            .count(),
        alerts_truncated,
        running_jobs_truncated,
        backups_truncated,
        recent_alerts,
        degraded_agents,
    }
}

fn build_resources<'a>(
    rollups: impl Iterator<Item = &'a TelemetryRollupView>,
) -> DashboardResourcesView {
    let mut sampled_clients = 0_usize;
    let mut cpu_total = 0.0_f64;
    let mut cpu_max: Option<f64> = None;
    let mut memory_total = 0_i128;
    let mut memory_used_weighted = 0.0_f64;
    let mut disk_total = 0_i128;
    let mut disk_used_weighted = 0.0_f64;

    for rollup in rollups {
        sampled_clients += 1;
        cpu_total += rollup.cpu_load_1_avg.max(0.0);
        cpu_max = Some(
            cpu_max
                .unwrap_or(rollup.cpu_load_1_max)
                .max(rollup.cpu_load_1_max),
        );
        memory_total += rollup.memory_total_bytes_max.max(0) as i128;
        memory_used_weighted += rollup.memory_used_ratio_avg.clamp(0.0, 1.0)
            * rollup.memory_total_bytes_max.max(0) as f64;
        disk_total += rollup.disk_total_bytes_max.max(0) as i128;
        disk_used_weighted +=
            rollup.disk_used_ratio_avg.clamp(0.0, 1.0) * rollup.disk_total_bytes_max.max(0) as f64;
    }

    DashboardResourcesView {
        sampled_clients,
        cpu_load_avg: (sampled_clients > 0).then_some(cpu_total / sampled_clients as f64),
        cpu_load_max: cpu_max,
        memory_used_ratio: (memory_total > 0)
            .then_some((memory_used_weighted / memory_total as f64).clamp(0.0, 1.0)),
        disk_free_ratio: (disk_total > 0)
            .then_some((1.0 - disk_used_weighted / disk_total as f64).clamp(0.0, 1.0)),
    }
}

struct ResourceCurveInput<'a> {
    metric: DashboardResourceMetric,
    rollups: &'a [TelemetryRollupView],
    agents: &'a [AgentView],
    range: &'a DashboardRange,
    chart_step_secs: u64,
    preferences: &'a OperatorPreferences,
    base_policy: &'a FleetAlertPolicy,
}

fn build_resource_curve(
    input: ResourceCurveInput<'_>,
) -> Result<DashboardResourceCurveView, ApiError> {
    let ResourceCurveInput {
        metric,
        rollups,
        agents,
        range,
        chart_step_secs,
        preferences,
        base_policy,
    } = input;
    let exclusions = &preferences.dashboard_curve_exclusions;
    let excluded_clients = agents
        .iter()
        .filter(|agent| {
            exclusions
                .iter()
                .any(|selector| agent_matches_curve_exclusion(agent, selector))
        })
        .count();
    let mut rollups_by_client = HashMap::<String, Vec<&TelemetryRollupView>>::new();
    for rollup in rollups {
        rollups_by_client
            .entry(rollup.client_id.clone())
            .or_default()
            .push(rollup);
    }
    let mut candidates = Vec::new();

    for agent in agents {
        if exclusions
            .iter()
            .any(|selector| agent_matches_curve_exclusion(agent, selector))
        {
            continue;
        }
        let Some(client_rollups) = rollups_by_client.get(&agent.id) else {
            continue;
        };
        let mut client_rollups = client_rollups.clone();
        client_rollups.sort_by(|left, right| {
            timestamp_sort_key(&left.bucket_start)
                .cmp(&timestamp_sort_key(&right.bucket_start))
                .then_with(|| left.latest_observed_at.cmp(&right.latest_observed_at))
        });
        let latest_sample_at = client_rollups
            .iter()
            .max_by_key(|rollup| timestamp_sort_key(&rollup.latest_observed_at))
            .map(|rollup| rollup.latest_observed_at.clone())
            .unwrap_or_default();

        let mut points_by_bucket = BTreeMap::<u64, ResourceBucketAggregate>::new();
        let mut current = None;
        for rollup in &client_rollups {
            let bucket = chart_bucket(&rollup.bucket_start, range, chart_step_secs);
            let entry = points_by_bucket.entry(bucket).or_default();
            if let Some(value) = metric.value_for_rollup(rollup) {
                entry.value_total += value;
                entry.value_count += 1;
                current = Some(value);
            }
            if let Some(peak) = metric.peak_for_rollup(rollup) {
                entry.peak = match (metric, entry.peak) {
                    (DashboardResourceMetric::DiskFree, Some(existing)) => Some(existing.min(peak)),
                    (DashboardResourceMetric::DiskFree, None) => Some(peak),
                    (_, Some(existing)) => Some(existing.max(peak)),
                    (_, None) => Some(peak),
                };
            }
        }
        let points = points_by_bucket
            .iter()
            .map(|(bucket_start, aggregate)| DashboardResourcePointView {
                bucket_start: unix_to_rfc3339(*bucket_start),
                value: (aggregate.value_count > 0)
                    .then_some(aggregate.value_total / aggregate.value_count as f64),
            })
            .collect::<Vec<_>>();
        if points.iter().all(|point| point.value.is_none()) {
            continue;
        }

        let peak_values = points_by_bucket
            .values()
            .filter_map(|aggregate| aggregate.peak)
            .collect::<Vec<_>>();
        let peak = match metric {
            DashboardResourceMetric::DiskFree => peak_values.into_iter().reduce(f64::min),
            DashboardResourceMetric::CpuLoad | DashboardResourceMetric::MemoryUsed => {
                peak_values.into_iter().reduce(f64::max)
            }
        };
        let risk_score = match metric {
            DashboardResourceMetric::DiskFree => peak.map(|value| 1.0 - value).unwrap_or(0.0),
            DashboardResourceMetric::CpuLoad | DashboardResourceMetric::MemoryUsed => {
                peak.unwrap_or(0.0)
            }
        };
        candidates.push(ResourceClientSeries {
            agent: agent.clone(),
            points,
            current,
            peak,
            latest_sample_at,
            risk_score,
            policy: base_policy.clone(),
        });
    }

    candidates.sort_by(|left, right| {
        right
            .risk_score
            .total_cmp(&left.risk_score)
            .then_with(|| left.agent.display_name.cmp(&right.agent.display_name))
    });
    let sampled_clients = candidates.len();
    let latest_sample_at = candidates
        .iter()
        .map(|candidate| candidate.latest_sample_at.as_str())
        .max_by_key(|value| timestamp_sort_key(value))
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let top_limit = preferences.dashboard_resource_top_limit as usize;
    let series = candidates
        .into_iter()
        .take(top_limit)
        .map(|candidate| {
            let (warning_threshold, critical_threshold) = metric.thresholds(&candidate.policy);
            let client_id = candidate.agent.id.clone();
            DashboardResourceSeriesView {
                client_id: client_id.clone(),
                label: candidate.agent.display_name,
                current: candidate.current,
                peak: candidate.peak,
                warning_threshold,
                critical_threshold,
                threshold_direction: metric.threshold_direction().to_string(),
                points: candidate.points,
                drilldown: client_drilldown(&client_id),
            }
        })
        .collect();

    Ok(DashboardResourceCurveView {
        metric: metric.as_str().to_string(),
        sampled_clients,
        excluded_clients,
        top_limit,
        latest_sample_at,
        series,
    })
}

fn build_network(
    rates: &[TelemetryNetworkRateView],
    latest_rates_by_client: &HashMap<String, NetworkClientAggregate>,
    agents_by_id: &HashMap<String, AgentView>,
    top_limit: usize,
    range: &DashboardRange,
    chart_step_secs: u64,
) -> DashboardNetworkView {
    let mut speed_by_step = BTreeMap::<u64, BTreeMap<String, NetworkBucketAggregate>>::new();
    let mut traffic_by_step = BTreeMap::<u64, TrafficBucketAggregate>::new();
    let mut traffic_by_client = HashMap::<String, TrafficClientAggregate>::new();
    for rate in rates {
        let bucket = chart_bucket(&rate.bucket_start, range, chart_step_secs);
        let raw_bucket_key = rate.bucket_start.clone();
        let speed_entry = speed_by_step
            .entry(bucket)
            .or_default()
            .entry(raw_bucket_key)
            .or_default();
        speed_entry.rx_bps += rate.rx_bps_avg.max(0.0);
        speed_entry.tx_bps += rate.tx_bps_avg.max(0.0);

        let rx_bytes = rate.rx_bytes_delta.max(0);
        let tx_bytes = rate.tx_bytes_delta.max(0);
        let traffic_entry = traffic_by_step.entry(bucket).or_default();
        traffic_entry.rx_bytes += rx_bytes;
        traffic_entry.tx_bytes += tx_bytes;

        let client_entry = traffic_by_client.entry(rate.client_id.clone()).or_default();
        client_entry.rx_bytes += rx_bytes;
        client_entry.tx_bytes += tx_bytes;
        client_entry.interfaces.insert(rate.interface.clone());
        let client_bucket = client_entry.points.entry(bucket).or_default();
        client_bucket.rx_bytes += rx_bytes;
        client_bucket.tx_bytes += tx_bytes;
    }
    let mut points = speed_by_step
        .into_iter()
        .map(|(bucket_start, raw_buckets)| {
            let sample_count = raw_buckets.len().max(1) as f64;
            let (rx_bps, tx_bps) = raw_buckets
                .values()
                .fold((0.0, 0.0), |(rx_total, tx_total), bucket| {
                    (rx_total + bucket.rx_bps, tx_total + bucket.tx_bps)
                });
            DashboardNetworkPointView {
                bucket_start: unix_to_rfc3339(bucket_start),
                rx_bps: rx_bps / sample_count,
                tx_bps: tx_bps / sample_count,
            }
        })
        .collect::<Vec<_>>();
    if points.len() > DASHBOARD_MAX_NETWORK_POINTS {
        points.drain(0..points.len() - DASHBOARD_MAX_NETWORK_POINTS);
    }
    let mut traffic_points = traffic_by_step
        .into_iter()
        .map(|(bucket_start, traffic)| DashboardTrafficPointView {
            bucket_start: unix_to_rfc3339(bucket_start),
            rx_bytes: traffic.rx_bytes,
            tx_bytes: traffic.tx_bytes,
        })
        .collect::<Vec<_>>();
    if traffic_points.len() > DASHBOARD_MAX_NETWORK_POINTS {
        traffic_points.drain(0..traffic_points.len() - DASHBOARD_MAX_NETWORK_POINTS);
    }

    let rx_bps = latest_rates_by_client
        .values()
        .map(|client| client.rx_bps)
        .sum();
    let tx_bps = latest_rates_by_client
        .values()
        .map(|client| client.tx_bps)
        .sum();
    let mut top_clients = latest_rates_by_client
        .iter()
        .map(|(client_id, rate)| DashboardNetworkClientView {
            client_id: client_id.clone(),
            label: agents_by_id
                .get(client_id)
                .map(|agent| agent.display_name.clone())
                .unwrap_or_else(|| client_id.clone()),
            rx_bps: rate.rx_bps,
            tx_bps: rate.tx_bps,
            interfaces: {
                let mut interfaces = rate.interfaces.iter().cloned().collect::<Vec<_>>();
                interfaces.sort();
                interfaces
            },
            drilldown: client_drilldown(client_id),
        })
        .collect::<Vec<_>>();
    top_clients.sort_by(|left, right| {
        total_bps(right)
            .total_cmp(&total_bps(left))
            .then_with(|| left.label.cmp(&right.label))
    });
    top_clients.truncate(top_limit);
    let mut traffic_series = build_traffic_series(traffic_by_client, agents_by_id);
    traffic_series.truncate(top_limit);
    let traffic_top_clients = traffic_series
        .iter()
        .map(|series| DashboardTrafficClientView {
            client_id: series.client_id.clone(),
            label: series.label.clone(),
            rx_bytes: series.rx_bytes,
            tx_bytes: series.tx_bytes,
            interfaces: series.interfaces.clone(),
            drilldown: series.drilldown.clone(),
        })
        .collect::<Vec<_>>();

    DashboardNetworkView {
        rx_bps,
        tx_bps,
        points,
        traffic_points,
        top_clients,
        traffic_top_clients,
        traffic_series,
    }
}

fn build_traffic_series(
    traffic_by_client: HashMap<String, TrafficClientAggregate>,
    agents_by_id: &HashMap<String, AgentView>,
) -> Vec<DashboardTrafficSeriesView> {
    let mut series = traffic_by_client
        .into_iter()
        .map(|(client_id, traffic)| {
            let mut interfaces = traffic.interfaces.into_iter().collect::<Vec<_>>();
            interfaces.sort();
            let points = traffic
                .points
                .into_iter()
                .map(|(bucket_start, point)| DashboardTrafficPointView {
                    bucket_start: unix_to_rfc3339(bucket_start),
                    rx_bytes: point.rx_bytes,
                    tx_bytes: point.tx_bytes,
                })
                .collect::<Vec<_>>();
            DashboardTrafficSeriesView {
                label: agents_by_id
                    .get(&client_id)
                    .map(|agent| agent.display_name.clone())
                    .unwrap_or_else(|| client_id.clone()),
                drilldown: client_drilldown(&client_id),
                client_id,
                rx_bytes: traffic.rx_bytes,
                tx_bytes: traffic.tx_bytes,
                interfaces,
                points,
            }
        })
        .collect::<Vec<_>>();
    series.sort_by(|left, right| {
        right
            .rx_bytes
            .saturating_add(right.tx_bytes)
            .cmp(&left.rx_bytes.saturating_add(left.tx_bytes))
            .then_with(|| left.label.cmp(&right.label))
    });
    series
}

fn build_grouped_statistics(
    group_by: DashboardGroupBy,
    context: &DashboardGroupingContext<'_>,
) -> Vec<DashboardLabelClusterView> {
    if group_by == DashboardGroupBy::Date {
        return build_date_groups(
            context.range,
            context.alerts,
            context.backups,
            context.running_jobs,
            context.network_rates,
            context.counts_truncated,
        );
    }

    let mut clients_by_tag = BTreeMap::<String, Vec<&AgentView>>::new();
    match group_by {
        DashboardGroupBy::Labels => {
            for agent in context.agents {
                for tag in &agent.tags {
                    clients_by_tag.entry(tag.clone()).or_default().push(agent);
                }
            }
        }
        DashboardGroupBy::Tags => {
            for agent in context.agents {
                for tag in agent
                    .tags
                    .iter()
                    .filter(|tag| !tag.starts_with("provider:") && !tag.starts_with("country:"))
                {
                    clients_by_tag.entry(tag.clone()).or_default().push(agent);
                }
            }
        }
        DashboardGroupBy::Countries => {
            for agent in context.agents {
                for tag in agent.tags.iter().filter(|tag| tag.starts_with("country:")) {
                    clients_by_tag.entry(tag.clone()).or_default().push(agent);
                }
            }
        }
        DashboardGroupBy::Providers => {
            for agent in context.agents {
                for tag in agent.tags.iter().filter(|tag| tag.starts_with("provider:")) {
                    clients_by_tag.entry(tag.clone()).or_default().push(agent);
                }
            }
        }
        DashboardGroupBy::Clients => {
            return build_client_groups(
                context.agents,
                context.alert_counts_by_client,
                context.running_job_targets,
                context.network_by_client,
                context.counts_truncated,
            );
        }
        DashboardGroupBy::Status => {
            return build_status_groups(
                context.agents,
                context.alert_counts_by_client,
                context.running_job_targets,
                context.network_by_client,
                context.counts_truncated,
            );
        }
        DashboardGroupBy::Date => unreachable!(),
    }

    let mut clusters = clients_by_tag
        .into_iter()
        .map(|(tag, clients)| {
            cluster_for_agents(
                tag.clone(),
                tag_kind(&tag).to_string(),
                Some(tag_query(&tag)),
                clients,
                context.alert_counts_by_client,
                context.running_job_targets,
                context.network_by_client,
                context.counts_truncated,
            )
        })
        .collect::<Vec<_>>();
    clusters.sort_by(|left, right| {
        right
            .warnings
            .cmp(&left.warnings)
            .then_with(|| right.stale.cmp(&left.stale))
            .then_with(|| right.running_jobs.cmp(&left.running_jobs))
            .then_with(|| right.total.cmp(&left.total))
            .then_with(|| left.label.cmp(&right.label))
    });
    clusters.truncate(DASHBOARD_TOP_CLUSTERS);

    if group_by == DashboardGroupBy::Labels {
        clusters.push(cluster_for_agents(
            "All VPS".to_string(),
            "all".to_string(),
            None,
            context.agents.iter().collect::<Vec<_>>(),
            context.alert_counts_by_client,
            context.running_job_targets,
            context.network_by_client,
            context.counts_truncated,
        ));
    }
    clusters
}

fn build_client_groups(
    agents: &[AgentView],
    alert_counts_by_client: &HashMap<String, usize>,
    running_job_targets: &HashMap<String, usize>,
    network_by_client: &HashMap<String, NetworkClientAggregate>,
    counts_truncated: bool,
) -> Vec<DashboardLabelClusterView> {
    let mut groups = agents
        .iter()
        .map(|agent| {
            cluster_for_agents(
                agent.display_name.clone(),
                "client".to_string(),
                Some(format!("id:{}", agent.id)),
                vec![agent],
                alert_counts_by_client,
                running_job_targets,
                network_by_client,
                counts_truncated,
            )
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        right
            .warnings
            .cmp(&left.warnings)
            .then_with(|| right.running_jobs.cmp(&left.running_jobs))
            .then_with(|| left.label.cmp(&right.label))
    });
    groups.truncate(DASHBOARD_TOP_CLUSTERS);
    groups
}

fn build_status_groups(
    agents: &[AgentView],
    alert_counts_by_client: &HashMap<String, usize>,
    running_job_targets: &HashMap<String, usize>,
    network_by_client: &HashMap<String, NetworkClientAggregate>,
    counts_truncated: bool,
) -> Vec<DashboardLabelClusterView> {
    let mut clients_by_status = BTreeMap::<String, Vec<&AgentView>>::new();
    for agent in agents {
        clients_by_status
            .entry(agent.status.clone())
            .or_default()
            .push(agent);
    }
    clients_by_status
        .into_iter()
        .map(|(status, clients)| {
            cluster_for_agents(
                status.clone(),
                "status".to_string(),
                Some(format!("status:{status}")),
                clients,
                alert_counts_by_client,
                running_job_targets,
                network_by_client,
                counts_truncated,
            )
        })
        .collect()
}

fn build_date_groups(
    range: &DashboardRange,
    alerts: &[FleetAlertView],
    backups: &[crate::model::BackupRequestView],
    running_jobs: &[JobHistoryView],
    network_rates: &[TelemetryNetworkRateView],
    counts_truncated: bool,
) -> Vec<DashboardLabelClusterView> {
    let bucket_secs = date_group_bucket_secs(range);
    let mut groups = BTreeMap::<u64, DashboardLabelClusterView>::new();
    let bucket_start = |timestamp: u64| {
        range.start_unix
            + ((timestamp.saturating_sub(range.start_unix) / bucket_secs) * bucket_secs)
    };

    for rate in network_rates {
        if !timestamp_in_range(&rate.bucket_start, range) {
            continue;
        }
        if let Some(timestamp) = parse_timestamp_unix(&rate.bucket_start) {
            let bucket = bucket_start(timestamp);
            let group = date_group_entry(&mut groups, bucket, counts_truncated);
            group.total += rate.sample_count.max(0) as usize;
            group.rx_bps += rate.rx_bps_avg.max(0.0);
            group.tx_bps += rate.tx_bps_avg.max(0.0);
        }
    }
    for alert in alerts {
        if !timestamp_in_range(&alert.observed_at, range) {
            continue;
        }
        if let Some(timestamp) = parse_timestamp_unix(&alert.observed_at) {
            let group = date_group_entry(&mut groups, bucket_start(timestamp), counts_truncated);
            group.warnings += 1;
        }
    }
    for backup in backups {
        if !timestamp_in_range(&backup.created_at, range) {
            continue;
        }
        if let Some(timestamp) = parse_timestamp_unix(&backup.created_at) {
            let group = date_group_entry(&mut groups, bucket_start(timestamp), counts_truncated);
            if backup.status == BackupRequestStatus::ArtifactMetadataRecorded.as_str() {
                group.online += 1;
            } else {
                group.stale += 1;
            }
        }
    }
    for job in running_jobs {
        if !timestamp_in_range(&job.created_at, range) {
            continue;
        }
        if let Some(timestamp) = parse_timestamp_unix(&job.created_at) {
            date_group_entry(&mut groups, bucket_start(timestamp), counts_truncated)
                .running_jobs += 1;
        }
    }

    let mut groups = groups.into_values().collect::<Vec<_>>();
    if groups.len() > DASHBOARD_TOP_CLUSTERS {
        groups.drain(0..groups.len() - DASHBOARD_TOP_CLUSTERS);
    }
    groups
}

fn date_group_entry(
    groups: &mut BTreeMap<u64, DashboardLabelClusterView>,
    bucket: u64,
    counts_truncated: bool,
) -> &mut DashboardLabelClusterView {
    groups
        .entry(bucket)
        .or_insert_with(|| DashboardLabelClusterView {
            label: unix_to_rfc3339(bucket),
            kind: "date".to_string(),
            query: None,
            total: 0,
            online: 0,
            offline: 0,
            revoked: 0,
            stale: 0,
            warnings: 0,
            running_jobs: 0,
            counts_truncated,
            rx_bps: 0.0,
            tx_bps: 0.0,
            drilldown: drilldown("Open topology evidence", "Topology", "evidence", None),
        })
}

fn date_group_bucket_secs(range: &DashboardRange) -> u64 {
    let span = range.end_unix.saturating_sub(range.start_unix);
    if span <= 6 * 60 * 60 {
        15 * 60
    } else if span <= 48 * 60 * 60 {
        60 * 60
    } else {
        24 * 60 * 60
    }
}

fn cluster_for_agents(
    label: String,
    kind: String,
    query: Option<String>,
    agents: Vec<&AgentView>,
    alert_counts_by_client: &HashMap<String, usize>,
    running_job_targets: &HashMap<String, usize>,
    network_by_client: &HashMap<String, NetworkClientAggregate>,
    counts_truncated: bool,
) -> DashboardLabelClusterView {
    let mut online = 0_usize;
    let mut offline = 0_usize;
    let mut revoked = 0_usize;
    let mut stale = 0_usize;
    let mut warnings = 0_usize;
    let mut running_jobs = 0_usize;
    let mut rx_bps = 0.0_f64;
    let mut tx_bps = 0.0_f64;
    for agent in &agents {
        if agent.status == "online" {
            online += 1;
        } else if matches!(agent.status.as_str(), "offline" | "disconnected" | "never") {
            offline += 1;
        } else if agent.status == "revoked" {
            revoked += 1;
        } else if is_degraded_agent_status(&agent.status) {
            stale += 1;
        }
        warnings += alert_counts_by_client
            .get(&agent.id)
            .copied()
            .unwrap_or_default();
        running_jobs += running_job_targets
            .get(&agent.id)
            .copied()
            .unwrap_or_default();
        if let Some(network) = network_by_client.get(&agent.id) {
            rx_bps += network.rx_bps;
            tx_bps += network.tx_bps;
        }
    }
    let total = agents.len();
    DashboardLabelClusterView {
        label: label.clone(),
        kind,
        query: query.clone(),
        total,
        online,
        offline,
        revoked,
        stale,
        warnings,
        running_jobs,
        counts_truncated,
        rx_bps,
        tx_bps,
        drilldown: drilldown(
            "Open matching VPS",
            "Fleet",
            "instances",
            query.or_else(|| Some(String::new())),
        ),
    }
}

fn recent_alerts(
    alerts: &[FleetAlertView],
    agents_by_id: &HashMap<String, AgentView>,
) -> Vec<DashboardAlertSummaryView> {
    let mut alerts = alerts.to_vec();
    alerts.sort_by(|left, right| {
        timestamp_sort_key(&right.observed_at)
            .cmp(&timestamp_sort_key(&left.observed_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    alerts
        .into_iter()
        .take(DASHBOARD_TOP_ALERTS)
        .map(|alert| {
            let label = alert
                .client_id
                .as_ref()
                .and_then(|client_id| agents_by_id.get(client_id))
                .map(|agent| format!("Open {}", agent.display_name))
                .unwrap_or_else(|| "Open alert target".to_string());
            DashboardAlertSummaryView {
                id: alert.id,
                severity: alert.severity,
                category: alert.category,
                title: alert.title,
                client_label: alert
                    .client_id
                    .as_ref()
                    .and_then(|client_id| agents_by_id.get(client_id))
                    .map(|agent| agent.display_name.clone()),
                client_id: alert.client_id.clone(),
                observed_at: alert.observed_at,
                drilldown: drilldown(
                    &label,
                    "Fleet",
                    "alerts",
                    alert.client_id.map(|client_id| format!("id:{client_id}")),
                ),
            }
        })
        .collect()
}

fn latest_rollups_by_client(
    rollups: &[TelemetryRollupView],
) -> HashMap<String, TelemetryRollupView> {
    let mut latest = HashMap::new();
    for rollup in rollups {
        let replace = latest
            .get(&rollup.client_id)
            .map(|stored: &TelemetryRollupView| {
                timestamp_sort_key(&rollup.latest_observed_at)
                    > timestamp_sort_key(&stored.latest_observed_at)
            })
            .unwrap_or(true);
        if replace {
            latest.insert(rollup.client_id.clone(), rollup.clone());
        }
    }
    latest
}

fn latest_rates_by_client_interface(
    rates: &[TelemetryNetworkRateView],
) -> HashMap<(String, String), TelemetryNetworkRateView> {
    let mut latest = HashMap::new();
    for rate in rates {
        let key = (rate.client_id.clone(), rate.interface.clone());
        let replace = latest
            .get(&key)
            .map(|stored: &TelemetryNetworkRateView| {
                timestamp_sort_key(&rate.bucket_start) > timestamp_sort_key(&stored.bucket_start)
            })
            .unwrap_or(true);
        if replace {
            latest.insert(key, rate.clone());
        }
    }
    latest
}

fn coherent_latest_rates(
    mut rates: HashMap<(String, String), TelemetryNetworkRateView>,
) -> HashMap<(String, String), TelemetryNetworkRateView> {
    let mut latest_by_client = HashMap::<String, u64>::new();
    for rate in rates.values() {
        let observed = timestamp_sort_key(&rate.bucket_start);
        latest_by_client
            .entry(rate.client_id.clone())
            .and_modify(|latest| *latest = (*latest).max(observed))
            .or_insert(observed);
    }
    rates.retain(|_, rate| {
        let latest = latest_by_client
            .get(&rate.client_id)
            .copied()
            .unwrap_or_default();
        latest.saturating_sub(timestamp_sort_key(&rate.bucket_start))
            <= NETWORK_SNAPSHOT_COHERENCE_SECS
    });
    rates
}

fn network_by_client<'a>(
    rates: impl Iterator<Item = &'a TelemetryNetworkRateView>,
) -> HashMap<String, NetworkClientAggregate> {
    let mut by_client = HashMap::<String, NetworkClientAggregate>::new();
    for rate in rates {
        let entry = by_client.entry(rate.client_id.clone()).or_default();
        entry.rx_bps += rate.rx_bps_avg.max(0.0);
        entry.tx_bps += rate.tx_bps_avg.max(0.0);
        entry.interfaces.insert(rate.interface.clone());
    }
    by_client
}

fn alert_counts_by_client(alerts: &[FleetAlertView]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for alert in alerts {
        if let Some(client_id) = &alert.client_id {
            *counts.entry(client_id.clone()).or_insert(0) += 1;
        }
    }
    counts
}

fn tag_kind(tag: &str) -> &'static str {
    if tag.starts_with("provider:") {
        "provider"
    } else if tag.starts_with("country:") {
        "country"
    } else {
        "tag"
    }
}

fn tag_query(tag: &str) -> String {
    if tag.starts_with("provider:") || tag.starts_with("country:") {
        tag.to_string()
    } else {
        format!("tag:{tag}")
    }
}

fn normalized_namespaced_value(namespace: &str, value: &str) -> String {
    if value.starts_with(&format!("{namespace}:")) {
        value.to_string()
    } else {
        format!("{namespace}:{value}")
    }
}

fn agent_matches_curve_exclusion(agent: &AgentView, selector: &str) -> bool {
    let selector = selector.trim();
    if selector.is_empty() {
        return false;
    }
    let Some((kind, value)) = selector.split_once(':') else {
        return agent
            .tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case(selector));
    };
    let kind = kind.trim().to_ascii_lowercase();
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    match kind.as_str() {
        "name" => agent
            .display_name
            .to_ascii_lowercase()
            .contains(&value.to_ascii_lowercase()),
        "id" => agent
            .id
            .to_ascii_lowercase()
            .starts_with(&value.to_ascii_lowercase()),
        "provider" => tag_matches(&agent.tags, &normalized_namespaced_value("provider", value)),
        "country" => tag_matches(&agent.tags, &normalized_namespaced_value("country", value)),
        "tag" => tag_matches(&agent.tags, value),
        _ => tag_matches(&agent.tags, selector),
    }
}

fn tag_matches(tags: &[String], expected: &str) -> bool {
    tags.iter().any(|tag| tag.eq_ignore_ascii_case(expected))
}

fn is_degraded_agent_status(status: &str) -> bool {
    status == "stale"
}

fn total_bps(client: &DashboardNetworkClientView) -> f64 {
    client.rx_bps + client.tx_bps
}

fn timestamp_in_range(value: &str, range: &DashboardRange) -> bool {
    timestamp_in_optional_bounds(value, Some(range.start_unix), Some(range.end_unix))
}

fn timestamp_sort_key(value: &str) -> u64 {
    parse_timestamp_unix(value).unwrap_or_default()
}

fn chart_bucket(value: &str, range: &DashboardRange, chart_step_secs: u64) -> u64 {
    let timestamp = parse_timestamp_unix(value).unwrap_or(range.start_unix);
    let step = chart_step_secs.max(1);
    range
        .start_unix
        .saturating_add((timestamp.saturating_sub(range.start_unix) / step) * step)
}

fn parse_timestamp_unix(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(timestamp) = value.parse::<i64>() {
        return (timestamp >= 0).then_some(timestamp as u64);
    }
    DateTime::parse_from_rfc3339(value)
        .ok()
        .or_else(|| DateTime::parse_from_rfc3339(&normalize_postgres_timestamp(value)).ok())
        .map(|timestamp| timestamp.timestamp())
        .filter(|timestamp| *timestamp >= 0)
        .map(|timestamp| timestamp as u64)
}

fn normalize_postgres_timestamp(value: &str) -> String {
    let mut normalized = value.replacen(' ', "T", 1);
    if let Some(offset_start) = normalized.rfind(['+', '-']) {
        let offset = &normalized[offset_start..];
        if offset.len() == 3 {
            normalized.push_str(":00");
        } else if offset.len() == 5 && !offset.contains(':') {
            normalized.insert(offset_start + 3, ':');
        }
    }
    normalized
}

fn unix_to_rfc3339(value: u64) -> String {
    Utc.timestamp_opt(value as i64, 0)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339()
}

fn client_drilldown(client_id: &str) -> DashboardDrilldownView {
    drilldown(
        "Open VPS details",
        "Fleet",
        "instances",
        Some(format!("id:{client_id}")),
    )
}

fn drilldown(
    label: &str,
    view: &str,
    subpage: &str,
    query: Option<String>,
) -> DashboardDrilldownView {
    DashboardDrilldownView {
        label: label.to_string(),
        view: view.to_string(),
        subpage: subpage.to_string(),
        query: query.filter(|value| !value.is_empty()),
    }
}

#[cfg(test)]
#[path = "tests_routes_dashboard.rs"]
mod chart_step_tests;
