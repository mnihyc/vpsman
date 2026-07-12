use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::{payload_hash, RuntimeTunnelAdapterCommands, RuntimeTunnelCommand};

use crate::repository::Repository;

impl Repository {
    pub(crate) async fn resolve_runtime_tunnel_adapter(
        &self,
        template_id: &str,
        client_id: &str,
    ) -> Result<RuntimeTunnelAdapterCommands> {
        let template_id =
            Uuid::parse_str(template_id).context("runtime_tunnel_adapter_template_id_invalid")?;
        let template = self
            .source_template_by_id_in_domain(template_id, "runtime_tunnel_adapter")
            .await?
            .context("runtime_tunnel_adapter_template_not_found")?;
        if template.domain != "runtime_tunnel_adapter" {
            anyhow::bail!("runtime_tunnel_adapter_template_domain_mismatch");
        }
        if template.scope == "vps_local" && template.owner_client_id.as_deref() != Some(client_id) {
            anyhow::bail!("runtime_tunnel_adapter_template_scope_mismatch");
        }
        if !matches!(template.scope.as_str(), "shared" | "vps_local") {
            anyhow::bail!("runtime_tunnel_adapter_template_scope_invalid");
        }
        if template
            .definition
            .get("manager")
            .and_then(serde_json::Value::as_str)
            != Some("external_managed_adapter")
            || template
                .definition
                .get("contract_version")
                .and_then(serde_json::Value::as_u64)
                != Some(1)
        {
            anyhow::bail!("runtime_tunnel_adapter_contract_invalid");
        }
        let status = required_command(&template.definition, "status_command")?;
        let startup = optional_command(&template.definition, "startup_command")?;
        let stop = optional_command(&template.definition, "stop_command")?;
        let cleanup = optional_command(&template.definition, "cleanup_command")?;
        let restart = optional_command(&template.definition, "restart_command")?;
        let traffic_limit_apply = optional_command(&template.definition, "traffic_limit_command")?;
        if startup.is_none() && restart.is_none() {
            anyhow::bail!("runtime_tunnel_adapter_start_command_required");
        }
        if stop.is_none() && cleanup.is_none() {
            anyhow::bail!("runtime_tunnel_adapter_remove_command_required");
        }
        let definition = serde_json::to_vec(&template.definition)?;
        Ok(RuntimeTunnelAdapterCommands {
            template_id: template.id.to_string(),
            template_name: template.name,
            definition_hash: payload_hash(&definition),
            startup,
            stop,
            cleanup,
            restart,
            status,
            traffic_limit_apply,
        })
    }
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
