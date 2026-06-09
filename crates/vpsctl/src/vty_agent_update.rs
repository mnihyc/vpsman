use anyhow::{Context, Result};

use crate::{
    vty_config::{
        parse_vty_agent_update, parse_vty_agent_update_activate, parse_vty_agent_update_rollback,
        submit_vty_agent_update, submit_vty_agent_update_activate,
        submit_vty_agent_update_rollback,
    },
    vty_jobs::VtyPrivilegeContext,
};

pub(crate) fn is_vty_agent_update_command(command: &str) -> bool {
    command.starts_with("agent-update ")
        || command.starts_with("agent-update-activate ")
        || command.starts_with("agent-update-rollback ")
}

pub(crate) fn submit_vty_agent_update_command(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    command: &str,
) -> Result<String> {
    anyhow::ensure!(
        privilege_context.enabled,
        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
    );
    let parts = command.split_whitespace().collect::<Vec<_>>();
    match parts.first().copied().context("agent update command is empty")? {
        "agent-update" => submit_vty_agent_update(
            api_url,
            token,
            &privilege_context.password,
            &privilege_context.salt_hex,
            parse_vty_agent_update(&parts[1..]).with_context(|| {
                "usage: agent-update --artifact-url <https-url> --sha256-hex <sha256> [--artifact-signature-hex <sig>] [--artifact-signing-key-hex <key>] <target ...> [--timeout <1-3600>] [--privilege-ttl <1-3600>] [--force-unprivileged] --confirmed"
            })?,
        ),
        "agent-update-activate" => submit_vty_agent_update_activate(
            api_url,
            token,
            &privilege_context.password,
            &privilege_context.salt_hex,
            parse_vty_agent_update_activate(&parts[1..]).with_context(|| {
                "usage: agent-update-activate --staged-sha256-hex <sha256> <target ...> [--restart-agent] [--timeout <1-3600>] [--privilege-ttl <1-3600>] [--force-unprivileged] --confirmed"
            })?,
        ),
        "agent-update-rollback" => submit_vty_agent_update_rollback(
            api_url,
            token,
            &privilege_context.password,
            &privilege_context.salt_hex,
            parse_vty_agent_update_rollback(&parts[1..]).with_context(|| {
                "usage: agent-update-rollback [--rollback-sha256-hex <sha256>] <target ...> [--timeout <1-3600>] [--privilege-ttl <1-3600>] [--force-unprivileged] --confirmed"
            })?,
        ),
        command => anyhow::bail!("unknown agent update command {command}"),
    }
}
