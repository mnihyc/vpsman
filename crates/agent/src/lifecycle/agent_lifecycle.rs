use std::{
    env, fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::Path,
    process::Command,
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use vpsman_common::{CommandOutput, OutputStream};

const RESTART_MODE_ENV: &str = "VPSMAN_AGENT_RESTART_MODE";
const STOP_MODE_ENV: &str = "VPSMAN_AGENT_STOP_MODE";
const SUPERVISED_RESTART_MODE: &str = "signal_only";
const SYSTEMD_STOP_MODE: &str = "restart_prevent_exit_64";
const SYSTEMD_STOP_EXIT_CODE: i32 = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentStopPlan {
    Direct,
    SystemdRestartPrevent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentRestartPlan {
    SupervisorSignal,
    SameProcessExec,
}

impl AgentRestartPlan {
    fn supervisor(self) -> &'static str {
        match self {
            Self::SupervisorSignal => "supervised",
            Self::SameProcessExec => "same_process_exec",
        }
    }
}

impl AgentStopPlan {
    fn exit_code(self) -> i32 {
        match self {
            Self::Direct => 0,
            Self::SystemdRestartPrevent => SYSTEMD_STOP_EXIT_CODE,
        }
    }

    fn supervisor(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::SystemdRestartPrevent => "systemd",
        }
    }
}

pub(crate) fn execute_agent_stop(job_id: uuid::Uuid) -> Result<Vec<CommandOutput>> {
    let restart_mode = env::var(RESTART_MODE_ENV).unwrap_or_default();
    let stop_mode = env::var(STOP_MODE_ENV).unwrap_or_default();
    let plan = agent_stop_plan(&restart_mode, &stop_mode)?;
    let output = lifecycle_output(
        job_id,
        "agent_stop",
        "stop_requested",
        plan.supervisor(),
        true,
    )?;
    request_agent_stop(plan)?;
    Ok(vec![output])
}

pub(crate) fn execute_agent_restart(job_id: uuid::Uuid) -> Result<Vec<CommandOutput>> {
    let current_exe = crate::agent_binary_path::current_agent_binary_path()?;
    let restart_mode = env::var(RESTART_MODE_ENV).unwrap_or_default();
    let plan = agent_restart_plan(&restart_mode);
    let output = lifecycle_output(
        job_id,
        "agent_restart",
        "restart_requested",
        plan.supervisor(),
        false,
    )?;
    request_agent_restart_with_plan(&current_exe, plan)?;
    Ok(vec![output])
}

pub(crate) fn request_agent_restart(current_exe: &Path) -> Result<()> {
    let restart_mode = env::var(RESTART_MODE_ENV).unwrap_or_default();
    request_agent_restart_with_plan(current_exe, agent_restart_plan(&restart_mode))
}

fn agent_restart_plan(restart_mode: &str) -> AgentRestartPlan {
    if restart_mode.trim() == SUPERVISED_RESTART_MODE {
        AgentRestartPlan::SupervisorSignal
    } else {
        AgentRestartPlan::SameProcessExec
    }
}

fn request_agent_restart_with_plan(current_exe: &Path, plan: AgentRestartPlan) -> Result<()> {
    if plan == AgentRestartPlan::SameProcessExec {
        validate_restart_executable(current_exe)?;
    }
    let pid = std::process::id();
    let current_exe = current_exe.to_path_buf();
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    thread::Builder::new()
        .name("vpsman-agent-restart-request".to_string())
        .spawn(move || {
            thread::sleep(Duration::from_secs(1));
            if plan == AgentRestartPlan::SameProcessExec {
                let error = Command::new(&current_exe)
                    .args(args)
                    .env("VPSMAN_AGENT_RESTARTED_FROM", pid.to_string())
                    .exec();
                tracing::error!(
                    %error,
                    path = %current_exe.display(),
                    "same-process agent restart failed; current process remains active"
                );
                return;
            }
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        })
        .context("failed to request supervised agent restart")?;
    Ok(())
}

fn validate_restart_executable(current_exe: &Path) -> Result<()> {
    let metadata = fs::metadata(current_exe).with_context(|| {
        format!(
            "agent_restart_executable_unavailable: failed to inspect {}",
            current_exe.display()
        )
    })?;
    anyhow::ensure!(
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
        "agent_restart_executable_unavailable: {} is not an executable file",
        current_exe.display()
    );
    Ok(())
}

fn agent_stop_plan(restart_mode: &str, stop_mode: &str) -> Result<AgentStopPlan> {
    if restart_mode.trim() != SUPERVISED_RESTART_MODE {
        return Ok(AgentStopPlan::Direct);
    }
    anyhow::ensure!(
        stop_mode.trim() == SYSTEMD_STOP_MODE,
        "agent_stop_supervisor_contract_unavailable: bundled systemd unit must declare {STOP_MODE_ENV}={SYSTEMD_STOP_MODE}"
    );
    Ok(AgentStopPlan::SystemdRestartPrevent)
}

fn request_agent_stop(plan: AgentStopPlan) -> Result<()> {
    thread::Builder::new()
        .name("vpsman-agent-stop-request".to_string())
        .spawn(move || {
            thread::sleep(Duration::from_secs(1));
            std::process::exit(plan.exit_code());
        })
        .context("failed to request agent stop")?;
    Ok(())
}

fn lifecycle_output(
    job_id: uuid::Uuid,
    operation_type: &str,
    status: &str,
    supervisor: &str,
    external_start_required: bool,
) -> Result<CommandOutput> {
    Ok(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": operation_type,
            "status": status,
            "supervisor": supervisor,
            "external_start_required": external_start_required,
        }))?,
        exit_code: Some(0),
        done: true,
    })
}

#[cfg(test)]
#[path = "tests_agent_lifecycle.rs"]
mod tests;
