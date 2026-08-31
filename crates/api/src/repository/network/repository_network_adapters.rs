use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::{payload_hash, RuntimeTunnelAdapterCommands, RuntimeTunnelCommand};

use crate::{
    model::NetworkAdapterDefinitionView, repository::Repository,
    repository_configuration_presets::validate_network_adapter_definition_view,
};

impl Repository {
    pub(crate) async fn resolve_runtime_tunnel_adapter(
        &self,
        definition_id: &str,
    ) -> Result<RuntimeTunnelAdapterCommands> {
        let definition_id = Uuid::parse_str(definition_id)
            .context("runtime_tunnel_adapter_definition_id_invalid")?;
        let definition = self
            .network_adapter_definition_by_id(definition_id, Some("runtime_tunnel"))
            .await?
            .context("runtime_tunnel_adapter_definition_not_found")?;
        runtime_tunnel_adapter_from_definition(&definition)
    }
}

pub(crate) fn runtime_tunnel_adapter_from_definition(
    definition: &NetworkAdapterDefinitionView,
) -> Result<RuntimeTunnelAdapterCommands> {
    validate_network_adapter_definition_view(definition)
        .context("runtime_tunnel_adapter_definition_invalid")?;
    if definition
        .definition
        .get("manager")
        .and_then(serde_json::Value::as_str)
        != Some("custom_adapter")
        || definition
            .definition
            .get("contract_version")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
    {
        anyhow::bail!("runtime_tunnel_adapter_contract_invalid");
    }
    let status = required_command(&definition.definition, "status_command")?;
    let startup = optional_command(&definition.definition, "startup_command")?;
    let stop = optional_command(&definition.definition, "stop_command")?;
    let cleanup = optional_command(&definition.definition, "cleanup_command")?;
    let restart = optional_command(&definition.definition, "restart_command")?;
    let traffic_limit_apply = optional_command(&definition.definition, "traffic_limit_command")?;
    if startup.is_none() && restart.is_none() {
        anyhow::bail!("runtime_tunnel_adapter_start_command_required");
    }
    if stop.is_none() && cleanup.is_none() {
        anyhow::bail!("runtime_tunnel_adapter_remove_command_required");
    }
    let definition_json = serde_json::to_vec(&definition.definition)?;
    Ok(RuntimeTunnelAdapterCommands {
        definition_id: definition.id.to_string(),
        definition_name: definition.name.clone(),
        definition_hash: payload_hash(&definition_json),
        startup,
        stop,
        cleanup,
        restart,
        status,
        traffic_limit_apply,
    })
}

fn required_command(definition: &serde_json::Value, field: &str) -> Result<RuntimeTunnelCommand> {
    optional_command(definition, field)?
        .with_context(|| format!("runtime_tunnel_adapter_{field}_required"))
}

fn optional_command(
    definition: &serde_json::Value,
    field: &str,
) -> Result<Option<RuntimeTunnelCommand>> {
    let Some(value) = definition.get(field) else {
        return Ok(None);
    };
    let command: RuntimeTunnelCommand = serde_json::from_value(value.clone())
        .with_context(|| format!("runtime_tunnel_adapter_{field}_invalid"))?;
    validate_command(&command)?;
    Ok(Some(command))
}

fn validate_command(command: &RuntimeTunnelCommand) -> Result<()> {
    if command.argv.is_empty()
        || command.argv.len() > 32
        || !command.argv[0].starts_with('/')
        || command
            .argv
            .iter()
            .any(|part| part.is_empty() || part.len() > 4096 || part.contains('\0'))
        || !(1..=120).contains(&command.max_timeout_secs)
        || !(1024..=64 * 1024).contains(&command.max_output_bytes)
    {
        anyhow::bail!("runtime_tunnel_adapter_command_invalid");
    }
    Ok(())
}
