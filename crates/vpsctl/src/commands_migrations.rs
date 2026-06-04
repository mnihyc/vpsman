use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::{
    commands_backups::restore_run_with_credentials,
    http::{http_get, http_post_json},
    proof::{load_super_password, load_super_salt_hex},
};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RestorePlanRecord {
    pub(crate) id: Uuid,
    pub(crate) source_backup_request_id: Uuid,
    pub(crate) target_client_id: String,
    pub(crate) paths: Vec<String>,
    pub(crate) include_config: bool,
    pub(crate) destination_root: Option<String>,
    pub(crate) status: String,
}

pub(crate) fn migration_links(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/migration-links?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn migration_link(
    api_url: &str,
    token: Option<&str>,
    restore_plan_id: String,
    note: Option<String>,
    confirmed: bool,
) -> Result<()> {
    let restore_plan_id = Uuid::parse_str(&restore_plan_id).context("invalid restore plan UUID")?;
    anyhow::ensure!(confirmed, "migration-link requires --confirmed");
    let body = json!({
        "restore_plan_id": restore_plan_id,
        "confirmed": confirmed,
        "note": note,
    });
    println!(
        "{}",
        http_post_json(api_url, "/api/v1/migration-links", token, &body)?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn migration_run(
    api_url: &str,
    token: Option<&str>,
    restore_plan_id: String,
    artifact_file: Option<PathBuf>,
    private_key_env: String,
    note: Option<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    let restore_plan_id = Uuid::parse_str(&restore_plan_id).context("invalid restore plan UUID")?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let response = migration_run_with_credentials(
        api_url,
        token,
        restore_plan_id,
        artifact_file,
        private_key_env,
        note,
        &password,
        &salt_hex,
        proof_ttl_secs,
        timeout_secs,
        confirmed,
        force_unprivileged,
    )?;
    println!("{response}");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn migration_run_with_credentials(
    api_url: &str,
    token: Option<&str>,
    restore_plan_id: Uuid,
    artifact_file: Option<PathBuf>,
    private_key_env: String,
    note: Option<String>,
    password: &str,
    salt_hex: &str,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<String> {
    anyhow::ensure!(confirmed, "migration-run requires --confirmed");
    let plan = find_restore_plan(api_url, token, restore_plan_id)?;
    anyhow::ensure!(
        plan.status == "planned_metadata_only",
        "migration-run requires a planned_metadata_only restore plan"
    );
    let migration_link_json = http_post_json(
        api_url,
        "/api/v1/migration-links",
        token,
        &json!({
            "restore_plan_id": restore_plan_id,
            "confirmed": true,
            "note": note,
        }),
    )?;
    let target_client_id = plan.target_client_id.clone();
    let restore_job_json = restore_run_with_credentials(
        api_url,
        token,
        plan.source_backup_request_id,
        plan.target_client_id,
        artifact_file,
        private_key_env,
        plan.paths,
        plan.include_config,
        plan.destination_root,
        password,
        salt_hex,
        proof_ttl_secs,
        timeout_secs,
        true,
        force_unprivileged,
    )?;
    Ok(json!({
        "migration_link": parse_json_or_text(&migration_link_json),
        "restore_job": parse_json_or_text(&restore_job_json),
        "restore_plan_id": restore_plan_id,
        "source_backup_request_id": plan.source_backup_request_id,
        "target_client_id": target_client_id,
    })
    .to_string())
}

fn find_restore_plan(
    api_url: &str,
    token: Option<&str>,
    restore_plan_id: Uuid,
) -> Result<RestorePlanRecord> {
    let body = http_get(api_url, "/api/v1/restore-plans?limit=200", token)?;
    let plans: Vec<RestorePlanRecord> =
        serde_json::from_str(&body).context("invalid restore plans JSON")?;
    plans
        .into_iter()
        .find(|plan| plan.id == restore_plan_id)
        .context("restore plan was not found in the latest 200 restore plans")
}

fn parse_json_or_text(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or_else(|_| json!({ "text": value }))
}
