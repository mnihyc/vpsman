use vpsman_common::{
    AgentCapabilitySnapshot, AgentPrivilegeMode, JobCommand, JobTargetStatus, TunnelEndpointSide,
    MAX_DIRECT_FILE_DOWNLOAD_BYTES,
};

pub use vpsman_common::{
    job_statuses, job_target_statuses, job_target_terminal_statuses, job_terminal_statuses,
    JobStatus, JobStatusClass, JobTargetStatusClass, JOB_STATUSES, JOB_STATUS_AGENT_TIMEOUT,
    JOB_STATUS_CANCELED, JOB_STATUS_COMPLETED, JOB_STATUS_CONTROL_TIMEOUT, JOB_STATUS_FAILED,
    JOB_STATUS_PARTIAL_SUCCESS, JOB_STATUS_QUEUED, JOB_STATUS_REJECTED, JOB_STATUS_RUNNING,
    JOB_STATUS_SKIPPED, JOB_TARGET_STATUSES, JOB_TARGET_TERMINAL_STATUSES, JOB_TERMINAL_STATUSES,
    TARGET_STATUS_AGENT_LOST, TARGET_STATUS_AGENT_TIMEOUT, TARGET_STATUS_CANCELED,
    TARGET_STATUS_COMPLETED, TARGET_STATUS_CONTROL_TIMEOUT, TARGET_STATUS_DISPATCHING,
    TARGET_STATUS_FAILED, TARGET_STATUS_QUEUED, TARGET_STATUS_REJECTED, TARGET_STATUS_RUNNING,
    TARGET_STATUS_SKIPPED,
};

pub const STATUS_OUTPUT_MAX_BYTES: usize = 32 * 1024;
pub const INLINE_OUTPUT_PREVIEW_BYTES: usize = 32 * 1024;
pub const DIRECT_STDOUT_MAX_BYTES: u64 = MAX_DIRECT_FILE_DOWNLOAD_BYTES;
pub const SCOPE_FLEET_READ: &str = "fleet:read";
pub const SCOPE_JOBS_READ: &str = "jobs:read";
pub const SCOPE_BACKUPS_READ: &str = "backups:read";
pub const SCOPE_TERMINAL_READ: &str = "terminal:read";
pub const SCOPE_INTEGRATIONS_READ: &str = "integrations:read";
pub const SCOPE_INTEGRATIONS_WRITE: &str = "integrations:write";
pub const SCOPE_TEMPLATES_READ: &str = "templates:read";
pub const SCOPE_TEMPLATES_WRITE: &str = "templates:write";
pub const SCOPE_SCHEDULES_READ: &str = "schedules:read";
pub const SCOPE_CONFIG_READ: &str = "config:read";
pub const SCOPE_NETWORK_READ: &str = "network:read";
pub const SCOPE_AUDIT_READ: &str = "audit:read";
pub const SCOPE_HISTORY_WRITE: &str = "history:write";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkTargetValidationError {
    SingleEndpointTargetMismatch,
    SpeedTestTargetMismatch,
}

