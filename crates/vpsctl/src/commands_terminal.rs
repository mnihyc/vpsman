use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use clap::Args;
use serde_json::Value;
use uuid::Uuid;
use vpsman_common::{
    default_terminal_flow_window_bytes, default_terminal_idle_timeout_secs, JobCommand,
};

use crate::jobs::{submit_privileged_operation, PrivilegedOperationRequest};

#[derive(Debug, Args)]
pub(crate) struct TerminalOpenCommand {
    #[arg(long)]
    pub(crate) session_id: Option<Uuid>,
    #[arg(long, value_delimiter = ',', required = true)]
    pub(crate) argv: Vec<String>,
    #[arg(long)]
    pub(crate) cwd: Option<String>,
    #[arg(long, default_value_t = 120)]
    pub(crate) cols: u16,
    #[arg(long, default_value_t = 40)]
    pub(crate) rows: u16,
    #[arg(long)]
    pub(crate) replay_from_seq: Option<u64>,
    #[arg(long, default_value_t = default_terminal_idle_timeout_secs())]
    pub(crate) idle_timeout_secs: u32,
    #[arg(long, default_value_t = default_terminal_flow_window_bytes())]
    pub(crate) flow_window_bytes: u32,
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) pools: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) proof_ttl_secs: u64,
    #[arg(long, default_value_t = 30)]
    pub(crate) timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TerminalInputCommand {
    #[arg(long)]
    pub(crate) session_id: Uuid,
    #[arg(long)]
    pub(crate) input_seq: u64,
    #[arg(long)]
    pub(crate) text: Option<String>,
    #[arg(long)]
    pub(crate) data_base64: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) pools: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) proof_ttl_secs: u64,
    #[arg(long, default_value_t = 30)]
    pub(crate) timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TerminalPollCommand {
    #[arg(long)]
    pub(crate) session_id: Uuid,
    #[arg(long)]
    pub(crate) replay_from_seq: Option<u64>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) pools: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) proof_ttl_secs: u64,
    #[arg(long, default_value_t = 30)]
    pub(crate) timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TerminalResizeCommand {
    #[arg(long)]
    pub(crate) session_id: Uuid,
    #[arg(long)]
    pub(crate) cols: u16,
    #[arg(long)]
    pub(crate) rows: u16,
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) pools: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) proof_ttl_secs: u64,
    #[arg(long, default_value_t = 30)]
    pub(crate) timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TerminalCloseCommand {
    #[arg(long)]
    pub(crate) session_id: Uuid,
    #[arg(long)]
    pub(crate) reason: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) pools: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) proof_ttl_secs: u64,
    #[arg(long, default_value_t = 30)]
    pub(crate) timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

pub(crate) fn terminal_open(
    api_url: &str,
    token: Option<&str>,
    command: TerminalOpenCommand,
) -> Result<()> {
    let session_id = command.session_id.unwrap_or_else(Uuid::new_v4);
    let operation = JobCommand::TerminalOpen {
        session_id,
        argv: command.argv,
        cwd: command.cwd,
        cols: command.cols,
        rows: command.rows,
        replay_from_seq: command.replay_from_seq,
        idle_timeout_secs: command.idle_timeout_secs,
        flow_window_bytes: command.flow_window_bytes,
    };
    let response = submit_terminal_operation(
        api_url,
        token,
        &operation,
        "terminal_open",
        &command.clients,
        &command.pools,
        &command.tags,
        &command.password_env,
        command.super_salt_hex.as_deref(),
        command.proof_ttl_secs,
        command.timeout_secs,
        command.confirmed,
    )?;
    println!("{}", enrich_terminal_open_response(session_id, &response)?);
    Ok(())
}

