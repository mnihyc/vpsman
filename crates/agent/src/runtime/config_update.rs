use anyhow::{Context, Result};
use vpsman_common::{AgentConfig, AgentRuntimeConfig, CommandOutput, OutputStream};

pub(crate) fn read_redacted_config(
    job_id: uuid::Uuid,
    current: &AgentConfig,
) -> Result<Vec<CommandOutput>> {
    let runtime_config = projected_runtime_config(current)?;
    Ok(vec![CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "config_read",
            "status": "read",
            "scope": "effective_runtime_config",
            "runtime_config": runtime_config,
        }))?,
        exit_code: Some(0),
        done: true,
    }])
}

fn projected_runtime_config(current: &AgentConfig) -> Result<serde_json::Value> {
    let mut runtime_config = AgentRuntimeConfig::from_agent_config(0, current);
    for plan in &mut runtime_config.network.runtime_status_telemetry_plans {
        plan.builtin_credentials = None;
    }
    let mut projected = serde_json::to_value(runtime_config)
        .context("failed to serialize effective runtime config")?;
    projected
        .as_object_mut()
        .context("effective runtime config did not serialize as an object")?
        .remove("version");
    Ok(projected)
}
