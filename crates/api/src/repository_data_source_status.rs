use std::collections::HashMap;

use anyhow::Result;
use serde_json::json;

use crate::{
    data_source_builtin_presets::DATA_SOURCE_DOMAINS,
    model::{
        AgentView, DataSourcePresetAssignmentView, DataSourcePresetView, DataSourceStatusView,
        TelemetryTunnelView,
    },
    repository::Repository,
};

impl Repository {
    pub(crate) async fn list_data_source_status(
        &self,
        client_id: Option<&str>,
        domain: Option<&str>,
    ) -> Result<Vec<DataSourceStatusView>> {
        let agents = self
            .list_agents()
            .await?
            .into_iter()
            .filter(|agent| client_id.is_none_or(|client_id| agent.id == client_id))
            .collect::<Vec<_>>();
        let presets = self
            .list_data_source_presets(domain)
            .await?
            .into_iter()
            .map(|preset| (preset.id, preset))
            .collect::<HashMap<_, _>>();
        let assignments = self
            .list_data_source_assignments(client_id, domain)
            .await?
            .into_iter()
            .filter(|assignment| presets.contains_key(&assignment.preset_id))
            .collect::<Vec<_>>();
        let tunnels = self.list_telemetry_tunnels(200, client_id, None).await?;
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
            let Some(preset) = presets.get(&assignment.preset_id) else {
                continue;
            };
            rows.push(status_for_assignment(
                agent,
                &assignment,
                preset,
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
}

fn status_for_assignment(
    agent: &AgentView,
    assignment: &DataSourcePresetAssignmentView,
    preset: &DataSourcePresetView,
    tunnels: &[TelemetryTunnelView],
) -> DataSourceStatusView {
    let source_kind = source_kind(preset);
    let (status, status_reason, evidence) = derive_status(agent, preset, &source_kind, tunnels);
    DataSourceStatusView {
        client_id: assignment.client_id.clone(),
        display_name: agent.display_name.clone(),
        client_status: agent.status.clone(),
        domain: assignment.domain.clone(),
        module: module_label(&assignment.domain).to_string(),
        preset_id: assignment.preset_id,
        preset_name: assignment.preset_name.clone(),
        preset_scope: assignment.preset_scope.clone(),
        source_kind,
        status,
        status_reason,
        evidence,
        assigned_at: assignment.assigned_at.clone(),
    }
}

fn derive_status(
    agent: &AgentView,
    preset: &DataSourcePresetView,
    source_kind: &str,
    tunnels: &[TelemetryTunnelView],
) -> (String, String, serde_json::Value) {
    let domain = preset.domain.as_str();
    if agent.status != "online" {
        return (
            "agent_offline".to_string(),
            "selected preset exists, but the agent is not currently online".to_string(),
            json!({
                "agent_status": agent.status,
                "continuous_status": false,
            }),
        );
    }

    match domain {
        "telemetry_metrics_source" => (
            "selected".to_string(),
            "agent is online; telemetry source is selected in agent config".to_string(),
            json!({
                "agent_status": agent.status,
                "continuous_status": true,
            }),
        ),
        "runtime_traffic_accounting_source" => traffic_status(source_kind, tunnels),
        "runtime_tunnel_adapter" => tunnel_adapter_status(tunnels),
        "latency_probe_source" => latency_probe_status(preset, source_kind),
        "speed_test_provider" => speed_test_status(preset, source_kind),
        "process_inventory_source" => process_inventory_status(agent, preset, source_kind),
        "user_session_inventory_source" => user_session_inventory_status(preset, source_kind),
        "command_execution_policy" => command_execution_policy_status(preset),
        "process_supervisor_policy" => process_supervisor_policy_status(agent, preset, source_kind),
        "backup_object_store" | "update_artifact_source" => (
            "selected_workflow".to_string(),
            "preset is selected; status is produced when the related privilege-gated workflow runs"
                .to_string(),
            json!({
                "agent_status": agent.status,
                "continuous_status": false,
            }),
        ),
        "restore_path_mapping" => restore_path_mapping_status(preset, source_kind),
        "update_restart_policy" => update_restart_policy_status(preset, source_kind),
        "update_rollback_heartbeat_source" => update_rollback_heartbeat_status(preset, source_kind),
        "traffic_limit_status_source" => traffic_limit_status_source_status(preset, source_kind),
        "routing_daemon_adapter" => routing_daemon_adapter_status(preset, source_kind),
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
    preset: &DataSourcePresetView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    let configured_ping = preset.definition.get("probe_ping_argv").is_some()
        || preset.definition.get("ping_argv").is_some()
        || preset.definition.get("argv").is_some();
    (
        "ready_on_demand".to_string(),
        format!(
            "latency probe preset {source_kind} is selected; tunnel probe jobs produce samples on demand"
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
    preset: &DataSourcePresetView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    let configured_adapter = preset.definition.get("server_argv").is_some()
        || preset.definition.get("client_argv").is_some();
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
    preset: &DataSourcePresetView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    let custom_command = preset.definition.get("process_command").is_some()
        || preset.definition.get("command").is_some();
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
    preset: &DataSourcePresetView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    let configured_command = preset.definition.get("user_sessions_command").is_some()
        || preset.definition.get("command").is_some();
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
    preset: &DataSourcePresetView,
) -> (String, String, serde_json::Value) {
    let shell_argv_len = preset
        .definition
        .get("shell_script_argv")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let environment_policy = preset
        .definition
        .get("environment_policy")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("inherit");
    let pty_policy = preset
        .definition
        .get("pty_policy")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("native_pty");
    let process_cleanup = preset
        .definition
        .get("process_cleanup")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("process_group");
    let environment_set_keys = preset
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
            "working_directory_configured": preset.definition.get("working_directory").is_some(),
            "environment_policy": environment_policy,
            "environment_set_keys": environment_set_keys,
            "pty_policy": pty_policy,
            "process_cleanup": process_cleanup,
        }),
    )
}

fn process_supervisor_policy_status(
    agent: &AgentView,
    preset: &DataSourcePresetView,
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
            "restart_policy_source": preset.definition.get("restart_policy_source").and_then(serde_json::Value::as_str).unwrap_or("process_run_policy"),
            "limit_source": preset.definition.get("limit_source").and_then(serde_json::Value::as_str).unwrap_or("agent_capability_snapshot"),
            "privilege_mode": agent.capabilities.privilege_mode,
            "can_apply_process_limits": agent.capabilities.can_apply_process_limits,
            "process_limits_status": process_limits_status,
            "unprivileged_hint": agent.capabilities.unprivileged_hint.clone(),
        }),
    )
}

