use anyhow::{Context, Result};
use serde::Deserialize;
use vpsman_common::JobCommand;

use crate::{
    http::{http_get, http_post_json},
    proof::build_envelopes_for_job_command,
};

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateRolloutActivateRequest {
    rollout_id: String,
    batch_size: Option<u16>,
    clients: Vec<String>,
    restart_agent: bool,
    timeout_secs: u64,
    proof_ttl_secs: u64,
    force_unprivileged: bool,
}

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateRolloutRollbackRequest {
    rollout_id: String,
    rollback_sha256_hex: Option<String>,
    clients: Vec<String>,
    timeout_secs: u64,
    proof_ttl_secs: u64,
    force_unprivileged: bool,
}

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateRolloutDelegateRollbackRequest {
    rollout_id: String,
    rollback_sha256_hex: Option<String>,
    clients: Vec<String>,
    proof_ttl_secs: u64,
    force_unprivileged: bool,
}

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateRolloutDelegateActivationRequest {
    rollout_id: String,
    clients: Vec<String>,
    restart_agent: bool,
    proof_ttl_secs: u64,
    force_unprivileged: bool,
}

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateRolloutControlRequest {
    rollout_id: String,
    pause: bool,
    resume: bool,
    pause_reason: Option<String>,
    health_gate: Option<String>,
}

#[derive(Debug)]
pub(crate) struct VtyAgentUpdateRolloutPolicyCreateRequest {
    name: String,
    scope_kind: String,
    scope_value: Option<String>,
    channel: Option<String>,
    canary_count: Option<i32>,
    health_gate: Option<String>,
    priority: i32,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct VtyAgentUpdateRolloutRecord {
    id: String,
    artifact_sha256_hex: String,
    canary_count: i32,
    #[serde(default)]
    automation_targets: Vec<String>,
    targets: Vec<VtyAgentUpdateRolloutTargetRecord>,
}

#[derive(Debug, Deserialize)]
struct VtyAgentUpdateRolloutTargetRecord {
    client_id: String,
    status: String,
}

pub(crate) fn is_vty_agent_update_rollouts_command(command: &str) -> bool {
    command == "agent-update-rollouts" || command.starts_with("agent-update-rollouts ")
}

pub(crate) fn is_vty_agent_update_rollout_policies_command(command: &str) -> bool {
    command == "agent-update-rollout-policies"
        || command.starts_with("agent-update-rollout-policies ")
}

pub(crate) fn is_vty_agent_update_rollout_policy_create_command(command: &str) -> bool {
    command == "agent-update-rollout-policy-create"
        || command.starts_with("agent-update-rollout-policy-create ")
}

pub(crate) fn is_vty_agent_update_rollout_activate_command(command: &str) -> bool {
    command == "agent-update-rollout-activate"
        || command.starts_with("agent-update-rollout-activate ")
}

pub(crate) fn is_vty_agent_update_rollout_rollback_command(command: &str) -> bool {
    command == "agent-update-rollout-rollback"
        || command.starts_with("agent-update-rollout-rollback ")
}

pub(crate) fn is_vty_agent_update_rollout_delegate_rollback_command(command: &str) -> bool {
    command == "agent-update-rollout-delegate-rollback"
        || command.starts_with("agent-update-rollout-delegate-rollback ")
}

pub(crate) fn is_vty_agent_update_rollout_delegate_activation_command(command: &str) -> bool {
    command == "agent-update-rollout-delegate-activation"
        || command.starts_with("agent-update-rollout-delegate-activation ")
}

pub(crate) fn is_vty_agent_update_rollout_control_command(command: &str) -> bool {
    command == "agent-update-rollout-control"
        || command.starts_with("agent-update-rollout-control ")
}

pub(crate) fn parse_vty_agent_update_rollout_activate(
    tokens: &[&str],
) -> Result<VtyAgentUpdateRolloutActivateRequest> {
    let mut rollout_id = None;
    let mut batch_size = None;
    let mut clients = Vec::new();
    let mut timeout_secs = 60_u64;
    let mut proof_ttl_secs = 300_u64;
    let mut confirmed = false;
    let mut restart_agent = false;
    let mut force_unprivileged = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--rollout-id" => {
                rollout_id = Some(
                    tokens
                        .get(index + 1)
                        .context("--rollout-id requires an id")?
                        .to_string(),
                );
                index += 2;
            }
            "--batch-size" => {
                batch_size = Some(
                    tokens
                        .get(index + 1)
                        .context("--batch-size requires a value")?
                        .parse()
                        .context("--batch-size must be an integer")?,
                );
                index += 2;
            }
            "--client" => {
                clients.push(
                    tokens
                        .get(index + 1)
                        .context("--client requires a client id")?
                        .to_string(),
                );
                index += 2;
            }
            "--timeout" => {
                timeout_secs = tokens
                    .get(index + 1)
                    .context("--timeout requires a value")?
                    .parse()
                    .context("--timeout must be an integer")?;
                index += 2;
            }
            "--proof-ttl" => {
                proof_ttl_secs = tokens
                    .get(index + 1)
                    .context("--proof-ttl requires a value")?
                    .parse()
                    .context("--proof-ttl must be an integer")?;
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            "--restart-agent" => {
                restart_agent = true;
                index += 1;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            other => anyhow::bail!("unknown agent-update-rollout-activate option {other}"),
        }
    }
    validate_config_dispatch_bounds(
        timeout_secs,
        proof_ttl_secs,
        "agent-update-rollout-activate",
    )?;
    if let Some(batch_size) = batch_size {
        anyhow::ensure!(
            (1..=10_000).contains(&batch_size),
            "agent-update-rollout-activate --batch-size must be between 1 and 10000"
        );
    }
    clients.sort();
    clients.dedup();
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-activate requires --confirmed because it promotes staged binaries"
    );
    Ok(VtyAgentUpdateRolloutActivateRequest {
        rollout_id: rollout_id.context("agent-update-rollout-activate requires --rollout-id")?,
        batch_size,
        clients,
        restart_agent,
        timeout_secs,
        proof_ttl_secs,
        force_unprivileged,
    })
}

