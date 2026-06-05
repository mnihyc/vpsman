use std::collections::HashMap;

use anyhow::Result;
use serde_json::{json, Value};
use vpsman_common::payload_hash;

use crate::{
    model::{
        AgentUpdateRolloutView, AgentView, BackupRequestView, DataSourceStatusView,
        FleetAlertQuery, FleetAlertView, JobHistoryView, JobTargetView, TelemetryRollupView,
        TelemetryTunnelView,
    },
    model_alert_policies::FleetAlertPolicyOverrideView,
    model_alert_states::FleetAlertStateView,
    state::AppState,
    unix_now,
};

const DEFAULT_MEMORY_AVAILABLE_CRITICAL_RATIO: f64 = 0.10;
const DEFAULT_MEMORY_AVAILABLE_WARNING_RATIO: f64 = 0.20;
const DEFAULT_DISK_AVAILABLE_CRITICAL_RATIO: f64 = 0.10;
const DEFAULT_DISK_AVAILABLE_WARNING_RATIO: f64 = 0.20;
const DEFAULT_CPU_LOAD_WARNING: f64 = 2.0;
const DEFAULT_CPU_LOAD_CRITICAL: f64 = 4.0;

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

    pub(crate) fn with_override(&self, policy: &FleetAlertPolicyOverrideView) -> Result<Self> {
        Self::new(
            policy
                .memory_available_warning_ratio
                .unwrap_or(self.memory_available_warning_ratio),
            policy
                .memory_available_critical_ratio
                .unwrap_or(self.memory_available_critical_ratio),
            policy
                .disk_available_warning_ratio
                .unwrap_or(self.disk_available_warning_ratio),
            policy
                .disk_available_critical_ratio
                .unwrap_or(self.disk_available_critical_ratio),
            policy.cpu_load_warning.unwrap_or(self.cpu_load_warning),
            policy.cpu_load_critical.unwrap_or(self.cpu_load_critical),
        )
    }

    pub(crate) fn validate_override(policy: &FleetAlertPolicyOverrideView) -> Result<()> {
        if !policy.has_any_threshold() {
            anyhow::bail!("fleet alert policy must configure at least one threshold");
        }
        Self::default().with_override(policy).map(|_| ())
    }
}

impl FleetAlertPolicyOverrideView {
    pub(crate) fn has_any_threshold(&self) -> bool {
        self.memory_available_warning_ratio.is_some()
            || self.memory_available_critical_ratio.is_some()
            || self.disk_available_warning_ratio.is_some()
            || self.disk_available_critical_ratio.is_some()
            || self.cpu_load_warning.is_some()
            || self.cpu_load_critical.is_some()
    }
}