impl NetworkTargetValidationError {
    pub fn code(self) -> &'static str {
        match self {
            Self::SingleEndpointTargetMismatch => "network_apply_target_mismatch",
            Self::SpeedTestTargetMismatch => "network_speed_test_target_mismatch",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityFailure {
    pub reason: &'static str,
    pub hint: &'static str,
    pub message: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilitySkip {
    pub client_id: String,
    pub failure: CapabilityFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetCapability {
    pub client_id: String,
    pub capabilities: AgentCapabilitySnapshot,
}

pub fn job_command_type_label(command: &JobCommand) -> &'static str {
    vpsman_common::job_command_type_label(command)
}

pub fn scheduled_command_type_label(command: &JobCommand, fallback: &str) -> String {
    vpsman_common::scheduled_command_type_label(command, fallback)
}

pub fn default_operator_scopes(role: &str) -> Vec<String> {
    match role.trim() {
        "admin" => vec!["*".to_string()],
        "operator" => vec![
            SCOPE_FLEET_READ.to_string(),
            SCOPE_JOBS_READ.to_string(),
            SCOPE_BACKUPS_READ.to_string(),
            SCOPE_TERMINAL_READ.to_string(),
            SCOPE_INTEGRATIONS_READ.to_string(),
            SCOPE_TEMPLATES_READ.to_string(),
            SCOPE_SCHEDULES_READ.to_string(),
            SCOPE_CONFIG_READ.to_string(),
            SCOPE_NETWORK_READ.to_string(),
            SCOPE_AUDIT_READ.to_string(),
            "jobs:write".to_string(),
            "inventory:write".to_string(),
            "schedules:write".to_string(),
            "backups:write".to_string(),
            "network:write".to_string(),
            "config:write".to_string(),
            SCOPE_INTEGRATIONS_WRITE.to_string(),
            SCOPE_TEMPLATES_WRITE.to_string(),
            SCOPE_HISTORY_WRITE.to_string(),
        ],
        "viewer" => vec![SCOPE_FLEET_READ.to_string()],
        _ => Vec::new(),
    }
}

pub fn operator_has_scope(scopes: &[String], required: &str) -> bool {
    scopes.iter().any(|scope| scope == "*" || scope == required)
}

pub fn role_allows(actual: &str, required: &str) -> bool {
    match (operator_role_rank(actual), operator_role_rank(required)) {
        (Some(actual), Some(required)) => actual >= required,
        _ => false,
    }
}

pub fn operator_role_rank(role: &str) -> Option<u8> {
    match role.trim() {
        "viewer" => Some(0),
        "operator" => Some(1),
        "admin" => Some(2),
        _ => None,
    }
}

pub fn operator_is_active_authorized(
    status: &str,
    role: &str,
    scopes: &[String],
    required_role: &str,
    required_scopes: &[&str],
) -> bool {
    if status.trim() != "active" || !role_allows(role, required_role) {
        return false;
    }
    let defaulted_scopes;
    let effective_scopes = if scopes.is_empty() {
        defaulted_scopes = default_operator_scopes(role);
        &defaulted_scopes
    } else {
        scopes
    };
    required_scopes
        .iter()
        .all(|scope| operator_has_scope(effective_scopes, scope))
}

pub fn validate_network_apply_target(
    command: &JobCommand,
    resolved_targets: &[String],
) -> Result<(), NetworkTargetValidationError> {
    let expected = match command {
        JobCommand::NetworkApply { plan, side, .. }
        | JobCommand::NetworkOspfCostUpdate { plan, side, .. }
        | JobCommand::NetworkRollback { plan, side }
        | JobCommand::NetworkStatus { plan, side }
        | JobCommand::NetworkProbe { plan, side, .. } => match side {
            TunnelEndpointSide::Left => &plan.left_client_id,
            TunnelEndpointSide::Right => &plan.right_client_id,
        },
        JobCommand::NetworkSpeedTest { plan, .. } => {
            let mut expected = vec![plan.left_client_id.clone(), plan.right_client_id.clone()];
            expected.sort();
            let mut actual = resolved_targets.to_vec();
            actual.sort();
            return if actual == expected {
                Ok(())
            } else {
                Err(NetworkTargetValidationError::SpeedTestTargetMismatch)
            };
        }
        _ => return Ok(()),
    };
    if resolved_targets.len() == 1 && resolved_targets.first() == Some(expected) {
        Ok(())
    } else {
        Err(NetworkTargetValidationError::SingleEndpointTargetMismatch)
    }
}

pub fn split_targets_by_capability(
    command: &JobCommand,
    targets: &[String],
    agents: &[TargetCapability],
    force_unprivileged: bool,
) -> (Vec<String>, Vec<CapabilitySkip>) {
    if force_unprivileged {
        return (targets.to_vec(), Vec::new());
    }
    let mut dispatch_targets = Vec::new();
    let mut skipped_targets = Vec::new();
    for client_id in targets {
        if let Some(failure) = agents
            .iter()
            .find(|agent| agent.client_id == *client_id)
            .and_then(|agent| target_capability_failure(command, &agent.capabilities))
        {
            skipped_targets.push(CapabilitySkip {
                client_id: client_id.clone(),
                failure,
            });
        } else {
            dispatch_targets.push(client_id.clone());
        }
    }
    (dispatch_targets, skipped_targets)
}

pub fn target_capability_failure(
    command: &JobCommand,
    capabilities: &AgentCapabilitySnapshot,
) -> Option<CapabilityFailure> {
    if target_lacks_root_network_capability(command, capabilities) {
        return Some(CapabilityFailure {
            reason: "target_agent_lacks_root_runtime_network_capability",
            hint: "agent reported unprivileged mode or no root runtime network capability; root-only network mutation was not dispatched unless force_unprivileged is set",
            message: "target agent lacks root runtime network capability",
        });
    }
    if target_lacks_process_limit_capability(command, capabilities) {
        return Some(CapabilityFailure {
            reason: "target_agent_lacks_process_limit_capability",
            hint: "agent reported unprivileged mode or no process-limit capability; process start with resource limits was not dispatched unless force_unprivileged is set",
            message: "target agent lacks process limit capability",
        });
    }
    if target_lacks_agent_update_capability(command, capabilities) {
        return Some(CapabilityFailure {
            reason: "target_agent_lacks_agent_update_capability",
            hint: "agent reported unprivileged mode or no agent-update host-mutation capability; agent update was not dispatched unless force_unprivileged is set",
            message: "target agent lacks agent update capability",
        });
    }
    if target_lacks_restore_capability(command, capabilities) {
        return Some(CapabilityFailure {
            reason: "target_agent_lacks_restore_capability",
            hint: "agent reported unprivileged mode or no privileged host-mutation capability; restore mutation was not dispatched unless force_unprivileged is set",
            message: "target agent lacks restore capability",
        });
    }
    None
}

pub fn target_lacks_root_network_capability(
    command: &JobCommand,
    capabilities: &AgentCapabilitySnapshot,
) -> bool {
    let root_network_operation = matches!(
        command,
        JobCommand::NetworkApply { .. }
            | JobCommand::NetworkRollback { .. }
            | JobCommand::NetworkOspfCostUpdate { .. }
    );
    if !root_network_operation {
        return false;
    }
    match capabilities.privilege_mode {
        AgentPrivilegeMode::Unprivileged => true,
        AgentPrivilegeMode::Root => !capabilities.can_manage_runtime_tunnels,
        AgentPrivilegeMode::Unknown => false,
    }
}

pub fn target_lacks_process_limit_capability(
    command: &JobCommand,
    capabilities: &AgentCapabilitySnapshot,
) -> bool {
    let JobCommand::ProcessStart { limits, .. } = command else {
        return false;
    };
    if limits.is_default() {
        return false;
    }
    match capabilities.privilege_mode {
        AgentPrivilegeMode::Unprivileged => true,
        AgentPrivilegeMode::Root => !capabilities.can_apply_process_limits,
        AgentPrivilegeMode::Unknown => false,
    }
}

pub fn target_lacks_agent_update_capability(
    command: &JobCommand,
    capabilities: &AgentCapabilitySnapshot,
) -> bool {
    let agent_update_operation = matches!(
        command,
        JobCommand::HotConfig { .. }
            | JobCommand::DataSourceConfigPatch { .. }
            | JobCommand::UpdateAgent { .. }
            | JobCommand::AgentUpdateActivate { .. }
            | JobCommand::AgentUpdateRollback { .. }
            | JobCommand::AgentUpdateCheck { .. }
    );
    if !agent_update_operation {
        return false;
    }
    target_lacks_privileged_host_mutation_capability(capabilities)
}

pub fn target_lacks_restore_capability(
    command: &JobCommand,
    capabilities: &AgentCapabilitySnapshot,
) -> bool {
    let restore_operation = matches!(
        command,
        JobCommand::Restore { .. } | JobCommand::RestoreRollback { .. }
    );
    if !restore_operation {
        return false;
    }
    target_lacks_privileged_host_mutation_capability(capabilities)
}

#[cfg(test)]
mod auth_tests {
    use super::{default_operator_scopes, operator_is_active_authorized};

    #[test]
    fn active_operator_authority_requires_status_role_and_all_scopes() {
        let scopes = default_operator_scopes("operator");
        assert!(operator_is_active_authorized(
            "active",
            "operator",
            &scopes,
            "operator",
            &["jobs:write", "schedules:write"],
        ));
        assert!(!operator_is_active_authorized(
            "disabled",
            "operator",
            &scopes,
            "operator",
            &["jobs:write"],
        ));
        assert!(!operator_is_active_authorized(
            "active",
            "viewer",
            &default_operator_scopes("viewer"),
            "operator",
            &["jobs:write"],
        ));
        assert!(!operator_is_active_authorized(
            "active",
            "operator",
            &["jobs:write".to_string()],
            "operator",
            &["jobs:write", "schedules:write"],
        ));
    }
}

pub fn target_lacks_privileged_host_mutation_capability(
    capabilities: &AgentCapabilitySnapshot,
) -> bool {
    match capabilities.privilege_mode {
        AgentPrivilegeMode::Unprivileged => true,
        AgentPrivilegeMode::Root => !capabilities.can_attempt_privileged_ops,
        AgentPrivilegeMode::Unknown => false,
    }
}

pub fn target_status_is_active(status: &str) -> bool {
    JobTargetStatus::parse(status)
        .map(|status| status.class().is_in_progress())
        .unwrap_or(false)
}

pub fn aggregate_job_status_from_statuses(
    target_statuses: &[String],
    target_count: usize,
) -> &'static str {
    if target_count == 0 {
        return JOB_STATUS_SKIPPED;
    }
    if target_statuses
        .iter()
        .any(|status| target_status_is_active(status))
    {
        return JOB_STATUS_RUNNING;
    }

    let parsed_statuses = target_statuses
        .iter()
        .filter_map(|status| JobTargetStatus::parse(status))
        .collect::<Vec<_>>();
    let completed = parsed_statuses
        .iter()
        .filter(|status| status.class() == JobTargetStatusClass::Successful)
        .count();
    let skipped = parsed_statuses
        .iter()
        .filter(|status| status.class() == JobTargetStatusClass::Skipped)
        .count();
    if completed == target_count {
        return JOB_STATUS_COMPLETED;
    }
    if skipped == target_count {
        return JOB_STATUS_SKIPPED;
    }
    if completed > 0 {
        return JOB_STATUS_PARTIAL_SUCCESS;
    }
    if parsed_statuses.contains(&JobTargetStatus::ControlTimeout) {
        return JOB_STATUS_CONTROL_TIMEOUT;
    }
    if parsed_statuses.contains(&JobTargetStatus::AgentTimeout) {
        return JOB_STATUS_AGENT_TIMEOUT;
    }
    if parsed_statuses.contains(&JobTargetStatus::Failed) {
        return JOB_STATUS_FAILED;
    }
    if parsed_statuses.contains(&JobTargetStatus::Canceled) {
        return JOB_STATUS_CANCELED;
    }
    if parsed_statuses.contains(&JobTargetStatus::Rejected) {
        return JOB_STATUS_REJECTED;
    }
    JOB_STATUS_FAILED
}

pub fn is_backup_operation(command: &JobCommand) -> bool {
    matches!(command, JobCommand::Backup { .. })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use vpsman_common::{
        AgentCapabilitySnapshot, AgentPrivilegeMode, JobCommand, ProcessResourceLimits,
        ProcessRunPolicy,
    };

    use super::*;

    #[test]
    fn config_read_is_not_host_mutation_gated() {
        let capabilities = AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            ..AgentCapabilitySnapshot::default()
        };

        assert_eq!(
            target_capability_failure(&JobCommand::ConfigRead, &capabilities),
            None
        );
    }

    #[test]
    fn hot_config_remains_host_mutation_gated() {
        let capabilities = AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Unprivileged,
            ..AgentCapabilitySnapshot::default()
        };

        assert_eq!(
            target_capability_failure(
                &JobCommand::HotConfig {
                    apply_mode: vpsman_common::HOT_CONFIG_APPLY_MODE_FULL_OVERRIDE.to_string(),
                    toml: String::new(),
                    preserve_redacted: None,
                    base_config_sha256_hex: None,
                },
                &capabilities,
            )
            .map(|failure| failure.reason),
            Some("target_agent_lacks_agent_update_capability")
        );
    }