fn restore_path_mapping_status(
    preset: &DataSourcePresetView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    (
        "ready_on_demand".to_string(),
        format!(
            "restore path-mapping preset {source_kind} is selected; restore and migration plans provide concrete mappings"
        ),
        json!({
            "continuous_status": false,
            "workflow": "restore_migration",
            "command_types": ["restore_run", "restore_rollback", "migration_run"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "mapping_mode": preset.definition.get("mapping_mode").and_then(serde_json::Value::as_str).unwrap_or("explicit_paths"),
            "supports_agent_local_archive": preset.definition.get("supports_agent_local_archive").and_then(serde_json::Value::as_bool).unwrap_or(false),
            "supports_post_restore_hooks": preset.definition.get("supports_post_restore_hooks").and_then(serde_json::Value::as_bool).unwrap_or(false),
        }),
    )
}

fn update_restart_policy_status(
    preset: &DataSourcePresetView,
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
            "restart_method": preset.definition.get("restart_method").and_then(serde_json::Value::as_str).unwrap_or("agent_configured"),
            "fallback": preset.definition.get("fallback").and_then(serde_json::Value::as_str).unwrap_or("manual_supervisor"),
        }),
    )
}

fn update_rollback_heartbeat_status(
    preset: &DataSourcePresetView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    (
        "ready_on_demand".to_string(),
        format!(
            "rollback heartbeat source {source_kind} is selected; rollout workers classify heartbeat and activation failures"
        ),
        json!({
            "continuous_status": false,
            "workflow": "agent_update_rollout",
            "command_types": ["agent_update", "agent_update_activate", "agent_update_rollback"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "health_gate": preset.definition.get("health_gate").and_then(serde_json::Value::as_str).unwrap_or("heartbeat_verified"),
            "heartbeat_source": preset.definition.get("source").and_then(serde_json::Value::as_str).unwrap_or("agent_update_heartbeat"),
        }),
    )
}

fn traffic_limit_status_source_status(
    preset: &DataSourcePresetView,
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
            "command_types": ["network_apply", "network_status", "tunnel_speed_test"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "status_source": preset.definition.get("status_source").and_then(serde_json::Value::as_str).unwrap_or("network_status_and_telemetry"),
        }),
    )
}

fn routing_daemon_adapter_status(
    preset: &DataSourcePresetView,
    source_kind: &str,
) -> (String, String, serde_json::Value) {
    (
        "ready_on_demand".to_string(),
        format!(
            "routing-daemon adapter {source_kind} is selected; topology evidence and OSPF jobs provide status"
        ),
        json!({
            "continuous_status": false,
            "workflow": "network_routing",
            "command_types": ["network_status", "network_ospf_cost_update"],
            "privilege_gated": true,
            "source_kind": source_kind,
            "provider": preset.definition.get("provider").and_then(serde_json::Value::as_str).unwrap_or("bird2"),
            "status_source": preset.definition.get("status_source").and_then(serde_json::Value::as_str).unwrap_or("bird2_status"),
        }),
    )
}

