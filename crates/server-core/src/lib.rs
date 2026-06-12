use vpsman_common::{
    AgentCapabilitySnapshot, AgentPrivilegeMode, JobCommand, TunnelEndpointSide,
    MAX_DIRECT_FILE_DOWNLOAD_BYTES,
};

pub const STATUS_OUTPUT_MAX_BYTES: usize = 32 * 1024;
pub const INLINE_OUTPUT_PREVIEW_BYTES: usize = 32 * 1024;
pub const DIRECT_STDOUT_MAX_BYTES: u64 = MAX_DIRECT_FILE_DOWNLOAD_BYTES;

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
    match command {
        JobCommand::Shell { pty: true, .. } => "shell_pty",
        JobCommand::Shell { .. } => "shell_argv",
        JobCommand::ShellScript { .. } => "shell_script",
        JobCommand::TerminalOpen { .. } => "terminal_open",
        JobCommand::TerminalInput { .. } => "terminal_input",
        JobCommand::TerminalPoll { .. } => "terminal_poll",
        JobCommand::TerminalResize { .. } => "terminal_resize",
        JobCommand::TerminalClose { .. } => "terminal_close",
        JobCommand::FilePull { .. } => "file_pull",
        JobCommand::FilePush { .. } => "file_push",
        JobCommand::FilePushChunked { .. } => "file_push_chunked",
        JobCommand::FileTransferStart { .. } => "file_transfer_start",
        JobCommand::FileTransferChunk { .. } => "file_transfer_chunk",
        JobCommand::FileTransferCommit { .. } => "file_transfer_commit",
        JobCommand::FileTransferAbort { .. } => "file_transfer_abort",
        JobCommand::FileTransferDownloadStart { .. } => "file_transfer_download_start",
        JobCommand::FileTransferDownloadChunk { .. } => "file_transfer_download_chunk",
        JobCommand::FileStat { .. } => "file_stat",
        JobCommand::FileListDir { .. } => "file_list_dir",
        JobCommand::FileReadText { .. } => "file_read_text",
        JobCommand::FileWriteText { .. } => "file_write_text",
        JobCommand::FileMkdir { .. } => "file_mkdir",
        JobCommand::FileRename { .. } => "file_rename",
        JobCommand::FileDelete { .. } => "file_delete",
        JobCommand::FileChmod { .. } => "file_chmod",
        JobCommand::FileChown { .. } => "file_chown",
        JobCommand::FileCopy { .. } => "file_copy",
        JobCommand::FileDownload { .. } => "file_download",
        JobCommand::FileArchiveTar { .. } => "file_archive_tar",
        JobCommand::ConfigRead => "config_read",
        JobCommand::HotConfig { .. } => "hot_config",
        JobCommand::DataSourceConfigPatch { .. } => "data_source_config_patch",
        JobCommand::UpdateAgent { .. } => "agent_update",
        JobCommand::AgentUpdateActivate { .. } => "agent_update_activate",
        JobCommand::AgentUpdateRollback { .. } => "agent_update_rollback",
        JobCommand::AgentUpdateCheck { .. } => "agent_update_check",
        JobCommand::UserSessions => "user_sessions",
        JobCommand::ProcessList { .. } => "process_list",
        JobCommand::ProcessStart { .. } => "process_start",
        JobCommand::ProcessStop { .. } => "process_stop",
        JobCommand::ProcessRestart { .. } => "process_restart",
        JobCommand::ProcessStatus { .. } => "process_status",
        JobCommand::ProcessLogs { .. } => "process_logs",
        JobCommand::Backup { .. } => "backup",
        JobCommand::Restore { .. } => "restore",
        JobCommand::RestoreRollback { .. } => "restore_rollback",
        JobCommand::NetworkApply { .. } => "network_apply",
        JobCommand::NetworkOspfCostUpdate { .. } => "network_ospf_cost_update",
        JobCommand::NetworkRollback { .. } => "network_rollback",
        JobCommand::NetworkStatus { .. } => "network_status",
        JobCommand::NetworkInterfaces => "network_interfaces",
        JobCommand::NetworkProbe { .. } => "network_probe",
        JobCommand::NetworkSpeedTest { .. } => "network_speed_test",
    }
}

pub fn scheduled_command_type_label(command: &JobCommand, fallback: &str) -> String {
    match command {
        JobCommand::Shell { .. }
        | JobCommand::ShellScript { .. }
        | JobCommand::Backup { .. }
        | JobCommand::Restore { .. }
        | JobCommand::RestoreRollback { .. }
        | JobCommand::NetworkApply { .. }
        | JobCommand::NetworkOspfCostUpdate { .. }
        | JobCommand::NetworkRollback { .. }
        | JobCommand::NetworkStatus { .. }
        | JobCommand::NetworkInterfaces
        | JobCommand::NetworkProbe { .. }
        | JobCommand::NetworkSpeedTest { .. }
        | JobCommand::UpdateAgent { .. }
        | JobCommand::AgentUpdateActivate { .. }
        | JobCommand::AgentUpdateRollback { .. }
        | JobCommand::AgentUpdateCheck { .. } => job_command_type_label(command).to_string(),
        _ => fallback.to_string(),
    }
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

pub fn target_lacks_privileged_host_mutation_capability(
    capabilities: &AgentCapabilitySnapshot,
) -> bool {
    match capabilities.privilege_mode {
        AgentPrivilegeMode::Unprivileged => true,
        AgentPrivilegeMode::Root => !capabilities.can_attempt_privileged_ops,
        AgentPrivilegeMode::Unknown => false,
    }
}

pub fn target_status_counts_as_accepted(status: &str) -> bool {
    matches!(status, "accepted" | "completed" | "failed" | "timed_out")
}

pub fn target_status_is_pending(status: &str) -> bool {
    matches!(status, "queued" | "dispatching")
}

pub fn aggregate_job_status_from_statuses(
    target_statuses: &[String],
    target_count: usize,
) -> &'static str {
    let completed = target_statuses
        .iter()
        .filter(|status| status.as_str() == "completed")
        .count();
    if target_count > 0 && completed == target_count {
        return "completed";
    }
    if completed > 0 {
        return "partially_completed";
    }
    if target_statuses
        .iter()
        .any(|status| status.as_str() == "degraded_unprivileged")
    {
        return "degraded_unprivileged";
    }
    if target_statuses
        .iter()
        .any(|status| status.as_str() == "timed_out")
    {
        return "timed_out";
    }
    if target_statuses
        .iter()
        .any(|status| matches!(status.as_str(), "failed" | "rejected_by_agent"))
    {
        return "failed";
    }
    if target_statuses
        .iter()
        .any(|status| status.as_str() == "accepted")
    {
        return "accepted";
    }
    "dispatch_failed"
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
            "partially_completed"
        );
        assert_eq!(
            aggregate_job_status_from_statuses(
                &["degraded_unprivileged".to_string(), "failed".to_string()],
                2,
            ),
            "degraded_unprivileged"
        );
    }
}
