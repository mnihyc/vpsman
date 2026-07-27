use std::collections::HashMap;

use anyhow::Result;
use serde_json::json;
use sqlx::Row;

use crate::{
    model::{
        AgentView, SourceStatusView, SourceTemplateAssignmentView, SourceTemplateView,
        TelemetryTunnelView,
    },
    repository::Repository,
    repository_telemetry_rollups::tunnel_adapter_health_is_degraded,
    source_template_builtins::SOURCE_TEMPLATE_DOMAINS,
};

const SOURCE_STATUS_EVIDENCE_SAMPLE_LIMIT: usize = 100;

#[derive(Clone, Debug, Default)]
pub(crate) struct BackupSourceEvidenceCounts {
    pub(crate) artifact_count: usize,
    pub(crate) backup_request_count: usize,
    pub(crate) restore_source_count: usize,
    pub(crate) restore_target_count: usize,
    pub(crate) migration_source_count: usize,
    pub(crate) migration_target_count: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UpdateSourceEvidenceCounts {
    pub(crate) release_count: usize,
    pub(crate) external_release_count: usize,
}

impl Repository {
    pub(crate) async fn list_source_status(
        &self,
        client_id: Option<&str>,
        domain: Option<&str>,
    ) -> Result<Vec<SourceStatusView>> {
        let agents = self
            .list_agents()
            .await?
            .into_iter()
            .filter(|agent| client_id.is_none_or(|client_id| agent.id == client_id))
            .collect::<Vec<_>>();
        self.list_source_status_for_agents(&agents, domain).await
    }