fn traffic_status(
    source_kind: &str,
    tunnels: &[TelemetryTunnelView],
) -> (String, String, serde_json::Value) {
    let samples = tunnels
        .iter()
        .filter_map(|tunnel| {
            tunnel.traffic_status.as_ref().map(|status| {
                json!({
                    "interface": tunnel.interface,
                    "traffic_source": tunnel.traffic_source,
                    "traffic_status": status,
                    "traffic_reason": tunnel.traffic_reason,
                    "traffic_checked_unix": tunnel.traffic_checked_unix,
                })
            })
        })
        .collect::<Vec<_>>();
    if samples.is_empty() {
        return (
            "selected_no_samples".to_string(),
            format!("{source_kind} is selected, but no runtime traffic samples are available yet"),
            json!({
                "continuous_status": true,
                "sample_count": 0,
            }),
        );
    }
    let unhealthy = samples.iter().any(|sample| {
        sample
            .get("traffic_status")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|status| status != "ok")
    });
    if unhealthy {
        (
            "degraded".to_string(),
            "one or more runtime traffic sources reported degraded status".to_string(),
            json!({
                "continuous_status": true,
                "sample_count": samples.len(),
                "samples": samples,
            }),
        )
    } else {
        (
            "ok".to_string(),
            "runtime traffic source is reporting healthy samples".to_string(),
            json!({
                "continuous_status": true,
                "sample_count": samples.len(),
                "samples": samples,
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
    let promotion_required = tunnels
        .iter()
        .filter(|tunnel| tunnel.promotion_required)
        .count();
    let degraded = tunnels.iter().filter(|tunnel| {
        tunnel
            .adapter_health
            .as_ref()
            .is_some_and(|health| !health.success)
            || tunnel.plan_correlation == "stale_saved_plan"
    });
    let degraded_count = degraded.count();
    let samples = tunnels
        .iter()
        .map(|tunnel| {
            json!({
                "interface": tunnel.interface,
                "plan_correlation": tunnel.plan_correlation,
                "promotion_required": tunnel.promotion_required,
                "plan_id": tunnel.plan_id,
                "plan_name": tunnel.plan_name,
                "adapter_status": tunnel.adapter_health.as_ref().map(|health| health.status.as_str()),
            })
        })
        .collect::<Vec<_>>();
    if degraded_count > 0 {
        (
            "degraded".to_string(),
            "runtime tunnel telemetry reports adapter or saved-plan drift".to_string(),
            json!({
                "continuous_status": true,
                "sample_count": samples.len(),
                "promotion_required": promotion_required,
                "degraded_count": degraded_count,
                "samples": samples,
            }),
        )
    } else if promotion_required > 0 {
        (
            "needs_promotion".to_string(),
            "observed tunnel candidates need explicit preset-backed promotion".to_string(),
            json!({
                "continuous_status": true,
                "sample_count": samples.len(),
                "promotion_required": promotion_required,
                "degraded_count": degraded_count,
                "samples": samples,
            }),
        )
    } else {
        (
            "ok".to_string(),
            "runtime tunnel telemetry matches selected adapter policy".to_string(),
            json!({
                "continuous_status": true,
                "sample_count": samples.len(),
                "promotion_required": promotion_required,
                "degraded_count": degraded_count,
                "samples": samples,
            }),
        )
    }
}

fn source_kind(preset: &DataSourcePresetView) -> String {
    for key in ["source", "provider", "manager"] {
        if let Some(value) = preset
            .definition
            .get(key)
            .and_then(serde_json::Value::as_str)
        {
            return value.to_string();
        }
    }
    if preset.definition.get("shell_script_argv").is_some() {
        return "shell_script_argv".to_string();
    }
    if let Some(value) = preset
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
        "routing_daemon_adapter" => "Routing daemon adapter",
        "backup_object_store" => "Backup object store",
        "restore_path_mapping" => "Restore path mapping",
        "update_artifact_source" => "Update artifact source",
        "update_restart_policy" => "Update restart policy",
        "update_rollback_heartbeat_source" => "Update heartbeat source",
        _ => "Custom data-source domain",
    }
}

fn domain_order(domain: &str) -> usize {
    DATA_SOURCE_DOMAINS
        .iter()
        .position(|candidate| *candidate == domain)
        .unwrap_or(DATA_SOURCE_DOMAINS.len())
}
