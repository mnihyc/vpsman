use anyhow::Result;
use vpsman_common::JobCommand;

use crate::{
    http::http_get, jobs::submit_unprivileged_operation, util::percent_encode_path_segment,
};

pub(crate) fn storage_inventory(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    include_system_mounts: bool,
    clients: Vec<String>,
    tags: Vec<String>,
    max_timeout_secs: u64,
) -> Result<()> {
    let operation = storage_inventory_operation(limit, include_system_mounts)?;
    println!(
        "{}",
        submit_unprivileged_operation(
            api_url,
            token,
            &operation,
            "storage_inventory",
            &clients,
            &tags,
            max_timeout_secs,
        )?
    );
    Ok(())
}

pub(crate) fn host_storage(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    limit: u16,
) -> Result<()> {
    anyhow::ensure!(
        (1..=2048).contains(&limit),
        "host storage limit must be between 1 and 2048"
    );
    let client_id = client_id.trim();
    anyhow::ensure!(!client_id.is_empty(), "host storage client ID is required");
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/host-storage/{}?limit={limit}",
                percent_encode_path_segment(client_id)
            ),
            token,
        )?
    );
    Ok(())
}

fn storage_inventory_operation(limit: u16, include_system_mounts: bool) -> Result<JobCommand> {
    anyhow::ensure!(
        (1..=2048).contains(&limit),
        "storage inventory limit must be between 1 and 2048"
    );
    Ok(JobCommand::StorageInventory {
        include_pseudo_mounts: include_system_mounts,
        limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_inventory_is_read_only_and_preserves_explicit_mount_scope() {
        match storage_inventory_operation(2048, true).unwrap() {
            JobCommand::StorageInventory {
                include_pseudo_mounts: true,
                limit: 2048,
            } => {}
            other => panic!("unexpected operation: {other:?}"),
        }
        assert!(storage_inventory_operation(0, false).is_err());
    }
}
