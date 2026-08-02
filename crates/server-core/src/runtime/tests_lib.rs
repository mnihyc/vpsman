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
        follow_symlinks: false,
        missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
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
