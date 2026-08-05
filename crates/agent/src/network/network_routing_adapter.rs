use std::{path::Path, process::Stdio, str, time::Duration};

use anyhow::{Context, Result};
use tokio::{process::Command, time::Instant};
use vpsman_common::{
    render_tunnel_endpoint_config, CommandOutput, OutputStream, RoutingCostAdapterCommands,
    RoutingCostAdapterJobResult, RoutingCostAdapterOperation, RuntimeTunnelCommand,
    TunnelEndpointConfig, TunnelEndpointSide, TunnelPlan, ROUTING_COST_ADAPTER_CONTRACT_VERSION,
};

use crate::{
    child_process::{run_child_with_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult},
    command_worker::{CommandCancelToken, CommandCanceled},
    network_runtime::render_runtime_adapter_command_with_placeholders,
};

pub(crate) struct NetworkRoutingAdapterInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) client_id: &'a str,
    pub(crate) plan_id: &'a str,
    pub(crate) plan: &'a TunnelPlan,
    pub(crate) side: TunnelEndpointSide,
    pub(crate) adapter: &'a RoutingCostAdapterCommands,
    pub(crate) expected_current_cost: Option<u16>,
    pub(crate) desired_cost: Option<u16>,
    pub(crate) max_timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
}

pub(crate) async fn execute_network_routing_adapter_command(
    input: NetworkRoutingAdapterInput<'_>,
) -> Result<Vec<CommandOutput>> {
    validate_adapter_snapshot(input.adapter)?;
    let endpoint = render_tunnel_endpoint_config(input.plan, input.side)
        .map_err(|error| anyhow::anyhow!("invalid tunnel endpoint config: {error}"))?;
    if endpoint.local_client_id != input.client_id {
        anyhow::bail!(
            "routing adapter side targets {}, but this agent is {}",
            endpoint.local_client_id,
            input.client_id
        );
    }

    let operation = if input.desired_cost.is_some() {
        RoutingCostAdapterOperation::Apply
    } else {
        RoutingCostAdapterOperation::Status
    };
    let deadline = Instant::now() + Duration::from_secs(input.max_timeout_secs.max(1));
    let before = run_status_command(
        &input.adapter.status,
        input.plan_id,
        input.plan,
        &endpoint,
        input.side,
        input.expected_current_cost,
        input.desired_cost,
        remaining_secs(deadline)?,
        input.cancel_token.clone(),
    )
    .await?;

    let (previous_cost, current_cost, message) = if let Some(desired_cost) = input.desired_cost {
        if input
            .expected_current_cost
            .is_some_and(|expected| before != expected)
        {
            anyhow::bail!(
                "stale routing cost confirmation: expected {:?}, observed {}",
                input.expected_current_cost,
                before
            );
        }
        let update_output = run_adapter_command(
            &input.adapter.update,
            input.plan_id,
            input.plan,
            &endpoint,
            input.side,
            input.expected_current_cost,
            Some(desired_cost),
            remaining_secs(deadline)?,
            input.cancel_token.clone(),
            "update",
        )
        .await?;
        let after = run_status_command(
            &input.adapter.status,
            input.plan_id,
            input.plan,
            &endpoint,
            input.side,
            input.expected_current_cost,
            Some(desired_cost),
            remaining_secs(deadline)?,
            input.cancel_token.clone(),
        )
        .await?;
        if after != desired_cost {
            anyhow::bail!(
                "routing cost verification failed: desired {desired_cost}, observed {after}"
            );
        }
        (Some(before), after, output_message(&update_output))
    } else {
        (None, before, None)
    };

    let result = RoutingCostAdapterJobResult {
        contract_version: ROUTING_COST_ADAPTER_CONTRACT_VERSION,
        operation,
        plan_id: input.plan_id.to_string(),
        endpoint_side: input.side,
        client_id: input.client_id.to_string(),
        adapter_definition_id: input.adapter.definition_id.clone(),
        adapter_definition_hash: input.adapter.definition_hash.clone(),
        previous_cost,
        current_cost,
        message,
    };
    Ok(vec![CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&result)?,
        exit_code: Some(0),
        done: true,
    }])
}

