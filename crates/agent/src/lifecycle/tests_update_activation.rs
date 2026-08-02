use std::{fs, os::unix::fs::PermissionsExt};

use super::{
    activate_staged_update, read_activation_marker, rollback_update, sha256_hex,
    AgentUpdateActivateInput, AgentUpdateRollbackInput,
};

#[test]
fn activates_staged_update_and_preserves_rollback() {
    let dir = std::env::temp_dir().join(format!("vpsman-update-activate-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let current = dir.join("vpsman-agent");
    let staged = dir.join("vpsman-agent.next");
    let rollback = dir.join("vpsman-agent.rollback");
    fs::write(&current, b"old-agent").unwrap();
    fs::write(&staged, b"new-agent").unwrap();
    fs::write(&rollback, b"old-agent").unwrap();
    let output = activate_staged_update(
        &current,
        AgentUpdateActivateInput {
            job_id: uuid::Uuid::new_v4(),
            staged_sha256_hex: sha256_hex(b"new-agent"),
            restart_agent: false,
            max_timeout_secs: 5,
            cancel_token: crate::command_worker::CommandCancelToken::default(),
        },
    )
    .unwrap();

    assert_eq!(fs::read(&current).unwrap(), b"new-agent");
    assert_eq!(fs::read(&rollback).unwrap(), b"old-agent");
    assert!(!staged.exists());
    assert_eq!(
        read_activation_marker(&dir.join("vpsman-agent.activated.json"))
            .unwrap()
            .unwrap()
            .sha256_hex,
        sha256_hex(b"new-agent")
    );
    assert_eq!(
        fs::metadata(&current).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let status: serde_json::Value = serde_json::from_slice(&output.data).unwrap();
    assert_eq!(status["status"], "activated_pending_restart");

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn rollback_restores_saved_agent_binary() {
    let dir = std::env::temp_dir().join(format!("vpsman-update-rollback-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let current = dir.join("vpsman-agent");
    let rollback = dir.join("vpsman-agent.rollback");
    fs::write(&current, b"bad-agent").unwrap();
    fs::write(&rollback, b"old-agent").unwrap();
    let output = rollback_update(
        &current,
        AgentUpdateRollbackInput {
            job_id: uuid::Uuid::new_v4(),
            rollback_sha256_hex: Some(sha256_hex(b"old-agent")),
            max_timeout_secs: 5,
            cancel_token: crate::command_worker::CommandCancelToken::default(),
        },
    )
    .unwrap();

    assert_eq!(fs::read(&current).unwrap(), b"old-agent");
    assert!(!dir.join("vpsman-agent.activated.json").exists());
    let status: serde_json::Value = serde_json::from_slice(&output.data).unwrap();
    assert_eq!(status["status"], "rolled_back_pending_restart");
    assert_eq!(status["rollback_sha256_hex"], sha256_hex(b"old-agent"));

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn activation_rejects_hash_mismatch_without_replacing_active() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-update-activate-reject-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    let current = dir.join("vpsman-agent");
    let staged = dir.join("vpsman-agent.next");
    fs::write(&current, b"old-agent").unwrap();
    fs::write(&staged, b"new-agent").unwrap();

    assert!(activate_staged_update(
        &current,
        AgentUpdateActivateInput {
            job_id: uuid::Uuid::new_v4(),
            staged_sha256_hex: "00".repeat(32),
            restart_agent: false,
            max_timeout_secs: 5,
            cancel_token: crate::command_worker::CommandCancelToken::default(),
        },
    )
    .is_err());
    assert_eq!(fs::read(&current).unwrap(), b"old-agent");

    let _ = fs::remove_dir_all(dir);
}