    #[test]
    fn process_limits_require_capability() {
        let capabilities = AgentCapabilitySnapshot {
            privilege_mode: AgentPrivilegeMode::Root,
            can_attempt_privileged_ops: true,
            can_apply_process_limits: false,
            ..AgentCapabilitySnapshot::default()
        };
        let command = JobCommand::ProcessStart {
            name: "svc".to_string(),
            argv: vec!["/bin/true".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            policy: ProcessRunPolicy::default(),
            limits: ProcessResourceLimits {
                memory_max_bytes: Some(1024),
                ..ProcessResourceLimits::default()
            },
        };

        assert_eq!(
            target_capability_failure(&command, &capabilities).map(|failure| failure.reason),
            Some("target_agent_lacks_process_limit_capability")
        );
    }

    #[test]
    fn shared_labels_cover_file_and_schedule_cases() {
        let command = JobCommand::FileDownload {
            path: "/tmp/a".to_string(),
            max_bytes: 1,
            follow_symlinks: false,
        };
        assert_eq!(job_command_type_label(&command), "file_download");
        assert_eq!(
            scheduled_command_type_label(&command, "file_download"),
            "file_download"
        );

        let command = JobCommand::Backup {
            paths: vec!["/etc".to_string()],
            include_config: true,
            recipient_public_key_hex: None,
        };
        assert_eq!(scheduled_command_type_label(&command, "unknown"), "backup");
    }

    #[test]
    fn aggregate_status_preserves_existing_ordering() {
        assert_eq!(
            aggregate_job_status_from_statuses(&["completed".to_string()], 1),
            "completed"
        );
        assert_eq!(
            aggregate_job_status_from_statuses(&["completed".to_string(), "failed".to_string()], 2,),
            "partial_success"
        );
        assert_eq!(
            aggregate_job_status_from_statuses(&["skipped".to_string(), "skipped".to_string()], 2,),
            "skipped"
        );
    }
}