async fn run_adapter_command(
    command: &RuntimeTunnelCommand,
    plan_id: &str,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    side: TunnelEndpointSide,
    expected_current_cost: Option<u16>,
    desired_cost: Option<u16>,
    remaining_timeout_secs: u64,
    cancel_token: CommandCancelToken,
    phase: &str,
) -> Result<Vec<u8>> {
    validate_adapter_command(command)?;
    let argv = render_routing_adapter_command(
        command,
        plan_id,
        plan,
        endpoint,
        side,
        expected_current_cost,
        desired_cost,
    )?;
    let mut child = Command::new(&argv[0]);
    child.args(&argv[1..]).stdin(Stdio::null());
    let max_output_bytes =
        usize::try_from(command.max_output_bytes.clamp(1024, 64 * 1024)).unwrap_or(64 * 1024);
    let result = run_child_with_bounded_output_cancelable(
        child,
        command
            .max_timeout_secs
            .clamp(1, 120)
            .min(remaining_timeout_secs.max(1)),
        max_output_bytes,
        ChildCleanupPolicy::ProcessGroup,
        cancel_token,
    )
    .await
    .with_context(|| format!("failed to execute routing cost adapter {phase}"))?;
    let output = match result {
        ChildRunResult::Completed(output) => output,
        ChildRunResult::TimedOut(_) => anyhow::bail!("routing cost adapter {phase} timed out"),
        ChildRunResult::Canceled { reason, .. } => {
            return Err(CommandCanceled::new("network_routing_adapter", reason).into())
        }
    };
    if output.stdout_truncated || output.stderr_truncated {
        anyhow::bail!("routing cost adapter {phase} exceeded the output limit");
    }
    if output.exit_code != Some(0) {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = match (stdout.trim(), stderr.trim()) {
            ("", "") => "no message".to_string(),
            (stdout, "") => stdout.to_string(),
            ("", stderr) => stderr.to_string(),
            (stdout, stderr) => format!("{stdout}; {stderr}"),
        };
        anyhow::bail!(
            "routing cost adapter {phase} exited with {:?}: {}",
            output.exit_code,
            message
        );
    }
    Ok(output.stdout)
}

#[allow(clippy::too_many_arguments)]
async fn run_status_command(
    command: &RuntimeTunnelCommand,
    plan_id: &str,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    side: TunnelEndpointSide,
    expected_current_cost: Option<u16>,
    desired_cost: Option<u16>,
    remaining_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<u16> {
    let stdout = run_adapter_command(
        command,
        plan_id,
        plan,
        endpoint,
        side,
        expected_current_cost,
        desired_cost,
        remaining_timeout_secs,
        cancel_token,
        "status",
    )
    .await?;
    parse_status_cost(&stdout)
}

fn render_routing_adapter_command(
    command: &RuntimeTunnelCommand,
    plan_id: &str,
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    side: TunnelEndpointSide,
    expected_current_cost: Option<u16>,
    desired_cost: Option<u16>,
) -> Result<Vec<String>> {
    let expected_current_cost = expected_current_cost
        .map(|value| value.to_string())
        .unwrap_or_default();
    let desired_cost = desired_cost
        .map(|value| value.to_string())
        .unwrap_or_default();
    let endpoint_side = match side {
        TunnelEndpointSide::Left => "left",
        TunnelEndpointSide::Right => "right",
    };
    let placeholders = [
        ("{plan_id}", plan_id.to_string()),
        ("{endpoint_side}", endpoint_side.to_string()),
        ("{expected_current_cost}", expected_current_cost),
        ("{desired_cost}", desired_cost),
    ];
    render_runtime_adapter_command_with_placeholders(command, plan, endpoint, &placeholders)
}

fn parse_status_cost(stdout: &[u8]) -> Result<u16> {
    let value = str::from_utf8(stdout)
        .context("routing cost adapter status output is not UTF-8")?
        .trim();
    let cost = value.parse::<u16>().map_err(|_| {
        anyhow::anyhow!("routing cost adapter status must output one cost from 1 to 65535")
    })?;
    if cost == 0 {
        anyhow::bail!("routing cost adapter status must output one cost from 1 to 65535");
    }
    Ok(cost)
}

fn output_message(stdout: &[u8]) -> Option<String> {
    let message = String::from_utf8_lossy(stdout).trim().to_string();
    (!message.is_empty()).then_some(message)
}

fn validate_adapter_snapshot(adapter: &RoutingCostAdapterCommands) -> Result<()> {
    if adapter.definition_id.trim().is_empty() || adapter.definition_id.len() > 128 {
        anyhow::bail!("routing adapter definition id is invalid");
    }
    if adapter.definition_name.trim().is_empty() || adapter.definition_name.len() > 160 {
        anyhow::bail!("routing adapter definition name is invalid");
    }
    if adapter.definition_hash.len() != 64
        || !adapter
            .definition_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        anyhow::bail!("routing adapter definition hash is invalid");
    }
    validate_adapter_command(&adapter.status)?;
    validate_adapter_command(&adapter.update)
}

fn validate_adapter_command(command: &RuntimeTunnelCommand) -> Result<()> {
    if command.argv.is_empty() || command.argv.len() > 32 {
        anyhow::bail!("routing adapter argv must contain 1 to 32 entries");
    }
    if !Path::new(&command.argv[0]).is_absolute() {
        anyhow::bail!("routing adapter executable must be absolute");
    }
    if command
        .argv
        .iter()
        .any(|part| part.is_empty() || part.len() > 4096 || part.contains('\0'))
    {
        anyhow::bail!("routing adapter argv contains an invalid entry");
    }
    if !(1..=120).contains(&command.max_timeout_secs) {
        anyhow::bail!("routing adapter timeout must be between 1 and 120 seconds");
    }
    if !(1024..=64 * 1024).contains(&command.max_output_bytes) {
        anyhow::bail!("routing adapter output limit must be between 1024 and 65536 bytes");
    }
    Ok(())
}

fn remaining_secs(deadline: Instant) -> Result<u64> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        anyhow::bail!("routing adapter workflow timed out");
    }
    Ok(remaining.as_secs().max(1))
}

#[cfg(test)]
#[path = "tests_network_routing_adapter.rs"]
mod tests;
