use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use vpsman_common::{validate_agent_config_shape, AgentConfig};

#[derive(Debug, Parser)]
#[command(
    name = "vpsman-agent",
    about = "Headless VPS management agent",
    version = concat!(
        env!("VPSMAN_RELEASE_VERSION"),
        " (agent build ",
        env!("VPSMAN_AGENT_BUILD_NUMBER"),
        ")"
    )
)]
pub(crate) struct Args {
    #[arg(
        long,
        env = "VPSMAN_AGENT_CONFIG",
        default_value = "/etc/vpsman/agent.toml"
    )]
    pub(crate) config: PathBuf,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Run {
        #[arg(long, env = "VPSMAN_GATEWAY_ADDR")]
        endpoint: Option<String>,
    },
    Once,
}

pub(crate) fn load_config(path: &Path) -> Result<AgentConfig> {
    if !path.exists() {
        anyhow::bail!("agent config {} is required", path.display());
    }
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read agent config {}", path.display()))?;
    let config: AgentConfig = toml::from_str(&contents)
        .with_context(|| format!("failed to parse agent config {}", path.display()))?;
    validate_agent_config_shape(&config)
        .map_err(|message| anyhow::anyhow!("invalid agent config: {message}"))?;
    Ok(config)
}