fn validate_ratio_thresholds(name: &str, warning: f64, critical: f64) -> Result<()> {
    if !warning.is_finite() || !critical.is_finite() {
        anyhow::bail!("{name} alert thresholds must be finite numbers");
    }
    if !(0.0..1.0).contains(&warning) || !(0.0..1.0).contains(&critical) {
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

#[derive(Clone, Debug, Default)]
pub(crate) struct AgentAlertScope {
    pub(crate) client_id: String,
    pub(crate) tags: Vec<String>,
    pub(crate) provider: Option<String>,
}

impl AgentAlertScope {
    pub(crate) fn from_client_id(client_id: &str) -> Self {
        Self {
            client_id: client_id.to_string(),
            ..Self::default()
        }
    }
}

pub(crate) fn build_agent_alert_scopes(agents: &[AgentView]) -> HashMap<String, AgentAlertScope> {
    agents
        .iter()
        .map(|agent| {
            (
                agent.id.clone(),
                AgentAlertScope {
                    client_id: agent.id.clone(),
                    tags: agent.tags.clone(),
                    provider: tag_namespace_value(&agent.tags, "provider"),
                },
            )
        })
        .collect()
}

fn effective_policy_for_scope(
    base: &FleetAlertPolicy,
    policies: &[FleetAlertPolicyOverrideView],
    scope: &AgentAlertScope,
) -> Result<(FleetAlertPolicy, Vec<String>)> {
    let mut matching = policies
        .iter()
        .filter(|policy| policy.enabled && alert_policy_matches_scope(policy, scope))
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| {
        alert_policy_specificity(left)
            .cmp(&alert_policy_specificity(right))
            .then_with(|| left.priority.cmp(&right.priority))
            .then_with(|| left.updated_at.cmp(&right.updated_at))
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut effective = base.clone();
    let mut matched_policy_ids = Vec::new();
    for policy in matching {
        effective = effective.with_override(policy)?;
        matched_policy_ids.push(policy.id.to_string());
    }
    Ok((effective, matched_policy_ids))
}

fn alert_policy_matches_scope(
    policy: &FleetAlertPolicyOverrideView,
    scope: &AgentAlertScope,
) -> bool {
    match (policy.scope_kind.as_str(), policy.scope_value.as_deref()) {
        ("global", _) => true,
        ("provider", Some(provider)) => scope.provider.as_deref() == Some(provider),
        ("tag", Some(tag)) => scope.tags.iter().any(|stored| stored == tag),
        ("client", Some(client_id)) => scope.client_id == client_id,
        _ => false,
    }
}

fn alert_policy_specificity(policy: &FleetAlertPolicyOverrideView) -> u8 {
    match policy.scope_kind.as_str() {
        "global" => 0,
        "provider" => 1,
        "tag" => 2,
        "client" => 3,
        _ => 0,
    }
}

fn tag_namespace_value(tags: &[String], namespace: &str) -> Option<String> {
    let prefix = format!("{namespace}:");
    tags.iter()
        .find_map(|tag| tag.strip_prefix(&prefix).filter(|value| !value.is_empty()))
        .map(ToString::to_string)
}

impl AppState {
    pub(crate) async fn list_fleet_alerts(
        &self,
        query: FleetAlertQuery,
    ) -> Result<Vec<FleetAlertView>> {
        let mut alerts = Vec::new();
        let agents = self.repo.list_agents().await?;
        let agents_by_id = agents
            .iter()
            .map(|agent| (agent.id.as_str(), agent))
            .collect::<HashMap<_, _>>();
        let agent_scopes = build_agent_alert_scopes(&agents);
        let alert_policies = self
            .repo
            .list_fleet_alert_policies(1000, Some(true), None, None)
            .await?;
        append_agent_status_alerts(&mut alerts, &agents);

        let rollups = self.repo.list_telemetry_rollups(200, None, None).await?;
        append_resource_alerts(
            &mut alerts,
            &latest_rollups(rollups),
            &self.fleet_alert_policy,
            &alert_policies,
            &agent_scopes,
        )?;

        let tunnels = self.repo.list_telemetry_tunnels(200, None, None).await?;
        append_tunnel_alerts(&mut alerts, &tunnels);

        let source_status = self.list_data_source_status(None, None).await?;
        append_source_readiness_alerts(&mut alerts, &source_status);

        let backup_requests = self.repo.list_backup_requests(200).await?;
        append_backup_request_alerts(&mut alerts, &backup_requests);

        let rollouts = self.repo.list_agent_update_rollouts(200).await?;
        append_update_rollout_alerts(&mut alerts, &rollouts);

        let jobs = self.repo.list_jobs(200).await?;
        append_job_alerts(&mut alerts, &self.repo, &jobs, &agents_by_id).await?;

        let alert_states = self.repo.list_fleet_alert_states(1000, None).await?;
        apply_alert_states(&mut alerts, &alert_states);
        apply_alert_filters(&mut alerts, &query);
        Ok(alerts)
    }
}

fn append_agent_status_alerts(alerts: &mut Vec<FleetAlertView>, agents: &[AgentView]) {
    for agent in agents {
        if agent.status == "connected" || agent.status == "unknown" {
            continue;
        }
        let severity = if agent.status.contains("offline") {
            "critical"
        } else {
            "warning"
        };
        push_alert(
            alerts,
            AlertInput {
                severity,
                category: "agent_status",
                target_kind: "agent",
                target_id: &agent.id,
                client_id: Some(&agent.id),
                title: "Agent is not connected",
                detail: format!("{} currently reports {}", agent.display_name, agent.status),
                status: &agent.status,
                evidence: json!({
                    "display_name": &agent.display_name,
                    "tags": &agent.tags,
                    "capability_privilege_mode": agent.capabilities.privilege_mode,
                }),
                observed_at: unix_now().to_string(),
            },
        );
    }
}

fn append_resource_alerts(
    alerts: &mut Vec<FleetAlertView>,
    rollups: &HashMap<String, TelemetryRollupView>,
    policy: &FleetAlertPolicy,
    overrides: &[FleetAlertPolicyOverrideView],
    agent_scopes: &HashMap<String, AgentAlertScope>,
) -> Result<()> {
    for rollup in rollups.values() {
        let scope = agent_scopes
            .get(&rollup.client_id)
            .cloned()
            .unwrap_or_else(|| AgentAlertScope::from_client_id(&rollup.client_id));
        let (policy, matched_policy_ids) = effective_policy_for_scope(policy, overrides, &scope)?;
        let policy_evidence = json!({
            "matched_policy_ids": matched_policy_ids,
            "scope": {
                "client_id": &scope.client_id,
                "provider": &scope.provider,
                "tags": &scope.tags,
            },
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
        if tunnel
            .adapter_health
            .as_ref()
            .is_some_and(|health| !health.success)
        {
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
        if tunnel.plan_correlation == "stale_saved_plan" {
            push_tunnel_alert(
                alerts,
                "warning",
                tunnel,
                "tunnel_saved_plan_drift",
                "Saved tunnel plan has runtime drift",
                "runtime tunnel telemetry no longer matches the saved plan".to_string(),
                json!({
                    "plan_id": tunnel.plan_id,
                    "plan_name": &tunnel.plan_name,
                    "plan_correlation": &tunnel.plan_correlation,
                }),
            );
        } else if tunnel.promotion_required {
            push_tunnel_alert(
                alerts,
                "info",
                tunnel,
                "tunnel_import_candidate",
                "Observed tunnel needs promotion",
                "operator review is required before this observed tunnel becomes managed"
                    .to_string(),
                json!({
                    "promotion_required": true,
                    "mutation_policy": &tunnel.mutation_policy,
                    "plan_correlation": &tunnel.plan_correlation,
                }),
            );
        }
    }
}

fn append_source_readiness_alerts(alerts: &mut Vec<FleetAlertView>, rows: &[DataSourceStatusView]) {
    for row in rows {
        let severity = match row.status.as_str() {
            "degraded" | "selected_no_store" => "warning",
            "selected_no_samples" | "selected_no_artifacts" | "needs_promotion" => "info",
            _ => continue,
        };
        push_alert(
            alerts,
            AlertInput {
                severity,
                category: "source_readiness",
                target_kind: "data_source",
                target_id: &format!("{}:{}", row.client_id, row.domain),
                client_id: Some(&row.client_id),
                title: "Selected data source needs attention",
                detail: format!("{}: {}", row.module, row.status_reason),
                status: &row.status,
                evidence: json!({
                    "domain": &row.domain,
                    "preset_name": &row.preset_name,
                    "source_kind": &row.source_kind,
                    "evidence": &row.evidence,
                }),
                observed_at: row.assigned_at.clone(),
            },
        );
    }
}

fn append_backup_request_alerts(alerts: &mut Vec<FleetAlertView>, backups: &[BackupRequestView]) {
    for backup in backups {
        if backup.status.contains("failed") || backup.status.contains("rejected") {
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

fn append_update_rollout_alerts(
    alerts: &mut Vec<FleetAlertView>,
    rollouts: &[AgentUpdateRolloutView],
) {
    for rollout in rollouts {
        let severity = if rollout.status.contains("timeout") || rollout.status.contains("failed") {
            "critical"
        } else if rollout.automation_blocker.is_some() || rollout.failed_count > 0 {
            "warning"
        } else {
            continue;
        };
        push_alert(
            alerts,
            AlertInput {
                severity,
                category: "agent_update",
                target_kind: "agent_update_rollout",
                target_id: &rollout.id.to_string(),
                client_id: None,
                title: "Agent update rollout needs attention",
                detail: rollout
                    .automation_blocker
                    .clone()
                    .unwrap_or_else(|| format!("rollout status {}", rollout.status)),
                status: &rollout.status,
                evidence: json!({
                    "job_id": rollout.job_id,
                    "target_count": rollout.target_count,
                    "failed_count": rollout.failed_count,
                    "pending_count": rollout.pending_count,
                    "automation_status": &rollout.automation_status,
                    "automation_next_action": &rollout.automation_next_action,
                    "automation_targets": &rollout.automation_targets,
                }),
                observed_at: rollout.updated_at.clone(),
            },
        );
    }
}

async fn append_job_alerts(
    alerts: &mut Vec<FleetAlertView>,
    repo: &crate::repository::Repository,
    jobs: &[JobHistoryView],
    agents_by_id: &HashMap<&str, &AgentView>,
) -> Result<()> {
    for job in jobs {
        if failed_status(&job.status)
            && (job.command_type.contains("backup") || job.command_type.contains("restore"))
        {
            push_job_alert(
                alerts,
                "critical",
                job,
                "backup",
                "Backup or restore job failed",
                format!("{} job {}", job.command_type, job.status),
                json!({"command_type": &job.command_type, "target_count": job.target_count}),
            );
        } else if failed_status(&job.status) && job.command_type.contains("agent_update") {
            push_job_alert(
                alerts,
                "critical",
                job,
                "agent_update",
                "Agent update job failed",
                format!("agent update job {}", job.status),
                json!({"command_type": &job.command_type, "target_count": job.target_count}),
            );
        }

        let targets = repo.list_job_targets(job.id).await?;
        append_unprivileged_target_alerts(alerts, job, &targets, agents_by_id);
    }
    Ok(())
}

fn append_unprivileged_target_alerts(
    alerts: &mut Vec<FleetAlertView>,
    job: &JobHistoryView,
    targets: &[JobTargetView],
    agents_by_id: &HashMap<&str, &AgentView>,
) {
    for target in targets {
        if !target.status.contains("unprivileged") {
            continue;
        }
        let agent_hint = agents_by_id
            .get(target.client_id.as_str())
            .and_then(|agent| agent.capabilities.unprivileged_hint.clone());
        push_alert(
            alerts,
            AlertInput {
                severity: "warning",
                category: "unprivileged_blocked",
                target_kind: "job_target",
                target_id: &format!("{}:{}", job.id, target.client_id),
                client_id: Some(&target.client_id),
                title: "Privileged operation degraded on unprivileged agent",
                detail: agent_hint
                    .unwrap_or_else(|| format!("{} reported {}", target.client_id, target.status)),
                status: &target.status,
                evidence: json!({
                    "job_id": job.id,
                    "command_type": &job.command_type,
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

fn failed_status(status: &str) -> bool {
    status.contains("failed")
        || status.contains("rejected")
        || status.contains("timeout")
        || status.contains("error")
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
        "status": input.status,
        "evidence": input.evidence,
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

fn apply_alert_filters(alerts: &mut Vec<FleetAlertView>, query: &FleetAlertQuery) {
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
            .then_with(|| right.observed_at.cmp(&left.observed_at))
            .then_with(|| left.category.cmp(&right.category))
            .then_with(|| left.target_id.cmp(&right.target_id))
    });
    alerts.truncate(query.limit.unwrap_or(50).clamp(1, 200) as usize);
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
mod tests {
    use super::*;

    #[test]
    fn fleet_alert_policy_rejects_invalid_thresholds() {
        assert!(FleetAlertPolicy::new(0.10, 0.20, 0.20, 0.10, 2.0, 4.0).is_err());
        assert!(FleetAlertPolicy::new(0.20, 0.10, 0.10, 0.20, 2.0, 4.0).is_err());
        assert!(FleetAlertPolicy::new(0.20, 0.10, 0.20, 0.10, 4.0, 2.0).is_err());
        assert!(FleetAlertPolicy::new(0.20, 0.10, 0.20, 0.10, 2.0, 4.0).is_ok());
    }

    #[test]
    fn resource_alerts_use_configurable_policy_thresholds() {
        let policy = FleetAlertPolicy::new(0.50, 0.25, 0.40, 0.15, 1.0, 2.5).unwrap();
        let mut rollups = HashMap::new();
        rollups.insert(
            "edge-a".to_string(),
            TelemetryRollupView {
                client_id: "edge-a".to_string(),
                bucket_start: "100".to_string(),
                bucket_secs: 60,
                sample_count: 3,
                cpu_load_1_avg: 1.5,
                cpu_load_1_max: 2.6,
                memory_total_bytes_max: 1000,
                memory_available_bytes_avg: 400,
                memory_available_bytes_min: 300,
                disk_total_bytes_max: 2000,
                disk_available_bytes_avg: 500,
                disk_available_bytes_min: 200,
                network_rx_bytes_max: 0,
                network_tx_bytes_max: 0,
                latest_observed_at: "120".to_string(),
                updated_at: "121".to_string(),
            },
        );

        let mut alerts = Vec::new();
        append_resource_alerts(&mut alerts, &rollups, &policy, &[], &HashMap::new()).unwrap();

        let cpu = find_status(&alerts, "cpu_load_high");
        assert_eq!(cpu.severity, "critical");
        assert_eq!(
            cpu.evidence["threshold"].as_f64().unwrap(),
            policy.cpu_load_critical
        );

        let memory = find_status(&alerts, "memory_low");
        assert_eq!(memory.severity, "warning");
        assert_eq!(
            memory.evidence["warning_threshold"].as_f64().unwrap(),
            policy.memory_available_warning_ratio
        );

        let disk = find_status(&alerts, "disk_low");
        assert_eq!(disk.severity, "critical");
        assert_eq!(
            disk.evidence["critical_threshold"].as_f64().unwrap(),
            policy.disk_available_critical_ratio
        );
    }

    fn find_status<'a>(alerts: &'a [FleetAlertView], status: &str) -> &'a FleetAlertView {
        alerts
            .iter()
            .find(|alert| alert.status == status)
            .unwrap_or_else(|| panic!("missing {status} in {alerts:#?}"))
    }
}
