use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;
use vpsman_common::payload_hash;

use crate::{
    commands_backups::{restore_run_request_with_credentials, RestoreRunWithCredentials},
    http::{http_get, http_post_json},
    privilege::{
        build_privilege_for_db, load_super_password, load_super_salt_hex, DbPrivilegeRequest,
    },
};

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RestorePlanRecord {
    pub(crate) id: Uuid,
    pub(crate) source_backup_request_id: Uuid,
    pub(crate) source_client_id: String,
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
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    confirmed: bool,
) -> Result<()> {
    let restore_plan_id = Uuid::parse_str(&restore_plan_id).context("invalid restore plan UUID")?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    println!(
        "{}",
        migration_link_with_credentials(
            api_url,
            token,
            restore_plan_id,
            note,
            &password,
            &salt_hex,
            privilege_ttl_secs,
            confirmed,
        )?
    );
    Ok(())
}

pub(crate) fn migration_link_with_credentials(
    api_url: &str,
    token: Option<&str>,
    restore_plan_id: Uuid,
    note: Option<String>,
    password: &str,
    salt_hex: &str,
    privilege_ttl_secs: u64,
    confirmed: bool,
) -> Result<String> {
    anyhow::ensure!(confirmed, "migration-link requires --confirmed");
    let plan = find_restore_plan(api_url, token, restore_plan_id)?;
    anyhow::ensure!(
        plan.status == "planned_metadata_only",
        "migration-link requires a planned_metadata_only restore plan"
    );
    let body = migration_link_request_body(
        &plan,
        note,
        password,
        salt_hex,
        privilege_ttl_secs,
        confirmed,
    )?;
    http_post_json(api_url, "/api/v1/migration-links", token, &body)
}

pub(crate) fn migration_run(
    api_url: &str,
    token: Option<&str>,
    restore_plan_id: String,
    archive_transfer_session_id: String,
    note: Option<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    max_timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    let restore_plan_id = Uuid::parse_str(&restore_plan_id).context("invalid restore plan UUID")?;
    let archive_transfer_session_id = Uuid::parse_str(&archive_transfer_session_id)
        .context("invalid archive transfer session UUID")?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let response = migration_run_with_credentials(
        api_url,
        token,
        restore_plan_id,
        archive_transfer_session_id,
        note,
        &password,
        &salt_hex,
        privilege_ttl_secs,
        max_timeout_secs,
        confirmed,
        force_unprivileged,
    )?;
    println!("{response}");
    Ok(())
}

pub(crate) fn migration_run_with_credentials(
    api_url: &str,
    token: Option<&str>,
    restore_plan_id: Uuid,
    archive_transfer_session_id: Uuid,
    note: Option<String>,
    password: &str,
    salt_hex: &str,
    privilege_ttl_secs: u64,
    max_timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<String> {
    anyhow::ensure!(confirmed, "migration-run requires --confirmed");
    let plan = find_restore_plan(api_url, token, restore_plan_id)?;
    anyhow::ensure!(
        plan.status == "planned_metadata_only",
        "migration-run requires a planned_metadata_only restore plan"
    );
    let link_body = migration_link_request_body(
        &plan,
        note,
        password,
        salt_hex,
        privilege_ttl_secs,
        confirmed,
    )?;
    let target_client_id = plan.target_client_id.clone();
    let restore_job_body = restore_run_request_with_credentials(
        api_url,
        token,
        RestoreRunWithCredentials {
            source_backup_request_id: plan.source_backup_request_id,
            target_client_id: plan.target_client_id,
            archive_transfer_session_id,
            paths: plan.paths,
            include_config: plan.include_config,
            destination_root: plan.destination_root,
            password,
            salt_hex,
            privilege_ttl_secs,
            max_timeout_secs,
            confirmed: true,
            force_unprivileged,
        },
    )?;
    let migration_run_json = http_post_json(
        api_url,
        "/api/v1/migration-runs",
        token,
        &json!({
            "link": link_body,
            "job": restore_job_body,
        }),
    )?;
    Ok(json!({
        "migration_run": parse_json_or_text(&migration_run_json),
        "restore_plan_id": restore_plan_id,
        "source_backup_request_id": plan.source_backup_request_id,
        "target_client_id": target_client_id,
    })
    .to_string())
}

fn migration_link_request_body(
    plan: &RestorePlanRecord,
    note: Option<String>,
    password: &str,
    salt_hex: &str,
    privilege_ttl_secs: u64,
    confirmed: bool,
) -> Result<serde_json::Value> {
    let payload = migration_link_payload(plan, note.as_ref());
    let payload_bytes =
        serde_json::to_vec(&payload).context("failed to encode migration-link payload")?;
    let payload_hash = payload_hash(&payload_bytes);
    let target = plan.id.to_string();
    let resolved_targets = vec![plan.source_client_id.clone(), plan.target_client_id.clone()];
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: "migration.link",
            target: &target,
            selector_expression: None,
            resolved_targets: &resolved_targets,
            confirmed,
            payload_hash: Some(&payload_hash),
        },
        password,
        salt_hex,
        privilege_ttl_secs,
    )?;
    Ok(json!({
        "restore_plan_id": plan.id,
        "confirmed": confirmed,
        "note": note,
        "privilege_assertion": privilege_assertion,
    }))
}

fn migration_link_payload(plan: &RestorePlanRecord, note: Option<&String>) -> serde_json::Value {
    json!({
        "version": 1,
        "restore_plan_id": plan.id,
        "source_backup_request_id": plan.source_backup_request_id,
        "source_client_id": plan.source_client_id,
        "target_client_id": plan.target_client_id,
        "paths": plan.paths,
        "include_config": plan.include_config,
        "destination_root": plan.destination_root,
        "note": note,
    })
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
