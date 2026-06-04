use anyhow::{Context, Result};
use serde::Deserialize;
use vpsman_common::JobCommand;

use crate::{
    http::http_post_json,
    proof::{build_envelopes_for_job_command, load_super_password, load_super_salt_hex},
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
    pools: &[String],
    tags: &[String],
    destructive: bool,
    confirmed: bool,
) -> Result<Vec<String>> {
    let body = http_post_json(
        api_url,
        "/api/v1/bulk/resolve",
        token,
        &serde_json::json!({
            "clients": clients,
            "pools": pools,
            "tags": tags,
            "destructive": destructive,
            "confirmed": confirmed,
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
        "job-create resolved no targets; provide explicit clients, pools, or tags with connected agents"
    );
    Ok(target_ids)
}

pub(crate) struct PrivilegedOperationRequest<'a> {
    pub(crate) api_url: &'a str,
    pub(crate) token: Option<&'a str>,
    pub(crate) operation: &'a JobCommand,
    pub(crate) command_label: &'a str,
    pub(crate) clients: &'a [String],
    pub(crate) pools: &'a [String],
    pub(crate) tags: &'a [String],
    pub(crate) password_env: &'a str,
    pub(crate) super_salt_hex: Option<&'a str>,
    pub(crate) proof_ttl_secs: u64,
    pub(crate) timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

pub(crate) fn submit_privileged_operation(
    request: PrivilegedOperationRequest<'_>,
) -> Result<String> {
    let password = load_super_password(request.password_env)?;
    let salt_hex = load_super_salt_hex(request.super_salt_hex)?;
    let target_ids = resolve_target_ids(
        request.api_url,
        request.token,
        request.clients,
        request.pools,
        request.tags,
        false,
        request.confirmed,
    )?;
    let (_payload_hash_hex, envelopes) = build_envelopes_for_job_command(
        &target_ids,
        request.operation,
        &password,
        &salt_hex,
        request.proof_ttl_secs,
    )?;
    http_post_json(
        request.api_url,
        "/api/v1/jobs",
        request.token,
        &serde_json::json!({
            "command": request.command_label,
            "argv": [],
            "operation": request.operation,
            "clients": request.clients,
            "pools": request.pools,
            "tags": request.tags,
            "privileged": true,
            "destructive": false,
            "confirmed": request.confirmed,
            "force_unprivileged": request.force_unprivileged,
            "timeout_secs": request.timeout_secs,
            "envelope": null,
            "envelopes": envelopes,
        }),
    )
}
