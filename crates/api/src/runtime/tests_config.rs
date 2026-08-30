use crate::job_request::validate_job_command;
use vpsman_common::{
    AgentRuntimeConfig, AgentUpdateConfig, JobCommand, MAX_RUNTIME_CONFIG_REASON_BYTES,
};

#[test]
fn autonomous_updater_defaults_disabled_with_official_manifest_defaults() {
    let update = AgentUpdateConfig::default();

    assert!(!update.unmanaged_enabled);
    assert_eq!(
        update.unmanaged_version_url,
        "https://github.com/mnihyc/vpsman/releases/latest/download/version.json"
    );
    assert_eq!(update.unmanaged_interval_secs, 86_400);
    assert_eq!(update.unmanaged_jitter_secs, 86_400);
    assert!(update.unmanaged_activate);
    assert!(update.unmanaged_restart_agent);
}

#[test]
fn runtime_config_sync_reason_uses_four_kibibyte_limit() {
    let max_reason = "x".repeat(MAX_RUNTIME_CONFIG_REASON_BYTES);
    validate_job_command(&JobCommand::RuntimeConfigSync {
        desired_version: 1,
        reason: max_reason,
        config: Box::new(AgentRuntimeConfig {
            version: 1,
            ..AgentRuntimeConfig::default()
        }),
    })
    .unwrap();

    let oversized_reason = "x".repeat(MAX_RUNTIME_CONFIG_REASON_BYTES + 1);
    assert!(validate_job_command(&JobCommand::RuntimeConfigSync {
        desired_version: 1,
        reason: oversized_reason,
        config: Box::new(AgentRuntimeConfig {
            version: 1,
            ..AgentRuntimeConfig::default()
        }),
    })
    .is_err());
}

#[test]
fn validates_agent_update_job_document() {
    let command = JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
    };
    validate_job_command(&command).unwrap();

    validate_job_command(&JobCommand::AgentUpdateActivate {
        staged_sha256_hex: "ef".repeat(32),
        restart_agent: false,
    })
    .unwrap();
    validate_job_command(&JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: Some("01".repeat(32)),
    })
    .unwrap();
    validate_job_command(&JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: None,
    })
    .unwrap();
}

#[test]
fn rejects_invalid_agent_update_job_document() {
    assert!(validate_job_command(&JobCommand::UpdateAgent {
        artifact_url: "http://updates.example/vpsman-agent".to_string(),
        sha256_hex: "ab".repeat(32),
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::UpdateAgent {
        artifact_url: "https://updates.example/vpsman-agent".to_string(),
        sha256_hex: "not-a-hash".to_string(),
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::AgentUpdateActivate {
        staged_sha256_hex: "not-a-hash".to_string(),
        restart_agent: false,
    })
    .is_err());
    assert!(validate_job_command(&JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: Some("not-a-hash".to_string()),
    })
    .is_err());
}