pub(crate) fn parse_vty_agent_update_rollout_rollback(
    tokens: &[&str],
) -> Result<VtyAgentUpdateRolloutRollbackRequest> {
    let mut rollout_id = None;
    let mut rollback_sha256_hex = None;
    let mut clients = Vec::new();
    let mut timeout_secs = 60_u64;
    let mut proof_ttl_secs = 300_u64;
    let mut confirmed = false;
    let mut force_unprivileged = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--rollout-id" => {
                rollout_id = Some(
                    tokens
                        .get(index + 1)
                        .context("--rollout-id requires an id")?
                        .to_string(),
                );
                index += 2;
            }
            "--rollback-sha256-hex" => {
                rollback_sha256_hex = Some(validate_sha256(
                    tokens
                        .get(index + 1)
                        .context("--rollback-sha256-hex requires a value")?,
                    "--rollback-sha256-hex",
                )?);
                index += 2;
            }
            "--client" => {
                clients.push(
                    tokens
                        .get(index + 1)
                        .context("--client requires a client id")?
                        .to_string(),
                );
                index += 2;
            }
            "--timeout" => {
                timeout_secs = tokens
                    .get(index + 1)
                    .context("--timeout requires a value")?
                    .parse()
                    .context("--timeout must be an integer")?;
                index += 2;
            }
            "--proof-ttl" => {
                proof_ttl_secs = tokens
                    .get(index + 1)
                    .context("--proof-ttl requires a value")?
                    .parse()
                    .context("--proof-ttl must be an integer")?;
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            other => anyhow::bail!("unknown agent-update-rollout-rollback option {other}"),
        }
    }
    validate_config_dispatch_bounds(
        timeout_secs,
        proof_ttl_secs,
        "agent-update-rollout-rollback",
    )?;
    clients.sort();
    clients.dedup();
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-rollback requires --confirmed because it restores rollback binaries"
    );
    Ok(VtyAgentUpdateRolloutRollbackRequest {
        rollout_id: rollout_id.context("agent-update-rollout-rollback requires --rollout-id")?,
        rollback_sha256_hex,
        clients,
        timeout_secs,
        proof_ttl_secs,
        force_unprivileged,
    })
}

pub(crate) fn parse_vty_agent_update_rollout_delegate_rollback(
    tokens: &[&str],
) -> Result<VtyAgentUpdateRolloutDelegateRollbackRequest> {
    let mut rollout_id = None;
    let mut rollback_sha256_hex = None;
    let mut clients = Vec::new();
    let mut proof_ttl_secs = 3600_u64;
    let mut force_unprivileged = false;
    let mut confirmed = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--rollout-id" => {
                rollout_id = Some(
                    tokens
                        .get(index + 1)
                        .context("--rollout-id requires an id")?
                        .to_string(),
                );
                index += 2;
            }
            "--rollback-sha256-hex" => {
                rollback_sha256_hex = Some(validate_sha256(
                    tokens
                        .get(index + 1)
                        .context("--rollback-sha256-hex requires a value")?,
                    "--rollback-sha256-hex",
                )?);
                index += 2;
            }
            "--client" => {
                clients.push(
                    tokens
                        .get(index + 1)
                        .context("--client requires a client id")?
                        .to_string(),
                );
                index += 2;
            }
            "--proof-ttl" | "--proof-ttl-secs" => {
                proof_ttl_secs = tokens
                    .get(index + 1)
                    .context("--proof-ttl requires a value")?
                    .parse()
                    .context("--proof-ttl must be an integer")?;
                index += 2;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            other => anyhow::bail!("unknown agent-update-rollout-delegate-rollback option {other}"),
        }
    }
    anyhow::ensure!(
        (15..=86_400).contains(&proof_ttl_secs),
        "agent-update-rollout-delegate-rollback --proof-ttl must be between 15 and 86400"
    );
    clients.sort();
    clients.dedup();
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-delegate-rollback requires --confirmed because it escrows privileged rollback proofs"
    );
    Ok(VtyAgentUpdateRolloutDelegateRollbackRequest {
        rollout_id: rollout_id
            .context("agent-update-rollout-delegate-rollback requires --rollout-id")?,
        rollback_sha256_hex,
        clients,
        proof_ttl_secs,
        force_unprivileged,
    })
}

