use std::path::PathBuf;

use anyhow::{Context, Result};

pub(crate) fn agent_state_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("VPSMAN_AGENT_STATE_DIR")
        .or_else(|| std::env::var_os("VPSMAN_STATE_DIR"))
        .filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(path));
    }
    Ok(std::env::current_dir()
        .context("failed to resolve agent working directory")?
        .join("state"))
}
