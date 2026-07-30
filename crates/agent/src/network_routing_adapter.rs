use std::{path::Path, time::Duration};

use anyhow::{Context, Result};
use tokio::{process::Command, time::Instant};
use vpsman_common::{
    render_tunnel_endpoint_config, CommandOutput, OutputStream, RoutingCostAdapterCommands,
    RoutingCostAdapterJobResult, RoutingCostAdapterOperation, RoutingCostAdapterRequest,
    RoutingCostAdapterResponse, RuntimeTunnelCommand, TunnelEndpointSide, TunnelPlan,
    ROUTING_COST_ADAPTER_CONTRACT_VERSION,
};

use crate::{
    child_process::{
        run_child_with_input_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult,
    },
    command_worker::{CommandCancelToken, CommandCanceled},
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
    let status_request = adapter_request(&input, RoutingCostAdapterOperation::Status, None);
    let before = run_adapter_command(
        &input.adapter.status,
        &status_request,
        remaining_secs(deadline)?,
        input.cancel_token.clone(),
    )
    .await?;
    validate_adapter_response(&before, &input.plan.interface_name, "status")?;
    if !before.ready {
        anyhow::bail!(
            "routing adapter is not ready{}",
            response_message_suffix(&before)
        );
    }

    let (before_result, update_result, after) = if let Some(desired_cost) = input.desired_cost {
        if before.current_cost != input.expected_current_cost {
            anyhow::bail!(
                "stale routing cost confirmation: expected {:?}, observed {:?}",
                input.expected_current_cost,
                before.current_cost
            );
        }
        let update_request = adapter_request(
            &input,
            RoutingCostAdapterOperation::Apply,
            Some(desired_cost),
        );
        let update = run_adapter_command(
            &input.adapter.update,
            &update_request,
            remaining_secs(deadline)?,
            input.cancel_token.clone(),
        )
        .await?;
        validate_adapter_response(&update, &input.plan.interface_name, "update")?;
        if !update.ready || update.applied_cost != Some(desired_cost) {
            anyhow::bail!(
                "routing adapter did not acknowledge desired cost {desired_cost}; applied {:?}{}",
                update.applied_cost,
                response_message_suffix(&update)
            );
        }

        let after = run_adapter_command(
            &input.adapter.status,
            &status_request,
            remaining_secs(deadline)?,
            input.cancel_token.clone(),
        )
        .await?;
        validate_adapter_response(&after, &input.plan.interface_name, "verification status")?;
        if !after.ready || after.current_cost != Some(desired_cost) {
            anyhow::bail!(
                "routing cost verification failed: desired {desired_cost}, observed {:?}{}",
                after.current_cost,
                response_message_suffix(&after)
            );
        }
        (Some(before), Some(update), after)
    } else {
        (None, None, before)
    };

    let result = RoutingCostAdapterJobResult {
        contract_version: ROUTING_COST_ADAPTER_CONTRACT_VERSION,
        operation,
        plan_id: input.plan_id.to_string(),
        endpoint_side: input.side,
        client_id: input.client_id.to_string(),
        adapter_definition_id: input.adapter.definition_id.clone(),
        adapter_definition_hash: input.adapter.definition_hash.clone(),
        before: before_result,
        update: update_result,
        after,
    };
    Ok(vec![CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&result)?,
        exit_code: Some(0),
        done: true,
    }])
}

fn adapter_request(
    input: &NetworkRoutingAdapterInput<'_>,
    operation: RoutingCostAdapterOperation,
    desired_cost: Option<u16>,
) -> RoutingCostAdapterRequest {
    let (client_id, peer_client_id, local_underlay, remote_underlay, local_address, remote_address) =
        match input.side {
            TunnelEndpointSide::Left => (
                &input.plan.left_client_id,
                &input.plan.right_client_id,
                &input.plan.left_local_underlay,
                &input.plan.left_remote_underlay,
                &input.plan.left_tunnel_address,
                &input.plan.right_tunnel_address,
            ),
            TunnelEndpointSide::Right => (
                &input.plan.right_client_id,
                &input.plan.left_client_id,
                &input.plan.right_local_underlay,
                &input.plan.right_remote_underlay,
                &input.plan.right_tunnel_address,
                &input.plan.left_tunnel_address,
            ),
        };
    RoutingCostAdapterRequest {
        contract_version: ROUTING_COST_ADAPTER_CONTRACT_VERSION,
        operation,
        plan_id: input.plan_id.to_string(),
        plan_name: input.plan.name.clone(),
        interface_name: input.plan.interface_name.clone(),
        endpoint_side: input.side,
        client_id: client_id.clone(),
        peer_client_id: peer_client_id.clone(),
        local_underlay: local_underlay.clone(),
        remote_underlay: remote_underlay.clone(),
        local_address: local_address.clone(),
        remote_address: remote_address.clone(),
        prefix_len: input.plan.tunnel_prefix_len,
        expected_current_cost: input.expected_current_cost,
        desired_cost,
    }
}

async fn run_adapter_command(
    command: &RuntimeTunnelCommand,
    request: &RoutingCostAdapterRequest,
    remaining_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<RoutingCostAdapterResponse> {
    validate_adapter_command(command)?;
    let mut child = Command::new(&command.argv[0]);
    child.args(&command.argv[1..]);
    let mut input = serde_json::to_vec(request)?;
    input.push(b'\n');
    let max_output_bytes =
        usize::try_from(command.max_output_bytes.clamp(1024, 64 * 1024)).unwrap_or(64 * 1024);
    let result = run_child_with_input_bounded_output_cancelable(
        child,
        input,
        command
            .max_timeout_secs
            .clamp(1, 120)
            .min(remaining_timeout_secs.max(1)),
        max_output_bytes,
        ChildCleanupPolicy::ProcessGroup,
        cancel_token,
    )
    .await
    .context("failed to execute routing cost adapter")?;
    let output = match result {
        ChildRunResult::Completed(output) => output,
        ChildRunResult::TimedOut(_) => anyhow::bail!("routing cost adapter timed out"),
        ChildRunResult::Canceled { reason, .. } => {
            return Err(CommandCanceled::new("network_routing_adapter", reason).into())
        }
    };
    if output.stdout_truncated || output.stderr_truncated {
        anyhow::bail!("routing cost adapter exceeded the output limit");
    }
    if output.exit_code != Some(0) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "routing cost adapter exited with {:?}: {}",
            output.exit_code,
            stderr.trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("routing cost adapter returned invalid JSON")
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

fn validate_adapter_response(
    response: &RoutingCostAdapterResponse,
    interface_name: &str,
    phase: &str,
) -> Result<()> {
    if response.contract_version != ROUTING_COST_ADAPTER_CONTRACT_VERSION {
        anyhow::bail!(
            "routing adapter {phase} returned contract version {}, expected {}",
            response.contract_version,
            ROUTING_COST_ADAPTER_CONTRACT_VERSION
        );
    }
    if response.interface_name != interface_name {
        anyhow::bail!(
            "routing adapter {phase} returned interface {}, expected {interface_name}",
            response.interface_name
        );
    }
    if response
        .message
        .as_ref()
        .is_some_and(|value| value.len() > 1024)
    {
        anyhow::bail!("routing adapter {phase} message exceeds 1024 bytes");
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

fn response_message_suffix(response: &RoutingCostAdapterResponse) -> String {
    response
        .message
        .as_deref()
        .filter(|message| !message.trim().is_empty())
        .map(|message| format!(": {message}"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