    pub(crate) async fn list_source_status_for_agents(
        &self,
        agents: &[AgentView],
        domain: Option<&str>,
    ) -> Result<Vec<SourceStatusView>> {
        if agents.is_empty() {
            return Ok(Vec::new());
        }
        let client_ids = agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();
        let templates = self
            .list_source_templates_for_status(domain)
            .await?
            .into_iter()
            .map(|template| (template.id, template))
            .collect::<HashMap<_, _>>();
        let assignments = self
            .list_source_template_assignments_for_clients(&client_ids, domain)
            .await?
            .into_iter()
            .filter(|assignment| templates.contains_key(&assignment.template_id))
            .collect::<Vec<_>>();
        let tunnels = self
            .list_declared_telemetry_tunnels_for_source_status_clients(&client_ids)
            .await?;
        let tunnels_by_client = tunnels.into_iter().fold(
            HashMap::<String, Vec<TelemetryTunnelView>>::new(),
            |mut grouped, tunnel| {
                grouped
                    .entry(tunnel.client_id.clone())
                    .or_default()
                    .push(tunnel);
                grouped
            },
        );
        let agents_by_id = agents
            .iter()
            .map(|agent| (agent.id.as_str(), agent))
            .collect::<HashMap<_, _>>();

        let mut rows = Vec::new();
        for assignment in assignments {
            let Some(agent) = agents_by_id.get(assignment.client_id.as_str()) else {
                continue;
            };
            let Some(template) = templates.get(&assignment.template_id) else {
                continue;
            };
            rows.push(status_for_assignment(
                agent,
                &assignment,
                template,
                tunnels_by_client
                    .get(&assignment.client_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            ));
        }
        rows.sort_by(|left, right| {
            left.client_id
                .cmp(&right.client_id)
                .then_with(|| domain_order(&left.domain).cmp(&domain_order(&right.domain)))
                .then_with(|| left.domain.cmp(&right.domain))
        });
        Ok(rows)
    }

    pub(crate) async fn source_backup_evidence_counts(
        &self,
        client_ids: &[String],
    ) -> Result<HashMap<String, BackupSourceEvidenceCounts>> {
        if client_ids.is_empty() {
            return Ok(HashMap::new());
        }
        match self {
            Self::Memory(memory) => {
                let mut counts = client_ids
                    .iter()
                    .cloned()
                    .map(|client_id| (client_id, BackupSourceEvidenceCounts::default()))
                    .collect::<HashMap<_, _>>();
                for artifact in memory.backup_artifacts.read().await.iter() {
                    if let Some(count) = counts.get_mut(&artifact.client_id) {
                        count.artifact_count += 1;
                    }
                }
                for request in memory.backup_requests.read().await.iter() {
                    if let Some(count) = counts.get_mut(&request.client_id) {
                        count.backup_request_count += 1;
                    }
                }
                for plan in memory.restore_plans.read().await.iter() {
                    if let Some(count) = counts.get_mut(&plan.source_client_id) {
                        count.restore_source_count += 1;
                    }
                    if let Some(count) = counts.get_mut(&plan.target_client_id) {
                        count.restore_target_count += 1;
                    }
                }
                for link in memory.migration_links.read().await.iter() {
                    if let Some(count) = counts.get_mut(&link.source_client_id) {
                        count.migration_source_count += 1;
                    }
                    if let Some(count) = counts.get_mut(&link.target_client_id) {
                        count.migration_target_count += 1;
                    }
                }
                Ok(counts)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        selected.client_id,
                        (
                            SELECT count(*)::bigint
                            FROM backup_artifacts artifact
                            WHERE artifact.client_id = selected.client_id
                        ) AS artifact_count,
                        (
                            SELECT count(*)::bigint
                            FROM backup_requests request
                            WHERE request.client_id = selected.client_id
                        ) AS backup_request_count,
                        (
                            SELECT count(*)::bigint
                            FROM restore_plans plan
                            WHERE plan.source_client_id = selected.client_id
                        ) AS restore_source_count,
                        (
                            SELECT count(*)::bigint
                            FROM restore_plans plan
                            WHERE plan.target_client_id = selected.client_id
                        ) AS restore_target_count,
                        (
                            SELECT count(*)::bigint
                            FROM migration_links link
                            WHERE link.source_client_id = selected.client_id
                        ) AS migration_source_count,
                        (
                            SELECT count(*)::bigint
                            FROM migration_links link
                            WHERE link.target_client_id = selected.client_id
                        ) AS migration_target_count
                    FROM unnest($1::text[]) AS selected(client_id)
                    "#,
                )
                .bind(client_ids)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let count = |name| -> Result<usize> {
                            Ok(row
                                .try_get::<i64, _>(name)?
                                .max(0)
                                .try_into()
                                .unwrap_or(usize::MAX))
                        };
                        let client_id: String = row.try_get("client_id")?;
                        Ok((
                            client_id,
                            BackupSourceEvidenceCounts {
                                artifact_count: count("artifact_count")?,
                                backup_request_count: count("backup_request_count")?,
                                restore_source_count: count("restore_source_count")?,
                                restore_target_count: count("restore_target_count")?,
                                migration_source_count: count("migration_source_count")?,
                                migration_target_count: count("migration_target_count")?,
                            },
                        ))
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn source_update_evidence_counts(&self) -> Result<UpdateSourceEvidenceCounts> {
        match self {
            Self::Memory(memory) => {
                let releases = memory.agent_update_releases.read().await;
                Ok(UpdateSourceEvidenceCounts {
                    release_count: releases.len(),
                    external_release_count: releases
                        .iter()
                        .filter(|release| release.artifact_url_sha256_hex.is_some())
                        .count(),
                })
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        count(*)::bigint AS release_count,
                        count(*) FILTER (
                            WHERE artifact_url_sha256_hex IS NOT NULL
                        )::bigint AS external_release_count
                    FROM agent_update_releases
                    "#,
                )
                .fetch_one(pool)
                .await?;
                Ok(UpdateSourceEvidenceCounts {
                    release_count: row
                        .try_get::<i64, _>("release_count")?
                        .max(0)
                        .try_into()
                        .unwrap_or(usize::MAX),
                    external_release_count: row
                        .try_get::<i64, _>("external_release_count")?
                        .max(0)
                        .try_into()
                        .unwrap_or(usize::MAX),
                })
            }
        }
    }
}

fn status_for_assignment(
    agent: &AgentView,
    assignment: &SourceTemplateAssignmentView,
    template: &SourceTemplateView,
    tunnels: &[TelemetryTunnelView],
) -> SourceStatusView {
    let source_kind = source_kind(template);
    let (status, status_reason, evidence) = derive_status(agent, template, &source_kind, tunnels);
    SourceStatusView {
        client_id: assignment.client_id.clone(),
        display_name: agent.display_name.clone(),
        client_status: agent.status.clone(),
        domain: assignment.domain.clone(),
        module: module_label(&assignment.domain).to_string(),
        template_id: assignment.template_id,
        template_name: assignment.template_name.clone(),
        template_scope: assignment.template_scope.clone(),
        source_kind,
        status,
        status_reason,
        evidence,
        assigned_at: assignment.assigned_at.clone(),
    }
}

fn derive_status(
    agent: &AgentView,
    template: &SourceTemplateView,
    source_kind: &str,
    tunnels: &[TelemetryTunnelView],
) -> (String, String, serde_json::Value) {
    let domain = template.domain.as_str();
    if agent.status != "online" {
        return (
            "agent_offline".to_string(),
            "selected template exists, but the agent is not currently online".to_string(),
            json!({
                "agent_status": agent.status,
                "continuous_status": false,
            }),
        );
    }

    match domain {
        "telemetry_metrics_source" => (
            "selected".to_string(),
            "agent is online; telemetry source is selected in runtime config".to_string(),
            json!({
                "agent_status": agent.status,
                "continuous_status": true,
            }),
        ),
        "runtime_traffic_accounting_source" => traffic_status(source_kind, tunnels),
        "runtime_tunnel_adapter" => tunnel_adapter_status(tunnels),
        "latency_probe_source" => latency_probe_status(template, source_kind),
        "speed_test_provider" => speed_test_status(template, source_kind),
        "process_inventory_source" => process_inventory_status(agent, template, source_kind),
        "user_session_inventory_source" => user_session_inventory_status(template, source_kind),
        "command_execution_policy" => command_execution_policy_status(template),
        "process_supervisor_policy" => process_supervisor_policy_status(agent, template, source_kind),
        "backup_object_store" | "update_artifact_source" => (
            "selected_workflow".to_string(),
            "template is selected; status is produced when the related privilege-gated workflow runs"
                .to_string(),
            json!({
                "agent_status": agent.status,
                "continuous_status": false,
            }),
        ),
        "restore_path_mapping" => restore_path_mapping_status(template, source_kind),
        "update_restart_policy" => update_restart_policy_status(template, source_kind),
        "update_rollback_heartbeat_source" => update_rollback_heartbeat_status(template, source_kind),
        "traffic_limit_status_source" => traffic_limit_status_source_status(template, source_kind),
        "routing_cost_adapter" => routing_cost_adapter_status(template),
        _ => (
            "unknown_domain".to_string(),
            "domain is selected but has no status policy yet".to_string(),
            json!({
                "agent_status": agent.status,
                "continuous_status": false,
            }),
        ),
    }
}

fn latency_probe_status(
    template: &SourceTemplateView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    let configured_ping = template.definition.get("probe_ping_argv").is_some()
        || template.definition.get("ping_argv").is_some()
        || template.definition.get("argv").is_some();
    (
        "ready_on_demand".to_string(),
        format!(
            "latency probe template {source_kind} is selected; tunnel probe jobs produce samples on demand"
        ),
        json!({
            "continuous_status": false,
            "workflow": "network_probe",
            "command_types": ["network_probe"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "configured_ping_argv": configured_ping,
            "sample_status": "on_demand",
        }),
    )
}

fn speed_test_status(
    template: &SourceTemplateView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    let configured_adapter = template.definition.get("server_argv").is_some()
        || template.definition.get("client_argv").is_some();
    (
        "ready_on_demand".to_string(),
        format!(
            "speed-test provider {source_kind} is selected; paired tunnel speed tests produce samples on demand"
        ),
        json!({
            "continuous_status": false,
            "workflow": "network_speed_test",
            "command_types": ["network_speed_test"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "configured_adapter_argv": configured_adapter,
            "requires_two_endpoints": true,
            "sample_status": "on_demand",
        }),
    )
}

fn process_inventory_status(
    agent: &AgentView,
    template: &SourceTemplateView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    let custom_command = template.definition.get("process_command").is_some()
        || template.definition.get("command").is_some();
    let process_limits_status = if agent.capabilities.can_apply_process_limits {
        "available"
    } else if agent.capabilities.privilege_mode == vpsman_common::AgentPrivilegeMode::Unprivileged {
        "degraded_unprivileged"
    } else {
        "unknown_or_unavailable"
    };
    (
        "ready_on_demand".to_string(),
        format!("process inventory source {source_kind} is selected; process and supervisor jobs produce snapshots on demand"),
        json!({
            "continuous_status": false,
            "workflow": "process_inventory",
            "supervisor_workflow": "process_supervisor",
            "command_types": [
                "process_list",
                "process_start",
                "process_status",
                "process_logs",
                "process_restart",
                "process_stop"
            ],
            "privilege_gated": true,
            "source_kind": source_kind,
            "custom_command_configured": custom_command,
            "snapshot_status": "on_demand",
            "privilege_mode": agent.capabilities.privilege_mode,
            "effective_uid_known": agent.capabilities.effective_uid.is_some(),
            "can_apply_process_limits": agent.capabilities.can_apply_process_limits,
            "process_limits_status": process_limits_status,
            "process_limits_source": "agent_capability_snapshot",
            "unprivileged_hint": agent.capabilities.unprivileged_hint.clone(),
        }),
    )
}

fn user_session_inventory_status(
    template: &SourceTemplateView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    let configured_command = template.definition.get("user_sessions_command").is_some()
        || template.definition.get("command").is_some();
    (
        "ready_on_demand".to_string(),
        format!(
            "user/session inventory source {source_kind} is selected; user-sessions jobs produce snapshots on demand"
        ),
        json!({
            "continuous_status": false,
            "workflow": "user_session_inventory",
            "command_types": ["user_sessions"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "custom_command_configured": configured_command,
            "snapshot_status": "on_demand",
        }),
    )
}

fn command_execution_policy_status(
    template: &SourceTemplateView,
) -> (String, String, serde_json::Value) {
    let shell_argv_len = template
        .definition
        .get("shell_script_argv")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let environment_policy = template
        .definition
        .get("environment_policy")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("inherit");
    let pty_policy = template
        .definition
        .get("pty_policy")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("native_pty");
    let process_cleanup = template
        .definition
        .get("process_cleanup")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("process_group");
    let environment_set_keys = template
        .definition
        .get("environment_set")
        .and_then(serde_json::Value::as_object)
        .map(|values| values.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    (
        "ready_on_demand".to_string(),
        "command execution policy is selected for privilege-gated argv, shell-script, terminal, and inventory workflows"
            .to_string(),
        json!({
            "continuous_status": false,
            "workflow": "command_execution",
            "command_types": ["shell_argv", "shell_script", "shell_pty", "terminal_open", "user_sessions"],
            "privilege_gated": true,
            "shell_script_argv_len": shell_argv_len,
            "working_directory_configured": template.definition.get("working_directory").is_some(),
            "environment_policy": environment_policy,
            "environment_set_keys": environment_set_keys,
            "pty_policy": pty_policy,
            "process_cleanup": process_cleanup,
        }),
    )
}

fn process_supervisor_policy_status(
    agent: &AgentView,
    template: &SourceTemplateView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    let process_limits_status = if agent.capabilities.can_apply_process_limits {
        "available"
    } else if agent.capabilities.privilege_mode == vpsman_common::AgentPrivilegeMode::Unprivileged {
        "degraded_unprivileged"
    } else {
        "unknown_or_unavailable"
    };
    (
        "ready_on_demand".to_string(),
        format!(
            "process supervisor policy {source_kind} is selected; supervisor jobs report restart and limit evidence on demand"
        ),
        json!({
            "continuous_status": false,
            "workflow": "process_supervisor",
            "command_types": ["process_start", "process_status", "process_logs", "process_restart", "process_stop"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "restart_policy_source": template.definition.get("restart_policy_source").and_then(serde_json::Value::as_str).unwrap_or("process_run_policy"),
            "limit_source": template.definition.get("limit_source").and_then(serde_json::Value::as_str).unwrap_or("agent_capability_snapshot"),
            "privilege_mode": agent.capabilities.privilege_mode,
            "can_apply_process_limits": agent.capabilities.can_apply_process_limits,
            "process_limits_status": process_limits_status,
            "unprivileged_hint": agent.capabilities.unprivileged_hint.clone(),
        }),
    )
}

fn restore_path_mapping_status(
    template: &SourceTemplateView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    (
        "ready_on_demand".to_string(),
        format!(
            "restore path-mapping template {source_kind} is selected; restore and migration plans provide concrete mappings"
        ),
        json!({
            "continuous_status": false,
            "workflow": "restore_migration",
            "command_types": ["restore_run", "restore_rollback", "migration_run"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "mapping_mode": template.definition.get("mapping_mode").and_then(serde_json::Value::as_str).unwrap_or("explicit_paths"),
            "supports_agent_local_archive": template.definition.get("supports_agent_local_archive").and_then(serde_json::Value::as_bool).unwrap_or(false),
            "supports_post_restore_hooks": template.definition.get("supports_post_restore_hooks").and_then(serde_json::Value::as_bool).unwrap_or(false),
        }),
    )
}

fn update_restart_policy_status(
    template: &SourceTemplateView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    (
        "ready_on_demand".to_string(),
        format!(
            "update restart policy {source_kind} is selected; agent-update activation jobs provide restart evidence"
        ),
        json!({
            "continuous_status": false,
            "workflow": "agent_update_activation",
            "command_types": ["agent_update_activate", "agent_update_rollback"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "restart_method": template.definition.get("restart_method").and_then(serde_json::Value::as_str).unwrap_or("agent_configured"),
            "fallback": template.definition.get("fallback").and_then(serde_json::Value::as_str).unwrap_or("manual_supervisor"),
        }),
    )
}

fn update_rollback_heartbeat_status(
    template: &SourceTemplateView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    (
        "ready_on_demand".to_string(),
        format!(
            "rollback heartbeat source {source_kind} is selected; agent-update jobs report heartbeat and activation evidence"
        ),
        json!({
            "continuous_status": false,
            "workflow": "agent_update_jobs",
            "command_types": ["agent_update", "agent_update_activate", "agent_update_rollback"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "health_gate": template.definition.get("health_gate").and_then(serde_json::Value::as_str).unwrap_or("heartbeat_verified"),
            "heartbeat_source": template.definition.get("source").and_then(serde_json::Value::as_str).unwrap_or("agent_update_heartbeat"),
        }),
    )
}

fn traffic_limit_status_source_status(
    template: &SourceTemplateView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    (
        "ready_on_demand".to_string(),
        format!(
            "traffic-limit status source {source_kind} is selected; tunnel plans and status jobs provide enforcement evidence"
        ),
        json!({
            "continuous_status": true,
            "workflow": "runtime_traffic_limits",
            "command_types": ["runtime_config_sync", "network_status", "tunnel_speed_test"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "status_source": template.definition.get("status_source").and_then(serde_json::Value::as_str).unwrap_or("network_status_and_telemetry"),
        }),
    )
}

fn routing_cost_adapter_status(
    template: &SourceTemplateView,
) -> (String, String, serde_json::Value) {
    (
        "ready_on_demand".to_string(),
        "routing cost adapter is available for explicit tunnel endpoint jobs".to_string(),
        json!({
            "continuous_status": false,
            "workflow": "network_routing_adapter",
            "command_types": ["network_routing_status", "network_routing_apply"],
            "privilege_gated": true,
            "contract_version": template.definition.get("contract_version"),
            "status_command_configured": template.definition.get("status_command").is_some(),
            "update_command_configured": template.definition.get("update_command").is_some(),
        }),
    )
}

fn traffic_status(
    source_kind: &str,
    tunnels: &[TelemetryTunnelView],
) -> (String, String, serde_json::Value) {
    let status_tunnels = tunnels
        .iter()
        .filter(|tunnel| tunnel.traffic_status.is_some())
        .collect::<Vec<_>>();
    if status_tunnels.is_empty() {
        return (
            "selected_no_samples".to_string(),
            format!("{source_kind} is selected, but no runtime traffic samples are available yet"),
            json!({
                "continuous_status": true,
                "sample_count": 0,
            }),
        );
    }
    let degraded_count = status_tunnels
        .iter()
        .filter(|tunnel| tunnel.traffic_status.as_deref() != Some("ok"))
        .count();
    let samples = status_tunnels
        .iter()
        .copied()
        .filter(|tunnel| tunnel.traffic_status.as_deref() != Some("ok"))
        .chain(
            status_tunnels
                .iter()
                .copied()
                .filter(|tunnel| tunnel.traffic_status.as_deref() == Some("ok")),
        )
        .take(SOURCE_STATUS_EVIDENCE_SAMPLE_LIMIT)
        .map(|tunnel| {
            json!({
                "interface": tunnel.interface,
                "traffic_source": tunnel.traffic_source,
                "traffic_status": tunnel.traffic_status,
                "traffic_reason": tunnel.traffic_reason,
                "traffic_checked_unix": tunnel.traffic_checked_unix,
            })
        })
        .collect::<Vec<_>>();
    let sample_count = status_tunnels.len();
    let truncated_count = sample_count.saturating_sub(samples.len());
    if degraded_count > 0 {
        (
            "degraded".to_string(),
            "one or more runtime traffic sources reported degraded status".to_string(),
            json!({
                "continuous_status": true,
                "sample_count": sample_count,
                "degraded_count": degraded_count,
                "samples": samples,
                "truncated_count": truncated_count,
            }),
        )
    } else {
        (
            "ok".to_string(),
            "runtime traffic source is reporting healthy samples".to_string(),
            json!({
                "continuous_status": true,
                "sample_count": sample_count,
                "degraded_count": degraded_count,
                "samples": samples,
                "truncated_count": truncated_count,
            }),
        )
    }
}

fn tunnel_adapter_status(tunnels: &[TelemetryTunnelView]) -> (String, String, serde_json::Value) {
    if tunnels.is_empty() {
        return (
            "selected_no_samples".to_string(),
            "runtime tunnel adapter is selected, but no tunnel telemetry is available yet"
                .to_string(),
            json!({
                "continuous_status": true,
                "sample_count": 0,
            }),
        );
    }
    let is_degraded = |tunnel: &&TelemetryTunnelView| tunnel_adapter_health_is_degraded(tunnel);
    let degraded_count = tunnels.iter().filter(is_degraded).count();
    let samples = tunnels
        .iter()
        .filter(is_degraded)
        .chain(tunnels.iter().filter(|tunnel| !is_degraded(tunnel)))
        .take(SOURCE_STATUS_EVIDENCE_SAMPLE_LIMIT)
        .map(|tunnel| {
            json!({
                "interface": tunnel.interface,
                "plan_id": tunnel.plan_id,
                "plan_name": tunnel.plan_name,
                "adapter_status": tunnel.adapter_health.as_ref().map(|health| health.status.as_str()),
            })
        })
        .collect::<Vec<_>>();
    let sample_count = tunnels.len();
    let truncated_count = sample_count.saturating_sub(samples.len());
    if degraded_count > 0 {
        (
            "degraded".to_string(),
            "runtime tunnel telemetry reports adapter or saved-plan drift".to_string(),
            json!({
                "continuous_status": true,
                "sample_count": sample_count,
                "degraded_count": degraded_count,
                "samples": samples,
                "truncated_count": truncated_count,
            }),
        )
    } else {
        (
            "ok".to_string(),
            "runtime tunnel telemetry matches selected adapter policy".to_string(),
            json!({
                "continuous_status": true,
                "sample_count": sample_count,
                "degraded_count": degraded_count,
                "samples": samples,
                "truncated_count": truncated_count,
            }),
        )
    }
}

fn source_kind(template: &SourceTemplateView) -> String {
    for key in ["source", "provider", "manager"] {
        if let Some(value) = template
            .definition
            .get(key)
            .and_then(serde_json::Value::as_str)
        {
            return value.to_string();
        }
    }
    if template.definition.get("shell_script_argv").is_some() {
        return "shell_script_argv".to_string();
    }
    if let Some(value) = template
        .definition
        .get("status_source")
        .and_then(serde_json::Value::as_str)
    {
        return value.to_string();
    }
    "preset_definition".to_string()
}

fn module_label(domain: &str) -> &'static str {
    match domain {
        "telemetry_metrics_source" => "Telemetry metrics",
        "runtime_traffic_accounting_source" => "Runtime traffic accounting",
        "latency_probe_source" => "Latency probes",
        "speed_test_provider" => "Speed tests",
        "process_inventory_source" => "Process inventory",
        "user_session_inventory_source" => "User/session inventory",
        "command_execution_policy" => "Command execution policy",
        "process_supervisor_policy" => "Process supervisor policy",
        "runtime_tunnel_adapter" => "Runtime tunnel adapter",
        "traffic_limit_status_source" => "Traffic-limit status",
        "routing_cost_adapter" => "Routing cost adapter",
        "backup_object_store" => "Backup object store",
        "restore_path_mapping" => "Restore path mapping",
        "update_artifact_source" => "Update artifact source",
        "update_restart_policy" => "Update restart policy",
        "update_rollback_heartbeat_source" => "Update heartbeat source",
        _ => "Custom source template domain",
    }
}

fn domain_order(domain: &str) -> usize {
    SOURCE_TEMPLATE_DOMAINS
        .iter()
        .position(|candidate| *candidate == domain)
        .unwrap_or(SOURCE_TEMPLATE_DOMAINS.len())
}
