use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use vpsman_common::{
    TunnelEndpointConfig, TunnelPlan, MANAGED_BIRD2_FILE, MANAGED_IFUPDOWN_FILE,
    MANAGED_NETPLAN_FILE, MANAGED_SYSTEMD_NETWORKD_NETDEV_FILE,
    MANAGED_SYSTEMD_NETWORKD_NETWORK_FILE,
};

const MAX_MANAGED_NETWORK_FILE_BYTES: u64 = 256 * 1024;

pub(crate) fn managed_destination(root: &Path, managed_path: &str) -> Result<PathBuf> {
    if !is_supported_managed_path(managed_path) {
        anyhow::bail!("unsupported managed network path {managed_path}");
    }
    let relative = managed_path.trim_start_matches('/');
    let destination = root.join(relative);
    Ok(destination)
}

fn is_supported_managed_path(managed_path: &str) -> bool {
    matches!(
        managed_path,
        MANAGED_IFUPDOWN_FILE
            | MANAGED_NETPLAN_FILE
            | MANAGED_SYSTEMD_NETWORKD_NETDEV_FILE
            | MANAGED_SYSTEMD_NETWORKD_NETWORK_FILE
            | MANAGED_BIRD2_FILE
    )
}

pub(crate) fn managed_block(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    block_kind: &'static str,
    contents: &str,
) -> String {
    format!(
        "# BEGIN VPSMAN {block_kind} plan={} side={} peer={}\n{}\n# END VPSMAN {block_kind} plan={} side={} peer={}\n",
        plan.name,
        endpoint_side_name(endpoint.side),
        endpoint.peer_client_id,
        contents.trim_end(),
        plan.name,
        endpoint_side_name(endpoint.side),
        endpoint.peer_client_id
    )
}

fn endpoint_side_name(side: vpsman_common::TunnelEndpointSide) -> &'static str {
    match side {
        vpsman_common::TunnelEndpointSide::Left => "left",
        vpsman_common::TunnelEndpointSide::Right => "right",
    }
}

pub(crate) async fn read_existing_regular_file(path: &Path) -> Result<Option<Vec<u8>>> {
    let Some(metadata) = tokio::fs::metadata(path).await.optional()? else {
        return Ok(None);
    };
    if !metadata.is_file() {
        anyhow::bail!("{} exists but is not a regular file", path.display());
    }
    if metadata.len() > MAX_MANAGED_NETWORK_FILE_BYTES {
        anyhow::bail!("{} exceeds managed network file limit", path.display());
    }
    Ok(Some(tokio::fs::read(path).await.with_context(|| {
        format!("failed to read {}", path.display())
    })?))
}

pub(crate) fn managed_block_bounds(existing: &str, block: &str) -> Result<Option<(usize, usize)>> {
    let mut lines = block.lines();
    let begin = lines.next().context("managed block missing begin marker")?;
    let end = block
        .lines()
        .last()
        .context("managed block missing end marker")?;
    let Some(begin_index) = existing.find(begin) else {
        return Ok(None);
    };
    let after_begin = &existing[begin_index + begin.len()..];
    let Some(relative_end) = after_begin.find(end) else {
        anyhow::bail!("managed block begin marker found without matching end marker");
    };
    let end_index = begin_index + begin.len() + relative_end + end.len();
    let mut final_end = end_index;
    if existing.as_bytes().get(final_end) == Some(&b'\n') {
        final_end += 1;
    }
    Ok(Some((begin_index, final_end)))
}

trait OptionalIo<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalIo<T> for std::io::Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}
