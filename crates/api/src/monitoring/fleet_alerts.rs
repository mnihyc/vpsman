use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;
use serde_json::{json, Value};
use vpsman_common::payload_hash;

use crate::{
    model::{
        AgentView, BackupRequestView, FleetAlertQuery, FleetAlertView, JobHistoryView,
        TelemetryRollupView, TelemetryTunnelView,
    },
    model_alert_policies::PolicyAlertQuery,
    model_alert_states::FleetAlertStateView,
    repository_alert_policies::policy_alert_to_fleet_alert,
    repository_jobs::CapabilityDegradedJobTargetCandidate,
    repository_telemetry_rollups::tunnel_adapter_health_is_degraded,
    state::AppState,
    unix_now,
    util::{compare_timestamps_desc, timestamp_in_optional_bounds},
};

const DEFAULT_MEMORY_AVAILABLE_CRITICAL_RATIO: f64 = 0.10;
const DEFAULT_MEMORY_AVAILABLE_WARNING_RATIO: f64 = 0.20;
const DEFAULT_DISK_AVAILABLE_CRITICAL_RATIO: f64 = 0.10;
const DEFAULT_DISK_AVAILABLE_WARNING_RATIO: f64 = 0.20;
const DEFAULT_CPU_LOAD_WARNING: f64 = 2.0;
const DEFAULT_CPU_LOAD_CRITICAL: f64 = 4.0;
const FLEET_ALERT_RESULT_LIMIT_MAX: i64 = 200;
// Historical/event sources are deliberately bounded independently before they
// are merged with current agent and resource snapshots. Repository
// selectors must apply native client, category, severity, and dashboard-window
// filters before this horizon so a narrow query is not crowded out by unrelated
// fleet history. Saturation is surfaced to dashboard/UI consumers as a lower
// bound; older event history remains available from its owning workflow.
const FLEET_EVENT_SOURCE_HORIZON_MAX: i64 = 200;

#[derive(Clone, Debug)]
pub(crate) struct FleetAlertPolicy {
    pub(crate) memory_available_warning_ratio: f64,
    pub(crate) memory_available_critical_ratio: f64,
    pub(crate) disk_available_warning_ratio: f64,
    pub(crate) disk_available_critical_ratio: f64,
    pub(crate) cpu_load_warning: f64,
    pub(crate) cpu_load_critical: f64,
}

impl Default for FleetAlertPolicy {
    fn default() -> Self {
        Self {
            memory_available_warning_ratio: DEFAULT_MEMORY_AVAILABLE_WARNING_RATIO,
            memory_available_critical_ratio: DEFAULT_MEMORY_AVAILABLE_CRITICAL_RATIO,
            disk_available_warning_ratio: DEFAULT_DISK_AVAILABLE_WARNING_RATIO,
            disk_available_critical_ratio: DEFAULT_DISK_AVAILABLE_CRITICAL_RATIO,
            cpu_load_warning: DEFAULT_CPU_LOAD_WARNING,
            cpu_load_critical: DEFAULT_CPU_LOAD_CRITICAL,
        }
    }
}