pub(crate) fn terminal_input(
    api_url: &str,
    token: Option<&str>,
    command: TerminalInputCommand,
) -> Result<()> {
    let data_base64 = terminal_input_data(command.text, command.data_base64)?;
    let operation = JobCommand::TerminalInput {
        session_id: command.session_id,
        input_seq: command.input_seq,
        data_base64,
    };
    println!(
        "{}",
        submit_terminal_operation(
            api_url,
            token,
            &operation,
            "terminal_input",
            &command.clients,
            &command.pools,
            &command.tags,
            &command.password_env,
            command.super_salt_hex.as_deref(),
            command.proof_ttl_secs,
            command.timeout_secs,
            command.confirmed,
        )?
    );
    Ok(())
}

pub(crate) fn terminal_poll(
    api_url: &str,
    token: Option<&str>,
    command: TerminalPollCommand,
) -> Result<()> {
    let operation = JobCommand::TerminalPoll {
        session_id: command.session_id,
        replay_from_seq: command.replay_from_seq,
    };
    println!(
        "{}",
        submit_terminal_operation(
            api_url,
            token,
            &operation,
            "terminal_poll",
            &command.clients,
            &command.pools,
            &command.tags,
            &command.password_env,
            command.super_salt_hex.as_deref(),
            command.proof_ttl_secs,
            command.timeout_secs,
            command.confirmed,
        )?
    );
    Ok(())
}

pub(crate) fn terminal_resize(
    api_url: &str,
    token: Option<&str>,
    command: TerminalResizeCommand,
) -> Result<()> {
    let operation = JobCommand::TerminalResize {
        session_id: command.session_id,
        cols: command.cols,
        rows: command.rows,
    };
    println!(
        "{}",
        submit_terminal_operation(
            api_url,
            token,
            &operation,
            "terminal_resize",
            &command.clients,
            &command.pools,
            &command.tags,
            &command.password_env,
            command.super_salt_hex.as_deref(),
            command.proof_ttl_secs,
            command.timeout_secs,
            command.confirmed,
        )?
    );
    Ok(())
}

pub(crate) fn terminal_close(
    api_url: &str,
    token: Option<&str>,
    command: TerminalCloseCommand,
) -> Result<()> {
    let operation = JobCommand::TerminalClose {
        session_id: command.session_id,
        reason: command.reason,
    };
    println!(
        "{}",
        submit_terminal_operation(
            api_url,
            token,
            &operation,
            "terminal_close",
            &command.clients,
            &command.pools,
            &command.tags,
            &command.password_env,
            command.super_salt_hex.as_deref(),
            command.proof_ttl_secs,
            command.timeout_secs,
            command.confirmed,
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn submit_terminal_operation(
    api_url: &str,
    token: Option<&str>,
    operation: &JobCommand,
    command_label: &'static str,
    clients: &[String],
    pools: &[String],
    tags: &[String],
    password_env: &str,
    super_salt_hex: Option<&str>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
) -> Result<String> {
    submit_privileged_operation(PrivilegedOperationRequest {
        api_url,
        token,
        operation,
        command_label,
        clients,
        pools,
        tags,
        password_env,
        super_salt_hex,
        proof_ttl_secs,
        timeout_secs,
        confirmed,
        force_unprivileged: false,
    })
}

fn terminal_input_data(text: Option<String>, data_base64: Option<String>) -> Result<String> {
    match (text, data_base64) {
        (Some(_), Some(_)) => anyhow::bail!("use either --text or --data-base64, not both"),
        (Some(text), None) => Ok(BASE64.encode(text.as_bytes())),
        (None, Some(data_base64)) => Ok(data_base64),
        (None, None) => anyhow::bail!("terminal-input requires --text or --data-base64"),
    }
}

fn enrich_terminal_open_response(session_id: Uuid, response: &str) -> Result<String> {
    let job: Value =
        serde_json::from_str(response).context("failed to parse terminal-open job response")?;
    Ok(serde_json::json!({
        "terminal_session_id": session_id,
        "job": job,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::terminal_input_data;

    #[test]
    fn terminal_input_text_is_encoded_and_exclusive() {
        assert_eq!(
            terminal_input_data(Some("id\n".to_string()), None).unwrap(),
            "aWQK"
        );
        assert!(terminal_input_data(Some("id".to_string()), Some("aWQ=".to_string())).is_err());
        assert!(terminal_input_data(None, None).is_err());
    }
}