pub(crate) fn parse_vty_agent_update_rollout_delegate_activation(
    tokens: &[&str],
) -> Result<VtyAgentUpdateRolloutDelegateActivationRequest> {
    let mut rollout_id = None;
    let mut clients = Vec::new();
    let mut restart_agent = false;
    let mut proof_ttl_secs = 3600_u64;
    let mut force_unprivileged = false;
    let mut confirmed = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--rollout-id" => {
                rollout_id = Some(
                    tokens
                        .get(index + 1)
                        .context("--rollout-id requires an id")?
                        .to_string(),
                );
                index += 2;
            }
            "--client" => {
                clients.push(
                    tokens
                        .get(index + 1)
                        .context("--client requires a client id")?
                        .to_string(),
                );
                index += 2;
            }
            "--restart-agent" => {
                restart_agent = true;
                index += 1;
            }
            "--proof-ttl" | "--proof-ttl-secs" => {
                proof_ttl_secs = tokens
                    .get(index + 1)
                    .context("--proof-ttl requires a value")?
                    .parse()
                    .context("--proof-ttl must be an integer")?;
                index += 2;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            other => {
                anyhow::bail!("unknown agent-update-rollout-delegate-activation option {other}")
            }
        }
    }
    anyhow::ensure!(
        (15..=86_400).contains(&proof_ttl_secs),
        "agent-update-rollout-delegate-activation --proof-ttl must be between 15 and 86400"
    );
    clients.sort();
    clients.dedup();
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-delegate-activation requires --confirmed because it escrows privileged activation proofs"
    );
    Ok(VtyAgentUpdateRolloutDelegateActivationRequest {
        rollout_id: rollout_id
            .context("agent-update-rollout-delegate-activation requires --rollout-id")?,
        clients,
        restart_agent,
        proof_ttl_secs,
        force_unprivileged,
    })
}

pub(crate) fn parse_vty_agent_update_rollout_control(
    tokens: &[&str],
) -> Result<VtyAgentUpdateRolloutControlRequest> {
    let mut rollout_id = None;
    let mut pause = false;
    let mut resume = false;
    let mut pause_reason = None;
    let mut health_gate = None;
    let mut confirmed = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--rollout-id" => {
                rollout_id = Some(
                    tokens
                        .get(index + 1)
                        .context("--rollout-id requires an id")?
                        .to_string(),
                );
                index += 2;
            }
            "--pause" => {
                pause = true;
                index += 1;
            }
            "--resume" => {
                resume = true;
                index += 1;
            }
            "--pause-reason" => {
                pause_reason = Some(
                    tokens
                        .get(index + 1)
                        .context("--pause-reason requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--health-gate" => {
                let value = tokens
                    .get(index + 1)
                    .context("--health-gate requires a value")?;
                validate_rollout_health_gate(value)?;
                health_gate = Some((*value).to_string());
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            other => anyhow::bail!("unknown agent-update-rollout-control option {other}"),
        }
    }
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-control requires --confirmed because it changes rollout automation policy"
    );
    anyhow::ensure!(
        !(pause && resume),
        "agent-update-rollout-control cannot use --pause and --resume together"
    );
    anyhow::ensure!(
        pause || resume || health_gate.is_some(),
        "agent-update-rollout-control requires --pause, --resume, or --health-gate"
    );
    Ok(VtyAgentUpdateRolloutControlRequest {
        rollout_id: rollout_id.context("agent-update-rollout-control requires --rollout-id")?,
        pause,
        resume,
        pause_reason,
        health_gate,
    })
}

