use std::{
    env, fs, os::unix::fs::PermissionsExt, path::Path, process::Command, thread, time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{task, time};
use tracing::debug;
use vpsman_common::{AgentUpdateHeartbeat, CommandOutput, OutputStream};

use crate::{
    agent_binary_path::{
        activation_marker_path, current_agent_binary_path, rollback_path, staged_path,
    },
    telemetry::unix_now,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ActivationMarker {
    pub(crate) activation_job_id: uuid::Uuid,
    pub(crate) sha256_hex: String,
    pub(crate) marker_unix: u64,
}

#[derive(Clone)]
pub(crate) struct AgentUpdateActivateInput {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) staged_sha256_hex: String,
    pub(crate) restart_agent: bool,
    pub(crate) timeout_secs: u64,
}

#[derive(Clone)]
pub(crate) struct AgentUpdateRollbackInput {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) rollback_sha256_hex: Option<String>,
    pub(crate) timeout_secs: u64,
}

pub(crate) async fn execute_update_activate(
    input: AgentUpdateActivateInput,
) -> Result<Vec<CommandOutput>> {
    let current_exe = current_agent_binary_path()?;
    let output = time::timeout(
        Duration::from_secs(input.timeout_secs.max(1)),
        task::spawn_blocking(move || activate_staged_update(&current_exe, input)),
    )
    .await
    .context("agent update activation timed out")?
    .context("agent update activation task failed")??;
    Ok(vec![output])
}

pub(crate) async fn execute_update_rollback(
    input: AgentUpdateRollbackInput,
) -> Result<Vec<CommandOutput>> {
    let current_exe = current_agent_binary_path()?;
    let output = time::timeout(
        Duration::from_secs(input.timeout_secs.max(1)),
        task::spawn_blocking(move || rollback_update(&current_exe, input)),
    )
    .await
    .context("agent update rollback timed out")?
    .context("agent update rollback task failed")??;
    Ok(vec![output])
}

fn activate_staged_update(
    current_exe: &Path,
    input: AgentUpdateActivateInput,
) -> Result<CommandOutput> {
    let expected_sha256_hex = normalize_sha256(&input.staged_sha256_hex)?;
    let staged_path = staged_path(current_exe)?;
    let rollback_path = rollback_path(current_exe)?;
    let staged = fs::read(&staged_path)
        .with_context(|| format!("failed to read staged update {}", staged_path.display()))?;
    let observed_sha256_hex = sha256_hex(&staged);
    if observed_sha256_hex != expected_sha256_hex {
        anyhow::bail!(
            "staged update hash mismatch: expected {expected_sha256_hex}, got {observed_sha256_hex}"
        );
    }
    if !rollback_path.exists() && current_exe.exists() {
        fs::copy(current_exe, &rollback_path).with_context(|| {
            format!(
                "failed to create activation rollback copy {}",
                rollback_path.display()
            )
        })?;
        fs::set_permissions(&rollback_path, fs::Permissions::from_mode(0o755)).with_context(
            || {
                format!(
                    "failed to set executable mode on {}",
                    rollback_path.display()
                )
            },
        )?;
    }
    replace_active_binary(current_exe, &staged)?;
    write_activation_marker(current_exe, input.job_id, &observed_sha256_hex)?;
    let _ = fs::remove_file(&staged_path);
    let restart = if input.restart_agent {
        request_supervised_restart(current_exe)?;
        "self_restart_requested"
    } else {
        "manual_restart_required"
    };
    let status = serde_json::json!({
        "type": "agent_update_activation",
        "status": "activated_pending_restart",
        "sha256_hex": observed_sha256_hex,
        "active_path": current_exe.display().to_string(),
        "staged_path": staged_path.display().to_string(),
        "rollback_path": rollback_path.display().to_string(),
        "restart": restart,
    });
    status_output(input.job_id, status)
}

fn request_supervised_restart(current_exe: &Path) -> Result<()> {
    let pid = std::process::id();
    let restart_mode = env::var("VPSMAN_AGENT_RESTART_MODE").unwrap_or_default();
    let spawn_replacement = restart_mode.trim() != "signal_only";
    let current_exe = current_exe.to_path_buf();
    thread::Builder::new()
        .name("vpsman-agent-restart-request".to_string())
        .spawn(move || {
            thread::sleep(Duration::from_secs(1));
            if spawn_replacement {
                let mut command = Command::new(&current_exe);
                command.args(env::args_os().skip(1));
                command.env("VPSMAN_AGENT_RESTARTED_FROM", pid.to_string());
                let _ = command.spawn();
                thread::sleep(Duration::from_millis(250));
            }
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        })
        .context("failed to request supervised agent restart")?;
    Ok(())
}