impl FleetAlertPolicy {
    pub(crate) fn new(
        memory_available_warning_ratio: f64,
        memory_available_critical_ratio: f64,
        disk_available_warning_ratio: f64,
        disk_available_critical_ratio: f64,
        cpu_load_warning: f64,
        cpu_load_critical: f64,
    ) -> Result<Self> {
        validate_ratio_thresholds(
            "memory_available",
            memory_available_warning_ratio,
            memory_available_critical_ratio,
        )?;
        validate_ratio_thresholds(
            "disk_available",
            disk_available_warning_ratio,
            disk_available_critical_ratio,
        )?;
        validate_cpu_thresholds(cpu_load_warning, cpu_load_critical)?;
        Ok(Self {
            memory_available_warning_ratio,
            memory_available_critical_ratio,
            disk_available_warning_ratio,
            disk_available_critical_ratio,
            cpu_load_warning,
            cpu_load_critical,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AgentAlertScope {
    pub(crate) provider: Option<String>,
    pub(crate) tags: Vec<String>,
}

pub(crate) struct FleetAlertSelector<'a> {
    pub(crate) allowed_client_ids: &'a HashSet<String>,
    pub(crate) start_unix: u64,
    pub(crate) end_unix: u64,
    pub(crate) snapshot_unix: u64,
    pub(crate) include_global: bool,
}

pub(crate) struct FleetAlertSelection {
    pub(crate) alerts: Vec<FleetAlertView>,
    pub(crate) truncated: bool,
}

pub(crate) fn build_agent_alert_scopes(agents: &[AgentView]) -> HashMap<String, AgentAlertScope> {
    agents
        .iter()
        .map(|agent| {
            (
                agent.id.clone(),
                AgentAlertScope {
                    provider: provider_from_agent(agent),
                    tags: agent.tags.clone(),
                },
            )
        })
        .collect()
}

fn provider_from_agent(agent: &AgentView) -> Option<String> {
    agent.tags.iter().find_map(|tag| {
        tag.strip_prefix("provider:")
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    })
}

fn validate_ratio_thresholds(name: &str, warning: f64, critical: f64) -> Result<()> {
    if !warning.is_finite() || !critical.is_finite() {
        anyhow::bail!("{name} alert thresholds must be finite numbers");
    }
    if warning <= 0.0 || warning >= 1.0 || critical <= 0.0 || critical >= 1.0 {
        anyhow::bail!("{name} alert thresholds must be greater than 0 and below 1");
    }
    if critical > warning {
        anyhow::bail!(
            "{name} critical threshold must be less than or equal to the warning threshold"
        );
    }
    Ok(())
}

fn validate_cpu_thresholds(warning: f64, critical: f64) -> Result<()> {
    if !warning.is_finite() || !critical.is_finite() {
        anyhow::bail!("cpu load alert thresholds must be finite numbers");
    }
    if warning <= 0.0 || critical <= 0.0 {
        anyhow::bail!("cpu load alert thresholds must be greater than 0");
    }
    if critical < warning {
        anyhow::bail!("cpu load critical threshold must be greater than or equal to warning");
    }
    Ok(())
}

impl AppState {
    pub(crate) async fn list_fleet_alerts(
        &self,
        query: FleetAlertQuery,
    ) -> Result<Vec<FleetAlertView>> {
        Ok(self.list_fleet_alerts_selected(query, None).await?.alerts)
    }

    pub(crate) async fn list_fleet_alerts_selected(
        &self,
        query: FleetAlertQuery,
        selector: Option<FleetAlertSelector<'_>>,
    ) -> Result<FleetAlertSelection> {
        let selector = selector.as_ref();
        let mut alerts = Vec::new();
        let mut source_saturated = false;
        let snapshot_observed_at = selector
            .map(|selector| selector.snapshot_unix)
            .unwrap_or_else(unix_now)
            .to_string();
        let needs_agents = (query_allows_category(&query, "agent_status")
            && query_allows_any_severity(&query, &["critical", "warning"]))
            || (query_allows_category(&query, "resource")
                && query_allows_any_severity(&query, &["critical", "warning"]));
        let visible_agents = self.repo.list_agents().await?;
        let visible_client_ids = visible_agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<HashSet<_>>();
        let operational_client_ids = selector
            .map(|selector| {
                selector
                    .allowed_client_ids
                    .intersection(&visible_client_ids)
                    .cloned()
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_else(|| visible_client_ids.clone());
        let agents = if needs_agents {
            visible_agents
                .into_iter()
                .filter(|agent| {
                    operational_client_ids.contains(&agent.id)
                        && query
                            .client_id
                            .as_deref()
                            .is_none_or(|client_id| agent.id == client_id)
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if query_allows_category(&query, "agent_status")
            && query_allows_any_severity(&query, &["critical", "warning"])
        {
            append_agent_status_alerts(&mut alerts, &agents, &snapshot_observed_at);
        }

        if query_allows_category(&query, "resource")
            && query_allows_any_severity(&query, &["critical", "warning"])
        {
            let rollup_client_ids = agents
                .iter()
                .map(|agent| agent.id.clone())
                .collect::<Vec<_>>();
            let rollups = self
                .repo
                .list_latest_telemetry_rollups_for_clients(&rollup_client_ids, None)
                .await?;
            let base_alert_policy = self.fleet_alert_policy();
            append_resource_alerts(&mut alerts, &latest_rollups(rollups), &base_alert_policy)?;
        }

        let policy_alerts = self
            .repo
            .list_policy_alert_candidates(
                &PolicyAlertQuery {
                    limit: None,
                    client_id: query.client_id.clone(),
                    severity: query.severity.clone(),
                    category: query.category.clone(),
                    policy_group_id: None,
                },
                FLEET_EVENT_SOURCE_HORIZON_MAX as usize,
                Some(&operational_client_ids),
                selector.map(|selector| selector.start_unix),
                selector.map(|selector| selector.end_unix),
            )
            .await?;
        source_saturated |= policy_alerts.len() >= FLEET_EVENT_SOURCE_HORIZON_MAX as usize;
        alerts.extend(policy_alerts.iter().map(policy_alert_to_fleet_alert));

        if query_allows_category(&query, "network")
            && query_allows_any_severity(&query, &["critical", "warning"])
        {
            let scoped_client_ids = operational_client_ids.iter().cloned().collect::<Vec<_>>();
            let tunnels = self
                .repo
                .list_fleet_alert_tunnel_candidates(
                    query.client_id.as_deref(),
                    Some(&scoped_client_ids),
                    query.severity.as_deref(),
                    selector.map(|selector| selector.start_unix),
                    selector.map(|selector| selector.end_unix),
                    FLEET_EVENT_SOURCE_HORIZON_MAX as usize,
                )
                .await?;
            source_saturated |= tunnels.len() >= FLEET_EVENT_SOURCE_HORIZON_MAX as usize;
            append_tunnel_alerts(&mut alerts, &tunnels);
        }

        if query_allows_category(&query, "backup") && query_allows_severity(&query, "critical") {
            let backup_requests = self
                .repo
                .list_failed_backup_request_candidates(
                    query.client_id.as_deref(),
                    Some(&operational_client_ids),
                    selector.map(|selector| selector.start_unix),
                    selector.map(|selector| selector.end_unix),
                    FLEET_EVENT_SOURCE_HORIZON_MAX,
                )
                .await?;
            source_saturated |= backup_requests.len() >= FLEET_EVENT_SOURCE_HORIZON_MAX as usize;
            append_backup_request_alerts(&mut alerts, &backup_requests);
        }

        if query.client_id.is_none()
            && selector.is_none_or(|selector| selector.include_global)
            && query_allows_any_category(&query, &["backup", "agent_update", "job"])
            && query_allows_any_severity(&query, &["critical", "warning"])
        {
            let jobs = self
                .repo
                .list_failed_job_alert_candidates(
                    query.category.as_deref(),
                    query.severity.as_deref(),
                    selector.map(|selector| selector.start_unix),
                    selector.map(|selector| selector.end_unix),
                    FLEET_EVENT_SOURCE_HORIZON_MAX,
                )
                .await?;
            source_saturated |= jobs.len() >= FLEET_EVENT_SOURCE_HORIZON_MAX as usize;
            append_job_alerts(&mut alerts, &jobs);
        }
        if query_allows_category(&query, "capability_degraded")
            && query_allows_severity(&query, "warning")
        {
            let targets = self
                .repo
                .list_capability_degraded_job_target_candidates(
                    query.client_id.as_deref(),
                    Some(&operational_client_ids),
                    selector.map(|selector| selector.start_unix),
                    selector.map(|selector| selector.end_unix),
                    FLEET_EVENT_SOURCE_HORIZON_MAX,
                )
                .await?;
            source_saturated |= targets.len() >= FLEET_EVENT_SOURCE_HORIZON_MAX as usize;
            append_capability_degraded_target_alerts(&mut alerts, &targets);
        }

        let alert_ids = alerts
            .iter()
            .map(|alert| alert.id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let alert_states = self
            .repo
            .list_fleet_alert_states_for_alert_ids(&alert_ids)
            .await?;
        apply_alert_states(&mut alerts, &alert_states);
        if let Some(selector) = selector {
            apply_alert_selector(&mut alerts, selector);
        }
        let result_truncated = apply_alert_filters(&mut alerts, &query);
        Ok(FleetAlertSelection {
            alerts,
            truncated: source_saturated || result_truncated,
        })
    }
}

fn query_allows_category(query: &FleetAlertQuery, category: &str) -> bool {
    query
        .category
        .as_deref()
        .is_none_or(|requested| requested == category)
}

fn query_allows_any_category(query: &FleetAlertQuery, categories: &[&str]) -> bool {
    query
        .category
        .as_deref()
        .is_none_or(|requested| categories.contains(&requested))
}

fn query_allows_severity(query: &FleetAlertQuery, severity: &str) -> bool {
    query
        .severity
        .as_deref()
        .is_none_or(|requested| requested == severity)
}

fn query_allows_any_severity(query: &FleetAlertQuery, severities: &[&str]) -> bool {
    query
        .severity
        .as_deref()
        .is_none_or(|requested| severities.contains(&requested))
}

fn append_agent_status_alerts(
    alerts: &mut Vec<FleetAlertView>,
    agents: &[AgentView],
    observed_at: &str,
) {
    for agent in agents {
        if agent.status == "online" {
            continue;
        }
        let severity = if matches!(agent.status.as_str(), "offline" | "revoked") {
            "critical"
        } else {
            "warning"
        };
        let (title, detail) = if agent.status == "revoked" {
            (
                "VPS access revoked",
                format!(
                    "{} cannot reconnect until an operator assigns a new key",
                    agent.display_name
                ),
            )
        } else {
            (
                "Agent is not online",
                format!("{} currently reports {}", agent.display_name, agent.status),
            )
        };
        push_alert(
            alerts,
            AlertInput {
                severity,
                category: "agent_status",
                target_kind: "agent",
                target_id: &agent.id,
                client_id: Some(&agent.id),
                title,
                detail,
                status: &agent.status,
                evidence: json!({
                    "display_name": &agent.display_name,
                    "tags": &agent.tags,
                    "capability_privilege_mode": agent.capabilities.privilege_mode,
                }),
                observed_at: observed_at.to_string(),
            },
        );
    }
}

fn append_resource_alerts(
    alerts: &mut Vec<FleetAlertView>,
    rollups: &HashMap<String, TelemetryRollupView>,
    policy: &FleetAlertPolicy,
) -> Result<()> {
    for rollup in rollups.values() {
        let policy_evidence = json!({
            "source": "builtin_default_resource_thresholds",
            "client_id": &rollup.client_id,
        });

        if rollup.cpu_load_1_max >= policy.cpu_load_critical {
            push_resource_alert(
                alerts,
                "critical",
                rollup,
                "cpu_load_high",
                "CPU load is high",
                format!("1-minute load max {:.2}", rollup.cpu_load_1_max),
                json!({
                    "cpu_load_1_max": rollup.cpu_load_1_max,
                    "threshold": policy.cpu_load_critical,
                    "alert_policy": policy_evidence.clone(),
                }),
            );
        } else if rollup.cpu_load_1_max >= policy.cpu_load_warning {
            push_resource_alert(
                alerts,
                "warning",
                rollup,
                "cpu_load_high",
                "CPU load is elevated",
                format!("1-minute load max {:.2}", rollup.cpu_load_1_max),
                json!({
                    "cpu_load_1_max": rollup.cpu_load_1_max,
                    "threshold": policy.cpu_load_warning,
                    "alert_policy": policy_evidence.clone(),
                }),
            );
        }

        if let Some((severity, ratio)) = available_ratio_alert(
            rollup.memory_total_bytes_max,
            rollup.memory_available_bytes_min,
            policy.memory_available_warning_ratio,
            policy.memory_available_critical_ratio,
        ) {
            push_resource_alert(
                alerts,
                severity,
                rollup,
                "memory_low",
                "Memory availability is low",
                format!("{:.0}% memory available", ratio * 100.0),
                json!({
                    "memory_total_bytes": rollup.memory_total_bytes_max,
                    "memory_available_bytes_min": rollup.memory_available_bytes_min,
                    "available_ratio": ratio,
                    "warning_threshold": policy.memory_available_warning_ratio,
                    "critical_threshold": policy.memory_available_critical_ratio,
                    "alert_policy": policy_evidence.clone(),
                }),
            );
        }

        if let Some((severity, ratio)) = available_ratio_alert(
            rollup.disk_total_bytes_max,
            rollup.disk_available_bytes_min,
            policy.disk_available_warning_ratio,
            policy.disk_available_critical_ratio,
        ) {
            push_resource_alert(
                alerts,
                severity,
                rollup,
                "disk_low",
                "Disk availability is low",
                format!("{:.0}% disk available", ratio * 100.0),
                json!({
                    "disk_total_bytes": rollup.disk_total_bytes_max,
                    "disk_available_bytes_min": rollup.disk_available_bytes_min,
                    "available_ratio": ratio,
                    "warning_threshold": policy.disk_available_warning_ratio,
                    "critical_threshold": policy.disk_available_critical_ratio,
                    "alert_policy": policy_evidence,
                }),
            );
        }
    }
    Ok(())
}

fn append_tunnel_alerts(alerts: &mut Vec<FleetAlertView>, tunnels: &[TelemetryTunnelView]) {
    for tunnel in tunnels {
        if tunnel_adapter_health_is_degraded(tunnel) {
            push_tunnel_alert(
                alerts,
                "critical",
                tunnel,
                "tunnel_adapter_degraded",
                "Tunnel adapter status failed",
                tunnel
                    .adapter_health
                    .as_ref()
                    .and_then(|health| health.reason.clone())
                    .unwrap_or_else(|| "adapter command did not report healthy status".to_string()),
                json!({"adapter_health": &tunnel.adapter_health}),
            );
        }
        if tunnel
            .traffic_status
            .as_deref()
            .is_some_and(|status| status != "ok")
        {
            push_tunnel_alert(
                alerts,
                "warning",
                tunnel,
                "tunnel_traffic_degraded",
                "Tunnel traffic source is degraded",
                tunnel
                    .traffic_reason
                    .clone()
                    .unwrap_or_else(|| "selected traffic source is not reporting ok".to_string()),
                json!({
                    "traffic_source": &tunnel.traffic_source,
                    "traffic_status": &tunnel.traffic_status,
                    "traffic_reason": &tunnel.traffic_reason,
                }),
            );
        }
    }
}

fn append_job_alerts(alerts: &mut Vec<FleetAlertView>, jobs: &[JobHistoryView]) {
    for job in jobs {
        let severity = if job.status == "partial_success" {
            "warning"
        } else {
            "critical"
        };
        let category =
            if job.command_type.contains("backup") || job.command_type.contains("restore") {
                "backup"
            } else if job.command_type.contains("agent_update") {
                "agent_update"
            } else {
                "job"
            };
        push_job_alert(
            alerts,
            severity,
            job,
            category,
            "Job requires operator attention",
            format!("{} job {}", job.command_type, job.status),
            json!({"command_type": &job.command_type, "target_count": job.target_count}),
        );
    }
}

fn append_backup_request_alerts(alerts: &mut Vec<FleetAlertView>, backups: &[BackupRequestView]) {
    for backup in backups {
        if backup.status == "execution_failed" {
            push_alert(
                alerts,
                AlertInput {
                    severity: "critical",
                    category: "backup",
                    target_kind: "backup_request",
                    target_id: &backup.id.to_string(),
                    client_id: Some(&backup.client_id),
                    title: "Backup request failed",
                    detail: format!("backup request {} is {}", backup.id, backup.status),
                    status: &backup.status,
                    evidence: json!({
                        "paths": &backup.paths,
                        "include_config": backup.include_config,
                        "artifact_id": backup.artifact_id,
                    }),
                    observed_at: backup.created_at.clone(),
                },
            );
        }
    }
}

fn append_capability_degraded_target_alerts(
    alerts: &mut Vec<FleetAlertView>,
    candidates: &[CapabilityDegradedJobTargetCandidate],
) {
    for candidate in candidates {
        let job = &candidate.job;
        let target = &candidate.target;
        push_alert(
            alerts,
            AlertInput {
                severity: "warning",
                category: "capability_degraded",
                target_kind: "job_target",
                target_id: &format!("{}:{}", job.id, target.client_id),
                client_id: Some(&target.client_id),
                title: "Operation skipped because the agent lacks a required capability",
                detail: candidate.hint.clone(),
                status: &candidate.reason,
                evidence: json!({
                    "job_id": job.id,
                    "command_type": &job.command_type,
                    "target_status": &target.status,
                    "target_message": &target.message,
                    "reason": &candidate.reason,
                    "hint": &candidate.hint,
                    "exit_code": target.exit_code,
                    "started_at": &target.started_at,
                    "completed_at": &target.completed_at,
                }),
                observed_at: target
                    .completed_at
                    .clone()
                    .or(target.started_at.clone())
                    .unwrap_or_else(|| job.created_at.clone()),
            },
        );
    }
}

fn latest_rollups(rollups: Vec<TelemetryRollupView>) -> HashMap<String, TelemetryRollupView> {
    let mut latest = HashMap::new();
    for rollup in rollups {
        let replace = latest
            .get(&rollup.client_id)
            .is_none_or(|current: &TelemetryRollupView| {
                rollup.latest_observed_at > current.latest_observed_at
            });
        if replace {
            latest.insert(rollup.client_id.clone(), rollup);
        }
    }
    latest
}

fn available_ratio_alert(
    total: i64,
    available: i64,
    warning_threshold: f64,
    critical_threshold: f64,
) -> Option<(&'static str, f64)> {
    if total <= 0 || available < 0 {
        return None;
    }
    let ratio = available as f64 / total as f64;
    if ratio <= critical_threshold {
        Some(("critical", ratio))
    } else if ratio <= warning_threshold {
        Some(("warning", ratio))
    } else {
        None
    }
}

fn push_resource_alert(
    alerts: &mut Vec<FleetAlertView>,
    severity: &'static str,
    rollup: &TelemetryRollupView,
    status: &'static str,
    title: &'static str,
    detail: String,
    evidence: Value,
) {
    push_alert(
        alerts,
        AlertInput {
            severity,
            category: "resource",
            target_kind: "agent",
            target_id: &rollup.client_id,
            client_id: Some(&rollup.client_id),
            title,
            detail,
            status,
            evidence,
            observed_at: rollup.latest_observed_at.clone(),
        },
    );
}

fn push_tunnel_alert(
    alerts: &mut Vec<FleetAlertView>,
    severity: &'static str,
    tunnel: &TelemetryTunnelView,
    status: &'static str,
    title: &'static str,
    detail: String,
    evidence: Value,
) {
    push_alert(
        alerts,
        AlertInput {
            severity,
            category: "network",
            target_kind: "tunnel",
            target_id: &format!("{}:{}", tunnel.client_id, tunnel.interface),
            client_id: Some(&tunnel.client_id),
            title,
            detail,
            status,
            evidence,
            observed_at: tunnel.observed_at.clone(),
        },
    );
}

fn push_job_alert(
    alerts: &mut Vec<FleetAlertView>,
    severity: &'static str,
    job: &JobHistoryView,
    category: &'static str,
    title: &'static str,
    detail: String,
    evidence: Value,
) {
    push_alert(
        alerts,
        AlertInput {
            severity,
            category,
            target_kind: "job",
            target_id: &job.id.to_string(),
            client_id: None,
            title,
            detail,
            status: &job.status,
            evidence,
            observed_at: job
                .completed_at
                .clone()
                .unwrap_or_else(|| job.created_at.clone()),
        },
    );
}

struct AlertInput<'a> {
    severity: &'static str,
    category: &'static str,
    target_kind: &'static str,
    target_id: &'a str,
    client_id: Option<&'a str>,
    title: &'static str,
    detail: String,
    status: &'a str,
    evidence: Value,
    observed_at: String,
}

fn push_alert(alerts: &mut Vec<FleetAlertView>, input: AlertInput<'_>) {
    let fingerprint = json!({
        "severity": input.severity,
        "category": input.category,
        "target_kind": input.target_kind,
        "target_id": input.target_id,
        "title": input.title,
        "status": input.status,
    });
    let hash = payload_hash(fingerprint.to_string().as_bytes());
    alerts.push(FleetAlertView {
        id: format!("{}:{}:{}", input.category, input.target_kind, &hash[..16]),
        severity: input.severity.to_string(),
        category: input.category.to_string(),
        target_kind: input.target_kind.to_string(),
        target_id: input.target_id.to_string(),
        client_id: input.client_id.map(ToOwned::to_owned),
        title: input.title.to_string(),
        detail: input.detail,
        status: input.status.to_string(),
        evidence: input.evidence,
        observed_at: input.observed_at,
        operator_state: "open".to_string(),
        muted_until_unix: None,
        escalation_level: 0,
        state_reason: None,
        state_actor_id: None,
        state_updated_at: None,
    });
}

fn apply_alert_states(alerts: &mut [FleetAlertView], states: &[FleetAlertStateView]) {
    let now = unix_now() as i64;
    let state_by_id = states
        .iter()
        .map(|state| (state.alert_id.as_str(), state))
        .collect::<HashMap<_, _>>();
    for alert in alerts {
        let Some(state) = state_by_id.get(alert.id.as_str()) else {
            continue;
        };
        let effective_state = if state.state == "muted" {
            match state.muted_until_unix {
                Some(until) if until > now => "muted",
                _ => "open",
            }
        } else {
            state.state.as_str()
        };
        alert.operator_state = effective_state.to_string();
        alert.muted_until_unix = state.muted_until_unix;
        alert.escalation_level = state.escalation_level;
        alert.state_reason = state.reason.clone();
        alert.state_actor_id = state.actor_id;
        alert.state_updated_at = Some(state.updated_at.clone());
    }
}

fn apply_alert_selector(alerts: &mut Vec<FleetAlertView>, selector: &FleetAlertSelector<'_>) {
    alerts.retain(|alert| {
        let client_matches = alert
            .client_id
            .as_ref()
            .map(|client_id| selector.allowed_client_ids.contains(client_id))
            .unwrap_or(selector.include_global);
        client_matches
            && timestamp_in_optional_bounds(
                &alert.observed_at,
                Some(selector.start_unix),
                Some(selector.end_unix),
            )
    });
}

fn apply_alert_filters(alerts: &mut Vec<FleetAlertView>, query: &FleetAlertQuery) -> bool {
    if let Some(client_id) = query.client_id.as_deref() {
        alerts.retain(|alert| alert.client_id.as_deref() == Some(client_id));
    }
    if let Some(severity) = query.severity.as_deref() {
        alerts.retain(|alert| alert.severity == severity);
    }
    if let Some(category) = query.category.as_deref() {
        alerts.retain(|alert| alert.category == category);
    }
    if !query.include_muted.unwrap_or(false) {
        alerts.retain(|alert| alert.operator_state != "muted");
    }
    if let Some(operator_state) = query.operator_state.as_deref() {
        alerts.retain(|alert| alert.operator_state == operator_state);
    }
    alerts.sort_by(|left, right| {
        operator_state_rank(&left.operator_state)
            .cmp(&operator_state_rank(&right.operator_state))
            .then_with(|| severity_rank(&left.severity).cmp(&severity_rank(&right.severity)))
            .then_with(|| right.escalation_level.cmp(&left.escalation_level))
            .then_with(|| compare_timestamps_desc(&left.observed_at, &right.observed_at))
            .then_with(|| left.category.cmp(&right.category))
            .then_with(|| left.target_id.cmp(&right.target_id))
    });
    let limit = query
        .limit
        .unwrap_or(50)
        .clamp(1, FLEET_ALERT_RESULT_LIMIT_MAX) as usize;
    let truncated = alerts.len() > limit;
    alerts.truncate(limit);
    truncated
}

fn operator_state_rank(state: &str) -> usize {
    match state {
        "escalated" => 0,
        "open" => 1,
        "acknowledged" => 2,
        "muted" => 3,
        _ => 4,
    }
}

fn severity_rank(severity: &str) -> usize {
    match severity {
        "critical" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}

#[cfg(test)]
#[path = "tests_fleet_alerts.rs"]
mod tests;
