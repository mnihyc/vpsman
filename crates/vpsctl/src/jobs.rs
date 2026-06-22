use anyhow::{Context, Result};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::{job_command_requires_confirmation, JobCommand};

use crate::{
    commands_schedules::selector_expression_from_targets,
    http::http_post_json,
    privilege::{build_privilege_for_job_command, load_super_password, load_super_salt_hex},
};

#[derive(Debug, Deserialize)]
struct BulkResolveResponse {
    targets: Vec<BulkTarget>,
}

#[derive(Debug, Deserialize)]
struct BulkTarget {
    id: String,
}

pub(crate) fn resolve_target_ids(
    api_url: &str,
    token: Option<&str>,
    clients: &[String],
    tags: &[String],
) -> Result<Vec<String>> {
    let body = http_post_json(
        api_url,
        "/api/v1/bulk/resolve",
        token,
        &serde_json::json!({
            "selector_expression": selector_expression_from_targets(clients, tags),
        }),
    )?;
    let response: BulkResolveResponse =
        serde_json::from_str(&body).context("failed to parse bulk target response")?;
    let target_ids = response
        .targets
        .into_iter()
        .map(|target| target.id)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !target_ids.is_empty(),
        "job-create resolved no targets; provide explicit clients or tags with online agents"
    );
    Ok(target_ids)
}

pub(crate) struct PrivilegedOperationRequest<'a> {
    pub(crate) api_url: &'a str,
    pub(crate) token: Option<&'a str>,
    pub(crate) operation: &'a JobCommand,
    pub(crate) command_label: &'a str,
    pub(crate) clients: &'a [String],
    pub(crate) tags: &'a [String],
    pub(crate) password_env: &'a str,
    pub(crate) super_salt_hex: Option<&'a str>,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) max_timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

pub(crate) fn submit_privileged_operation(
    request: PrivilegedOperationRequest<'_>,
) -> Result<String> {
    let password = load_super_password(request.password_env)?;
    let salt_hex = load_super_salt_hex(request.super_salt_hex)?;
    let selector_expression = selector_expression_from_targets(request.clients, request.tags);
    let target_ids = resolve_target_ids(
        request.api_url,
        request.token,
        request.clients,
        request.tags,
    )?;
    let privilege = build_privilege_for_job_command(
        &target_ids,
        request.operation,
        request.command_label,
        &selector_expression,
        &password,
        &salt_hex,
        request.privilege_ttl_secs,
        request.max_timeout_secs,
        request.force_unprivileged,
        true,
    )?;
    http_post_json(
        request.api_url,
        "/api/v1/jobs",
        request.token,
        &serde_json::json!({
            "job_id": Uuid::new_v4(),
            "command": request.command_label,
            "argv": [],
            "operation": request.operation,
            "selector_expression": selector_expression,
            "target_client_ids": target_ids,
            "privileged": true,
            "destructive": job_command_requires_confirmation(request.operation),
            "confirmed": request.confirmed,
            "force_unprivileged": request.force_unprivileged,
            "max_timeout_secs": request.max_timeout_secs,
            "privilege_assertion": privilege.privilege_assertion,
        }),
    )
}
