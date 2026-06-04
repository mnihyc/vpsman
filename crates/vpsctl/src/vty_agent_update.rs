use anyhow::{Context, Result};

use crate::{
    vty_config::{
        parse_vty_agent_update, parse_vty_agent_update_activate, parse_vty_agent_update_rollback,
        parse_vty_super_password_rotate, submit_vty_agent_update, submit_vty_agent_update_activate,
        submit_vty_agent_update_rollback, submit_vty_super_password_rotate,
    },
    vty_jobs::VtyProofContext,
};

pub(crate) fn is_vty_agent_update_command(command: &str) -> bool {
    command.starts_with("agent-update ")
        || command.starts_with("agent-update-activate ")
        || command.starts_with("agent-update-rollback ")
        || command.starts_with("super-password-rotate ")
}

pub(crate) fn submit_vty_agent_update_command(
    api_url: &str,
    token: Option<&str>,
    proof_context: &VtyProofContext,
    command: &str,
) -> Result<String> {
    anyhow::ensure!(
        proof_context.enabled,
        "privileged proof is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
    );
    let parts = command.split_whitespace().collect::<Vec<_>>();
    match parts.first().copied().context("agent update command is empty")? {
        "agent-update" => submit_vty_agent_update(
            api_url,
            token,
            &proof_context.password,
            &proof_context.salt_hex,
            parse_vty_agent_update(&parts[1..]).with_context(|| {
                "usage: agent-update --artifact-url <https-url> --sha256-hex <sha256> [--artifact-signature-hex <sig>] [--artifact-signing-key-hex <key>] [--canary-count <n>] <target ...> [--timeout <1-3600>] [--proof-ttl <1-3600>] [--force-unprivileged] --confirmed"
            })?,
        ),
        "agent-update-activate" => submit_vty_agent_update_activate(
            api_url,
            token,
            &proof_context.password,
            &proof_context.salt_hex,
            parse_vty_agent_update_activate(&parts[1..]).with_context(|| {
                "usage: agent-update-activate --staged-sha256-hex <sha256> <target ...> [--restart-agent] [--timeout <1-3600>] [--proof-ttl <1-3600>] [--force-unprivileged] --confirmed"
            })?,
        ),
        "agent-update-rollback" => submit_vty_agent_update_rollback(
            api_url,
            token,
            &proof_context.password,
            &proof_context.salt_hex,
            parse_vty_agent_update_rollback(&parts[1..]).with_context(|| {
                "usage: agent-update-rollback [--rollback-sha256-hex <sha256>] <target ...> [--timeout <1-3600>] [--proof-ttl <1-3600>] [--force-unprivileged] --confirmed"
            })?,
        ),
        "super-password-rotate" => submit_vty_super_password_rotate(
            api_url,
            token,
            &proof_context.password,
            &proof_context.salt_hex,
            parse_vty_super_password_rotate(&parts[1..]).with_context(|| {
                "usage: super-password-rotate (--new-proof-key-hex <hex>|--new-password-env <env> [--new-super-salt-hex <hex>]) [--rotation-generation <id>] <target ...> [--timeout <1-3600>] [--proof-ttl <1-3600>] --confirmed"
            })?,
        ),
        command => anyhow::bail!("unknown agent update command {command}"),
    }
}