fn rollback_update(current_exe: &Path, input: AgentUpdateRollbackInput) -> Result<CommandOutput> {
    let rollback_path = rollback_path(current_exe)?;
    let rollback = fs::read(&rollback_path)
        .with_context(|| format!("failed to read update rollback {}", rollback_path.display()))?;
    let rollback_sha256_hex = sha256_hex(&rollback);
    if let Some(expected) = input.rollback_sha256_hex.as_deref() {
        let expected = normalize_sha256(expected)?;
        if rollback_sha256_hex != expected {
            anyhow::bail!(
                "update rollback hash mismatch: expected {expected}, got {rollback_sha256_hex}"
            );
        }
    }
    replace_active_binary(current_exe, &rollback)?;
    let _ = fs::remove_file(activation_marker_path(current_exe)?);
    let status = serde_json::json!({
        "type": "agent_update_rollback",
        "status": "rolled_back_pending_restart",
        "rollback_sha256_hex": rollback_sha256_hex,
        "active_path": current_exe.display().to_string(),
        "rollback_path": rollback_path.display().to_string(),
        "restart": "manual_restart_required",
    });
    status_output(input.job_id, status)
}

pub(crate) fn read_activation_heartbeat() -> Result<Option<AgentUpdateHeartbeat>> {
    let current_exe = current_agent_binary_path()?;
    let marker_path = activation_marker_path(&current_exe)?;
    let Some(marker) = read_activation_marker(&marker_path)? else {
        debug!(
            path = %marker_path.display(),
            "no update activation heartbeat marker"
        );
        return Ok(None);
    };
    debug!(
        path = %marker_path.display(),
        activation_job_id = %marker.activation_job_id,
        sha256_hex = %marker.sha256_hex,
        "read update activation heartbeat marker"
    );
    Ok(Some(AgentUpdateHeartbeat {
        activation_job_id: marker.activation_job_id,
        sha256_hex: marker.sha256_hex,
        marker_unix: marker.marker_unix,
        observed_unix: unix_now(),
    }))
}

fn write_activation_marker(
    current_exe: &Path,
    activation_job_id: uuid::Uuid,
    sha256_hex: &str,
) -> Result<()> {
    let marker_path = activation_marker_path(current_exe)?;
    let temp_path = marker_path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let marker = ActivationMarker {
        activation_job_id,
        sha256_hex: sha256_hex.to_string(),
        marker_unix: unix_now(),
    };
    fs::write(&temp_path, serde_json::to_vec(&marker)?)
        .with_context(|| format!("failed to write activation marker {}", temp_path.display()))?;
    fs::rename(&temp_path, &marker_path).with_context(|| {
        let _ = fs::remove_file(&temp_path);
        format!(
            "failed to atomically replace activation marker {}",
            marker_path.display()
        )
    })?;
    Ok(())
}

fn read_activation_marker(marker_path: &Path) -> Result<Option<ActivationMarker>> {
    if !marker_path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(marker_path)
        .with_context(|| format!("failed to read activation marker {}", marker_path.display()))?;
    let marker = serde_json::from_slice::<ActivationMarker>(&bytes).with_context(|| {
        format!(
            "failed to parse activation marker {}",
            marker_path.display()
        )
    })?;
    Ok(Some(marker))
}

fn replace_active_binary(current_exe: &Path, next_bytes: &[u8]) -> Result<()> {
    let temp_path = current_exe.with_extension(format!("activate-tmp-{}", uuid::Uuid::new_v4()));
    fs::write(&temp_path, next_bytes)
        .with_context(|| format!("failed to write activation temp {}", temp_path.display()))?;
    fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed to set executable mode on {}", temp_path.display()))?;
    fs::rename(&temp_path, current_exe).with_context(|| {
        let _ = fs::remove_file(&temp_path);
        format!(
            "failed to atomically replace active agent {}",
            current_exe.display()
        )
    })?;
    Ok(())
}

fn normalize_sha256(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    anyhow::ensure!(
        value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "sha256 must be 64 lowercase or uppercase hex characters"
    );
    Ok(value)
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn status_output(job_id: uuid::Uuid, status: serde_json::Value) -> Result<CommandOutput> {
    Ok(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    })
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::{
        activate_staged_update, read_activation_marker, rollback_update, sha256_hex,
        AgentUpdateActivateInput, AgentUpdateRollbackInput,
    };

    #[test]
    fn activates_staged_update_and_preserves_rollback() {
        let dir =
            std::env::temp_dir().join(format!("vpsman-update-activate-{}", uuid::Uuid::new_v4()));
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
                timeout_secs: 5,
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
        let dir =
            std::env::temp_dir().join(format!("vpsman-update-rollback-{}", uuid::Uuid::new_v4()));
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
                timeout_secs: 5,
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
                timeout_secs: 5,
            },
        )
        .is_err());
        assert_eq!(fs::read(&current).unwrap(), b"old-agent");

        let _ = fs::remove_dir_all(dir);
    }
}
