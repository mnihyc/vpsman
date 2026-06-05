use anyhow::{Context, Result};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::JobCommand;

use crate::{
    http::{http_get, http_post_json},
    proof::{build_envelopes_for_payload_hash, load_super_password, load_super_salt_hex},
};

#[derive(Debug, Deserialize)]
struct JobHistoryResponse {
    id: Uuid,
    status: String,
    payload_hash: String,
}

#[derive(Debug, Deserialize)]
struct JobTargetResponse {
    client_id: String,
    status: String,
}

pub(crate) fn schedules(api_url: &str, token: Option<&str>) -> Result<()> {
    println!("{}", http_get(api_url, "/api/v1/schedules", token)?);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn schedule_create(
    api_url: &str,
    token: Option<&str>,
    name: String,
    command: String,
    argv: Vec<String>,
    pty: bool,
    clients: Vec<String>,
    tags: Vec<String>,
    interval_secs: u64,
    start_at_unix: Option<u64>,
    disabled: bool,
    catch_up_policy: String,
    catch_up_limit: i32,
    retry_delay_secs: i64,
    max_failures: i32,
) -> Result<()> {
    validate_schedule_policy(
        &catch_up_policy,
        catch_up_limit,
        retry_delay_secs,
        max_failures,
    )?;
    let operation = JobCommand::Shell {
        argv: if argv.is_empty() {
            vec![command.clone()]
        } else {
            argv
        },
        pty,
    };
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/schedules",
            token,
            &serde_json::json!({
                "name": name,
                "operation": operation,
                "clients": clients,
                "tags": tags,
                "interval_secs": interval_secs,
                "start_at_unix": start_at_unix,
                "enabled": !disabled,
                "catch_up_policy": catch_up_policy,
                "catch_up_limit": catch_up_limit,
                "retry_delay_secs": retry_delay_secs,
                "max_failures": max_failures,
            }),
        )?
    );
    Ok(())
}

fn validate_schedule_policy(
    catch_up_policy: &str,
    catch_up_limit: i32,
    retry_delay_secs: i64,
    max_failures: i32,
) -> Result<()> {
    anyhow::ensure!(
        matches!(
            catch_up_policy,
            "skip_missed" | "run_once" | "run_all_limited"
        ),
        "--catch-up-policy must be skip_missed, run_once, or run_all_limited"
    );
    anyhow::ensure!(
        (1..=25).contains(&catch_up_limit),
        "--catch-up-limit must be between 1 and 25"
    );
    anyhow::ensure!(
        (1..=86_400).contains(&retry_delay_secs),
        "--retry-delay-secs must be between 1 and 86400"
    );
    anyhow::ensure!(
        (1..=100).contains(&max_failures),
        "--max-failures must be between 1 and 100"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn schedule_dispatch(
    api_url: &str,
    token: Option<&str>,
    job_id: String,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    force_unprivileged: bool,
    confirmed: bool,
) -> Result<()> {
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    println!(
        "{}",
        schedule_dispatch_with_material(
            api_url,
            token,
            &job_id,
            &password,
            &salt_hex,
            proof_ttl_secs,
            timeout_secs,
            force_unprivileged,
            confirmed,
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn schedule_dispatch_with_material(
    api_url: &str,
    token: Option<&str>,
    job_id: &str,
    password: &str,
    salt_hex: &str,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    force_unprivileged: bool,
    confirmed: bool,
) -> Result<String> {
    anyhow::ensure!(
        confirmed,
        "scheduled dispatch requires --confirmed because it executes a frozen privileged run"
    );
    let job_id = Uuid::parse_str(job_id).context("invalid scheduled --job-id UUID")?;
    let job: JobHistoryResponse = serde_json::from_str(&http_get(
        api_url,
        &format!("/api/v1/jobs/{job_id}"),
        token,
    )?)
    .context("failed to parse scheduled job response")?;
    anyhow::ensure!(job.id == job_id, "API returned mismatched scheduled job id");
    anyhow::ensure!(
        job.status == "approval_required",
        "scheduled job must be approval_required, got {}",
        job.status
    );
    let targets: Vec<JobTargetResponse> = serde_json::from_str(&http_get(
        api_url,
        &format!("/api/v1/jobs/{job_id}/targets"),
        token,
    )?)
    .context("failed to parse scheduled job targets")?;
    let client_ids = targets
        .into_iter()
        .filter(|target| target.status == "approval_required")
        .map(|target| target.client_id)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !client_ids.is_empty(),
        "scheduled job has no approval_required targets"
    );
    let envelopes = build_envelopes_for_payload_hash(
        &client_ids,
        &job.payload_hash,
        password,
        salt_hex,
        proof_ttl_secs,
    )?;

    http_post_json(
        api_url,
        &format!("/api/v1/jobs/{job_id}/dispatch-scheduled"),
        token,
        &serde_json::json!({
            "confirmed": confirmed,
            "timeout_secs": timeout_secs,
            "force_unprivileged": force_unprivileged,
            "envelope": null,
            "envelopes": envelopes,
        }),
    )
}