pub(crate) fn parse_vty_agent_update_rollout_policy_create(
    tokens: &[&str],
) -> Result<VtyAgentUpdateRolloutPolicyCreateRequest> {
    let mut name = None;
    let mut scope_kind = None;
    let mut scope_value = None;
    let mut channel = None;
    let mut canary_count = None;
    let mut health_gate = None;
    let mut priority = 0_i32;
    let mut enabled = true;
    let mut confirmed = false;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--name" => {
                name = Some(
                    tokens
                        .get(index + 1)
                        .context("--name requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--scope-kind" => {
                scope_kind = Some(
                    tokens
                        .get(index + 1)
                        .context("--scope-kind requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--scope-value" => {
                scope_value = Some(
                    tokens
                        .get(index + 1)
                        .context("--scope-value requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--channel" => {
                channel = Some(
                    tokens
                        .get(index + 1)
                        .context("--channel requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--canary-count" => {
                let value = tokens
                    .get(index + 1)
                    .context("--canary-count requires a value")?
                    .parse()
                    .context("--canary-count must be an integer")?;
                anyhow::ensure!(
                    (0..=10_000).contains(&value),
                    "agent-update-rollout-policy-create --canary-count must be between 0 and 10000"
                );
                canary_count = Some(value);
                index += 2;
            }
            "--health-gate" => {
                let value = tokens
                    .get(index + 1)
                    .context("--health-gate requires a value")?;
                validate_rollout_health_gate(value)?;
                health_gate = Some((*value).to_string());
                index += 2;
            }
            "--priority" => {
                priority = tokens
                    .get(index + 1)
                    .context("--priority requires a value")?
                    .parse()
                    .context("--priority must be an integer")?;
                index += 2;
            }
            "--disabled" => {
                enabled = false;
                index += 1;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            other => anyhow::bail!("unknown agent-update-rollout-policy-create option {other}"),
        }
    }
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-policy-create requires --confirmed because it changes reusable rollout defaults"
    );
    let scope_kind =
        scope_kind.context("agent-update-rollout-policy-create requires --scope-kind")?;
    validate_rollout_policy_scope(&scope_kind, scope_value.as_deref())?;
    Ok(VtyAgentUpdateRolloutPolicyCreateRequest {
        name: name.context("agent-update-rollout-policy-create requires --name")?,
        scope_kind,
        scope_value,
        channel,
        canary_count,
        health_gate,
        priority,
        enabled,
    })
}

pub(crate) fn submit_vty_agent_update_rollouts(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    anyhow::ensure!(
        parts.first() == Some(&"agent-update-rollouts"),
        "usage: agent-update-rollouts [--limit <1-200>]"
    );
    let mut limit = 25_u16;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = parts
                    .get(index + 1)
                    .context("--limit requires a value")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            other => anyhow::bail!("unknown agent-update-rollouts option {other}"),
        }
    }
    anyhow::ensure!(
        (1..=200).contains(&limit),
        "agent-update-rollouts --limit must be between 1 and 200"
    );
    http_get(
        api_url,
        &format!("/api/v1/agent-update-rollouts?limit={limit}"),
        token,
    )
}

pub(crate) fn submit_vty_agent_update_rollout_policies(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    anyhow::ensure!(
        parts.first() == Some(&"agent-update-rollout-policies"),
        "usage: agent-update-rollout-policies [--limit <1-200>] [--enabled true|false] [--channel <name>]"
    );
    let mut limit = 25_u16;
    let mut enabled: Option<bool> = None;
    let mut channel = None;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" => {
                limit = parts
                    .get(index + 1)
                    .context("--limit requires a value")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            "--enabled" => {
                enabled = Some(
                    parts
                        .get(index + 1)
                        .context("--enabled requires true or false")?
                        .parse()
                        .context("--enabled must be true or false")?,
                );
                index += 2;
            }
            "--channel" => {
                channel = Some(
                    parts
                        .get(index + 1)
                        .context("--channel requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            other => anyhow::bail!("unknown agent-update-rollout-policies option {other}"),
        }
    }
    anyhow::ensure!(
        (1..=200).contains(&limit),
        "agent-update-rollout-policies --limit must be between 1 and 200"
    );
    let mut path = format!("/api/v1/agent-update-rollout-policies?limit={limit}");
    if let Some(enabled) = enabled {
        path.push_str(&format!("&enabled={enabled}"));
    }
    if let Some(channel) = channel {
        path.push_str("&channel=");
        path.push_str(&crate::util::percent_encode_path_segment(&channel));
    }
    http_get(api_url, &path, token)
}

pub(crate) fn submit_vty_agent_update_rollout_policy_create(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().skip(1).collect::<Vec<_>>();
    let request = parse_vty_agent_update_rollout_policy_create(&parts)?;
    http_post_json(
        api_url,
        "/api/v1/agent-update-rollout-policies",
        token,
        &serde_json::json!({
            "name": request.name,
            "scope_kind": request.scope_kind,
            "scope_value": request.scope_value,
            "channel": request.channel,
            "canary_count": request.canary_count,
            "automation_health_gate": request.health_gate,
            "priority": request.priority,
            "enabled": request.enabled,
            "confirmed": true,
        }),
    )
}

pub(crate) fn submit_vty_agent_update_rollout_activate(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().skip(1).collect::<Vec<_>>();
    let request = parse_vty_agent_update_rollout_activate(&parts)?;
    let rollout = load_vty_rollout(api_url, token, &request.rollout_id)?;
    let clients = select_vty_rollout_targets(
        &rollout,
        &request.clients,
        request
            .batch_size
            .or(rollout_canary_batch_size(rollout.canary_count)),
        &["completed"],
        "no staged rollout targets are eligible for activation",
    )?;
    submit_vty_rollout_operation(
        api_url,
        token,
        password,
        salt_hex,
        "agent_update_activate",
        JobCommand::AgentUpdateActivate {
            staged_sha256_hex: rollout.artifact_sha256_hex,
            restart_agent: request.restart_agent,
        },
        clients,
        request.timeout_secs,
        request.proof_ttl_secs,
        request.force_unprivileged,
    )
}

pub(crate) fn submit_vty_agent_update_rollout_rollback(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().skip(1).collect::<Vec<_>>();
    let request = parse_vty_agent_update_rollout_rollback(&parts)?;
    let rollout = load_vty_rollout(api_url, token, &request.rollout_id)?;
    let clients = select_vty_rollout_targets(
        &rollout,
        &request.clients,
        None,
        &[
            "activation_pending_restart",
            "activation_failed",
            "heartbeat_timeout",
            "heartbeat_verified",
        ],
        "no activation-pending, activation-failed, heartbeat-timeout, or heartbeat-verified rollout targets are eligible for rollback",
    )?;
    submit_vty_rollout_operation(
        api_url,
        token,
        password,
        salt_hex,
        "agent_update_rollback",
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex: request.rollback_sha256_hex,
        },
        clients,
        request.timeout_secs,
        request.proof_ttl_secs,
        request.force_unprivileged,
    )
}

pub(crate) fn submit_vty_agent_update_rollout_delegate_rollback(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().skip(1).collect::<Vec<_>>();
    let request = parse_vty_agent_update_rollout_delegate_rollback(&parts)?;
    let rollout = load_vty_rollout(api_url, token, &request.rollout_id)?;
    let clients = select_vty_rollout_delegation_targets(&rollout, &request.clients)?;
    let operation = JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: request.rollback_sha256_hex.clone(),
    };
    let (_payload_hash_hex, envelopes) = build_envelopes_for_job_command(
        &clients,
        &operation,
        password,
        salt_hex,
        request.proof_ttl_secs,
    )?;
    http_post_json(
        api_url,
        &format!(
            "/api/v1/agent-update-rollouts/{}/rollback-delegation",
            crate::util::percent_encode_path_segment(&request.rollout_id)
        ),
        token,
        &serde_json::json!({
            "confirmed": true,
            "rollback_sha256_hex": request.rollback_sha256_hex,
            "force_unprivileged": request.force_unprivileged,
            "envelopes": envelopes,
        }),
    )
}

pub(crate) fn submit_vty_agent_update_rollout_delegate_activation(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().skip(1).collect::<Vec<_>>();
    let request = parse_vty_agent_update_rollout_delegate_activation(&parts)?;
    let rollout = load_vty_rollout(api_url, token, &request.rollout_id)?;
    let clients = select_vty_rollout_activation_delegation_targets(&rollout, &request.clients)?;
    let operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: rollout.artifact_sha256_hex.clone(),
        restart_agent: request.restart_agent,
    };
    let (_payload_hash_hex, envelopes) = build_envelopes_for_job_command(
        &clients,
        &operation,
        password,
        salt_hex,
        request.proof_ttl_secs,
    )?;
    http_post_json(
        api_url,
        &format!(
            "/api/v1/agent-update-rollouts/{}/activation-delegation",
            crate::util::percent_encode_path_segment(&request.rollout_id)
        ),
        token,
        &serde_json::json!({
            "confirmed": true,
            "restart_agent": request.restart_agent,
            "force_unprivileged": request.force_unprivileged,
            "envelopes": envelopes,
        }),
    )
}

pub(crate) fn submit_vty_agent_update_rollout_control(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().skip(1).collect::<Vec<_>>();
    let request = parse_vty_agent_update_rollout_control(&parts)?;
    let paused = if request.pause {
        Some(true)
    } else if request.resume {
        Some(false)
    } else {
        None
    };
    http_post_json(
        api_url,
        &format!(
            "/api/v1/agent-update-rollouts/{}/control",
            crate::util::percent_encode_path_segment(&request.rollout_id)
        ),
        token,
        &serde_json::json!({
            "confirmed": true,
            "paused": paused,
            "pause_reason": request.pause_reason,
            "automation_health_gate": request.health_gate,
        }),
    )
}

fn validate_config_dispatch_bounds(
    timeout_secs: u64,
    proof_ttl_secs: u64,
    command: &str,
) -> Result<()> {
    anyhow::ensure!(
        (1..=3600).contains(&timeout_secs),
        "{command} --timeout must be between 1 and 3600"
    );
    anyhow::ensure!(
        (1..=3600).contains(&proof_ttl_secs),
        "{command} --proof-ttl must be between 1 and 3600"
    );
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    anyhow::ensure!(
        value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "{label} must be 64 hex characters"
    );
    Ok(value)
}

fn validate_rollout_health_gate(value: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(
            value,
            "heartbeat_verified" | "manual_after_canary" | "manual_only"
        ),
        "--health-gate must be heartbeat_verified, manual_after_canary, or manual_only"
    );
    Ok(())
}

fn validate_rollout_policy_scope(scope_kind: &str, scope_value: Option<&str>) -> Result<()> {
    anyhow::ensure!(
        matches!(scope_kind, "global" | "tag" | "pool" | "provider"),
        "--scope-kind must be global, tag, pool, or provider"
    );
    if scope_kind == "global" {
        anyhow::ensure!(
            scope_value.map(str::trim).unwrap_or("").is_empty(),
            "--scope-value is not allowed for global rollout policies"
        );
    } else {
        anyhow::ensure!(
            !scope_value.map(str::trim).unwrap_or("").is_empty(),
            "--scope-value is required unless --scope-kind global"
        );
    }
    Ok(())
}

fn load_vty_rollout(
    api_url: &str,
    token: Option<&str>,
    rollout_id: &str,
) -> Result<VtyAgentUpdateRolloutRecord> {
    let body = http_get(api_url, "/api/v1/agent-update-rollouts?limit=200", token)?;
    let rollouts: Vec<VtyAgentUpdateRolloutRecord> =
        serde_json::from_str(&body).context("failed to parse agent update rollout list")?;
    rollouts
        .into_iter()
        .find(|rollout| rollout.id == rollout_id)
        .with_context(|| {
            format!("agent update rollout {rollout_id} was not found in latest 200 records")
        })
}

fn rollout_canary_batch_size(canary_count: i32) -> Option<u16> {
    (canary_count > 0).then_some(canary_count as u16)
}

fn select_vty_rollout_targets(
    rollout: &VtyAgentUpdateRolloutRecord,
    explicit_clients: &[String],
    batch_size: Option<u16>,
    eligible_statuses: &[&str],
    empty_message: &'static str,
) -> Result<Vec<String>> {
    let mut clients = rollout
        .automation_targets
        .iter()
        .filter(|client_id| {
            rollout.targets.iter().any(|target| {
                target.client_id == **client_id
                    && eligible_statuses.contains(&target.status.as_str())
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    if clients.is_empty() {
        clients = rollout
            .targets
            .iter()
            .filter(|target| eligible_statuses.contains(&target.status.as_str()))
            .map(|target| target.client_id.clone())
            .collect();
    }
    if !explicit_clients.is_empty() {
        clients.retain(|client_id| explicit_clients.iter().any(|wanted| wanted == client_id));
    }
    clients.sort();
    clients.dedup();
    if let Some(batch_size) = batch_size {
        clients.truncate(usize::from(batch_size.max(1)));
    }
    anyhow::ensure!(!clients.is_empty(), empty_message);
    Ok(clients)
}

fn select_vty_rollout_delegation_targets(
    rollout: &VtyAgentUpdateRolloutRecord,
    explicit_clients: &[String],
) -> Result<Vec<String>> {
    let rollout_clients = rollout
        .targets
        .iter()
        .map(|target| target.client_id.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut clients = if explicit_clients.is_empty() {
        let automation_targets = rollout
            .automation_targets
            .iter()
            .filter(|client_id| rollout_clients.contains(*client_id))
            .cloned()
            .collect::<Vec<_>>();
        if automation_targets.is_empty() {
            rollout_clients.iter().cloned().collect()
        } else {
            automation_targets
        }
    } else {
        for client_id in explicit_clients {
            anyhow::ensure!(
                rollout_clients.contains(client_id),
                "--client {client_id} is not part of this rollout"
            );
        }
        explicit_clients.to_vec()
    };
    clients.sort();
    clients.dedup();
    anyhow::ensure!(
        !clients.is_empty(),
        "no rollout targets are available for rollback delegation"
    );
    Ok(clients)
}

fn select_vty_rollout_activation_delegation_targets(
    rollout: &VtyAgentUpdateRolloutRecord,
    explicit_clients: &[String],
) -> Result<Vec<String>> {
    let rollout_clients = rollout
        .targets
        .iter()
        .map(|target| target.client_id.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut clients = if explicit_clients.is_empty() {
        rollout_clients.iter().cloned().collect()
    } else {
        for client_id in explicit_clients {
            anyhow::ensure!(
                rollout_clients.contains(client_id),
                "--client {client_id} is not part of this rollout"
            );
        }
        explicit_clients.to_vec()
    };
    clients.sort();
    clients.dedup();
    anyhow::ensure!(
        !clients.is_empty(),
        "no rollout targets are available for activation delegation"
    );
    Ok(clients)
}

#[allow(clippy::too_many_arguments)]
fn submit_vty_rollout_operation(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    command_label: &str,
    operation: JobCommand,
    clients: Vec<String>,
    timeout_secs: u64,
    proof_ttl_secs: u64,
    force_unprivileged: bool,
) -> Result<String> {
    let (_payload_hash_hex, envelopes) =
        build_envelopes_for_job_command(&clients, &operation, password, salt_hex, proof_ttl_secs)?;
    http_post_json(
        api_url,
        "/api/v1/jobs",
        token,
        &serde_json::json!({
            "command": command_label,
            "argv": [],
            "operation": operation,
            "clients": clients,
            "pools": [],
            "tags": [],
            "privileged": true,
            "destructive": false,
            "confirmed": true,
            "force_unprivileged": force_unprivileged,
            "timeout_secs": timeout_secs,
            "envelope": null,
            "envelopes": envelopes,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        is_vty_agent_update_rollout_activate_command, is_vty_agent_update_rollout_control_command,
        is_vty_agent_update_rollout_delegate_activation_command,
        is_vty_agent_update_rollout_delegate_rollback_command,
        is_vty_agent_update_rollout_policies_command,
        is_vty_agent_update_rollout_policy_create_command,
        is_vty_agent_update_rollout_rollback_command, is_vty_agent_update_rollouts_command,
        parse_vty_agent_update_rollout_activate, parse_vty_agent_update_rollout_control,
        parse_vty_agent_update_rollout_delegate_activation,
        parse_vty_agent_update_rollout_delegate_rollback,
        parse_vty_agent_update_rollout_policy_create, parse_vty_agent_update_rollout_rollback,
        select_vty_rollout_activation_delegation_targets, select_vty_rollout_delegation_targets,
        select_vty_rollout_targets, submit_vty_agent_update_rollout_policies,
        submit_vty_agent_update_rollouts, VtyAgentUpdateRolloutRecord,
        VtyAgentUpdateRolloutTargetRecord,
    };

    #[test]
    fn recognizes_agent_update_rollouts_command() {
        assert!(is_vty_agent_update_rollouts_command(
            "agent-update-rollouts"
        ));
        assert!(is_vty_agent_update_rollouts_command(
            "agent-update-rollouts --limit 10"
        ));
        assert!(!is_vty_agent_update_rollouts_command(
            "agent-update --limit 10"
        ));
        assert!(is_vty_agent_update_rollout_activate_command(
            "agent-update-rollout-activate --rollout-id abc --confirmed"
        ));
        assert!(is_vty_agent_update_rollout_rollback_command(
            "agent-update-rollout-rollback --rollout-id abc --confirmed"
        ));
        assert!(is_vty_agent_update_rollout_delegate_rollback_command(
            "agent-update-rollout-delegate-rollback --rollout-id abc --confirmed"
        ));
        assert!(is_vty_agent_update_rollout_delegate_activation_command(
            "agent-update-rollout-delegate-activation --rollout-id abc --confirmed"
        ));
        assert!(is_vty_agent_update_rollout_control_command(
            "agent-update-rollout-control --rollout-id abc --pause --confirmed"
        ));
        assert!(is_vty_agent_update_rollout_policies_command(
            "agent-update-rollout-policies --limit 10"
        ));
        assert!(is_vty_agent_update_rollout_policy_create_command(
            "agent-update-rollout-policy-create --name stable --scope-kind global --confirmed"
        ));
    }

    #[test]
    fn parses_rollout_activation_and_rollback_helpers() {
        let activate = parse_vty_agent_update_rollout_activate(&[
            "--rollout-id",
            "rollout-a",
            "--batch-size",
            "2",
            "--client",
            "edge-a",
            "--client",
            "edge-b",
            "--restart-agent",
            "--force-unprivileged",
            "--timeout",
            "45",
            "--confirmed",
        ])
        .unwrap();
        assert_eq!(activate.rollout_id, "rollout-a");
        assert_eq!(activate.batch_size, Some(2));
        assert_eq!(activate.clients, vec!["edge-a", "edge-b"]);
        assert!(activate.restart_agent);
        assert_eq!(activate.timeout_secs, 45);
        assert!(activate.force_unprivileged);

        let rollback = parse_vty_agent_update_rollout_rollback(&[
            "--rollout-id",
            "rollout-a",
            "--rollback-sha256-hex",
            &"cc".repeat(32),
            "--client",
            "edge-a",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();
        assert_eq!(rollback.rollout_id, "rollout-a");
        assert_eq!(rollback.rollback_sha256_hex, Some("cc".repeat(32)));
        assert_eq!(rollback.clients, vec!["edge-a"]);
        assert!(rollback.force_unprivileged);

        let delegation = parse_vty_agent_update_rollout_delegate_rollback(&[
            "--rollout-id",
            "rollout-a",
            "--rollback-sha256-hex",
            &"dd".repeat(32),
            "--client",
            "edge-b",
            "--proof-ttl-secs",
            "7200",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();
        assert_eq!(delegation.rollout_id, "rollout-a");
        assert_eq!(delegation.rollback_sha256_hex, Some("dd".repeat(32)));
        assert_eq!(delegation.clients, vec!["edge-b"]);
        assert_eq!(delegation.proof_ttl_secs, 7200);
        assert!(delegation.force_unprivileged);

        let activation_delegation = parse_vty_agent_update_rollout_delegate_activation(&[
            "--rollout-id",
            "rollout-a",
            "--client",
            "edge-b",
            "--restart-agent",
            "--proof-ttl-secs",
            "7200",
            "--confirmed",
        ])
        .unwrap();
        assert_eq!(activation_delegation.rollout_id, "rollout-a");
        assert_eq!(activation_delegation.clients, vec!["edge-b"]);
        assert!(activation_delegation.restart_agent);
        assert_eq!(activation_delegation.proof_ttl_secs, 7200);
        assert!(!activation_delegation.force_unprivileged);

        let control = parse_vty_agent_update_rollout_control(&[
            "--rollout-id",
            "rollout-a",
            "--pause",
            "--pause-reason",
            "maintenance",
            "--health-gate",
            "manual_after_canary",
            "--confirmed",
        ])
        .unwrap();
        assert_eq!(control.rollout_id, "rollout-a");
        assert!(control.pause);
        assert!(!control.resume);
        assert_eq!(control.pause_reason.as_deref(), Some("maintenance"));
        assert_eq!(control.health_gate.as_deref(), Some("manual_after_canary"));

        let policy = parse_vty_agent_update_rollout_policy_create(&[
            "--name",
            "hetzner-stable",
            "--scope-kind",
            "provider",
            "--scope-value",
            "hetzner",
            "--channel",
            "stable",
            "--canary-count",
            "2",
            "--health-gate",
            "manual_after_canary",
            "--priority",
            "10",
            "--confirmed",
        ])
        .unwrap();
        assert_eq!(policy.name, "hetzner-stable");
        assert_eq!(policy.scope_kind, "provider");
        assert_eq!(policy.scope_value.as_deref(), Some("hetzner"));
        assert_eq!(policy.channel.as_deref(), Some("stable"));
        assert_eq!(policy.canary_count, Some(2));
        assert_eq!(policy.health_gate.as_deref(), Some("manual_after_canary"));
        assert_eq!(policy.priority, 10);
        assert!(policy.enabled);

        assert!(parse_vty_agent_update_rollout_policy_create(&[
            "--name",
            "bad",
            "--scope-kind",
            "global",
            "--scope-value",
            "unexpected",
            "--confirmed",
        ])
        .is_err());
    }

    #[test]
    fn rejects_bad_agent_update_rollouts_limit_before_http() {
        assert!(submit_vty_agent_update_rollouts(
            "http://127.0.0.1:1",
            None,
            "agent-update-rollouts --limit 0"
        )
        .is_err());
        assert!(submit_vty_agent_update_rollouts(
            "http://127.0.0.1:1",
            None,
            "agent-update-rollouts --limit 201"
        )
        .is_err());
        assert!(submit_vty_agent_update_rollouts(
            "http://127.0.0.1:1",
            None,
            "agent-update-rollouts --bad"
        )
        .is_err());
        assert!(submit_vty_agent_update_rollout_policies(
            "http://127.0.0.1:1",
            None,
            "agent-update-rollout-policies --limit 0"
        )
        .is_err());
    }

    #[test]
    fn vty_rollout_target_selection_prefers_worker_recommendations() {
        let rollout = VtyAgentUpdateRolloutRecord {
            id: "rollout-a".to_string(),
            artifact_sha256_hex: "ab".repeat(32),
            canary_count: 1,
            automation_targets: vec!["client-b".to_string()],
            targets: vec![
                VtyAgentUpdateRolloutTargetRecord {
                    client_id: "client-a".to_string(),
                    status: "completed".to_string(),
                },
                VtyAgentUpdateRolloutTargetRecord {
                    client_id: "client-b".to_string(),
                    status: "completed".to_string(),
                },
            ],
        };

        let selected = select_vty_rollout_targets(
            &rollout,
            &[],
            Some(10),
            &["completed"],
            "no eligible targets",
        )
        .unwrap();

        assert_eq!(selected, vec!["client-b"]);

        let delegated = select_vty_rollout_delegation_targets(&rollout, &[]).unwrap();
        assert_eq!(delegated, vec!["client-b"]);
        let activation_delegated =
            select_vty_rollout_activation_delegation_targets(&rollout, &[]).unwrap();
        assert_eq!(activation_delegated, vec!["client-a", "client-b"]);
        assert!(
            select_vty_rollout_delegation_targets(&rollout, &["client-c".to_string()]).is_err()
        );
    }
}
