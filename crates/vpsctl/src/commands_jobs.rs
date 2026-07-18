use std::{collections::BTreeSet, path::PathBuf, thread, time::Duration};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::{encode_json, payload_hash, JobCommand, JobRolloutPolicy, JobStatus};

use crate::{
    commands_schedules::selector_expression_from_targets,
    http::{http_get, http_get_to_file, http_post_json},
    jobs::{resolve_target_ids, submit_privileged_operation, PrivilegedOperationRequest},
    privilege::{
        build_privilege_for_job_command_with_rollout_hash, load_super_password, load_super_salt_hex,
    },
};

pub(crate) fn jobs(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/jobs?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn job_rollouts(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/job-rollouts?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn job_rollout(api_url: &str, token: Option<&str>, job_id: String) -> Result<()> {
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    println!(
        "{}",
        http_get(api_url, &format!("/api/v1/job-rollouts/{job_id}"), token)?
    );
    Ok(())
}

pub(crate) fn job_rollout_update(
    api_url: &str,
    token: Option<&str>,
    job_id: String,
    action: &str,
    confirmed: bool,
    reason: Option<String>,
) -> Result<()> {
    anyhow::ensure!(
        matches!(action, "pause" | "resume"),
        "invalid rollout action"
    );
    if action == "resume" {
        anyhow::ensure!(confirmed, "job-rollout-resume requires --confirmed");
    }
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/job-rollouts/{job_id}/{action}"),
            token,
            &serde_json::json!({
                "confirmed": confirmed,
                "reason": reason,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn job_cancel(
    api_url: &str,
    token: Option<&str>,
    job_id: String,
    reason: Option<String>,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "job-cancel requires --confirmed");
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/jobs/{job_id}/cancel"),
            token,
            &serde_json::json!({
                "confirmed": true,
                "reason": reason,
            }),
        )?
    );
    Ok(())
}

pub(crate) struct JobCreateOptions {
    pub(crate) command: String,
    pub(crate) argv: Vec<String>,
    pub(crate) pty: bool,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) password_env: String,
    pub(crate) super_salt_hex: Option<String>,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) max_timeout_secs: u64,
    pub(crate) privileged: bool,
    pub(crate) destructive: bool,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
    pub(crate) rollout_canary_clients: Vec<String>,
    pub(crate) rollout_batch_size: Option<u16>,
    pub(crate) rollout_max_failures: Option<u16>,
    pub(crate) rollout_batch_delay_secs: Option<u32>,
    pub(crate) rollout_continue_after_canary: bool,
}

pub(crate) fn job_create(
    api_url: &str,
    token: Option<&str>,
    options: JobCreateOptions,
) -> Result<()> {
    let effective_argv = if options.argv.is_empty() {
        vec![options.command.clone()]
    } else {
        options.argv.clone()
    };
    let operation = options.pty.then(|| JobCommand::Shell {
        argv: effective_argv.clone(),
        pty: true,
    });
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    let target_ids = resolve_target_ids(api_url, token, &options.clients, &options.tags)?;
    let rollout = build_job_rollout_policy(
        &options.rollout_canary_clients,
        options.rollout_batch_size,
        options.rollout_max_failures,
        options.rollout_batch_delay_secs,
        options.rollout_continue_after_canary,
        &target_ids,
    )?;
    let rollout_policy_hash = rollout
        .as_ref()
        .map(|policy| encode_json(policy).map(|payload| payload_hash(&payload)))
        .transpose()?;
    let privilege_assertion = if options.privileged {
        let password = load_super_password(&options.password_env)?;
        let salt_hex = load_super_salt_hex(options.super_salt_hex.as_deref())?;
        let assertion_command = if let Some(operation) = &operation {
            operation.clone()
        } else {
            JobCommand::Shell {
                argv: effective_argv.clone(),
                pty: false,
            }
        };
        Some(
            build_privilege_for_job_command_with_rollout_hash(
                &target_ids,
                &assertion_command,
                if operation.is_some() {
                    "shell_pty"
                } else {
                    "shell_argv"
                },
                &selector_expression,
                &password,
                &salt_hex,
                options.privilege_ttl_secs,
                options.max_timeout_secs,
                options.force_unprivileged,
                true,
                rollout_policy_hash.as_deref(),
            )?
            .privilege_assertion,
        )
    } else {
        None
    };
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/jobs",
            token,
            &serde_json::json!({
                "job_id": Uuid::new_v4(),
                "command": options.command,
                "argv": if operation.is_some() { Vec::<String>::new() } else { options.argv },
                "operation": operation,
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "privileged": options.privileged,
                "destructive": options.destructive,
                "confirmed": options.confirmed,
                "force_unprivileged": options.force_unprivileged,
                "max_timeout_secs": options.max_timeout_secs,
                "privilege_assertion": privilege_assertion,
                "rollout": rollout,
            }),
        )?
    );
    Ok(())
}

fn build_job_rollout_policy(
    canary_clients: &[String],
    batch_size: Option<u16>,
    max_failures: Option<u16>,
    batch_delay_secs: Option<u32>,
    continue_after_canary: bool,
    target_ids: &[String],
) -> Result<Option<JobRolloutPolicy>> {
    if canary_clients.is_empty() {
        anyhow::ensure!(
            batch_size.is_none()
                && max_failures.is_none()
                && batch_delay_secs.is_none()
                && !continue_after_canary,
            "rollout options require at least one explicit --rollout-canary"
        );
        return Ok(None);
    }
    anyhow::ensure!(
        target_ids.len() >= 2,
        "staged rollout requires at least two resolved clients"
    );
    let mut normalized_canaries = canary_clients
        .iter()
        .map(|client_id| client_id.trim().to_string())
        .collect::<Vec<_>>();
    anyhow::ensure!(
        normalized_canaries
            .iter()
            .all(|client_id| !client_id.is_empty()),
        "rollout canary client ID cannot be empty"
    );
    let original_count = normalized_canaries.len();
    normalized_canaries.sort();
    normalized_canaries.dedup();
    anyhow::ensure!(
        normalized_canaries.len() == original_count,
        "rollout canary client IDs must be unique"
    );
    anyhow::ensure!(
        normalized_canaries.len() <= 25,
        "staged rollout supports at most 25 canary clients"
    );
    anyhow::ensure!(
        normalized_canaries.len() < target_ids.len(),
        "rollout canaries must leave at least one client for a later batch"
    );
    for client_id in &normalized_canaries {
        anyhow::ensure!(
            target_ids.contains(client_id),
            "rollout canary {client_id} is not in the resolved target snapshot"
        );
    }
    let batch_size = batch_size.unwrap_or(5);
    let max_failures = max_failures.unwrap_or(0);
    let batch_delay_secs = batch_delay_secs.unwrap_or(0);
    anyhow::ensure!(
        (1..=100).contains(&batch_size),
        "rollout batch size must be between 1 and 100"
    );
    anyhow::ensure!(
        max_failures <= 100,
        "rollout tolerated failures must be between 0 and 100"
    );
    anyhow::ensure!(
        batch_delay_secs <= 86_400,
        "rollout batch delay must be between 0 and 86400 seconds"
    );
    Ok(Some(JobRolloutPolicy {
        canary_client_ids: normalized_canaries,
        batch_size,
        max_failures,
        pause_after_canary: !continue_after_canary,
        batch_delay_secs,
    }))
}

pub(crate) fn job_shell(
    api_url: &str,
    token: Option<&str>,
    script: Option<String>,
    script_file: Option<PathBuf>,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    max_timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    let script = load_shell_script(script, script_file)?;
    let operation = JobCommand::ShellScript { script };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "shell_script",
            clients: &clients,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            privilege_ttl_secs,
            max_timeout_secs,
            confirmed,
            force_unprivileged: false,
        })?
    );
    Ok(())
}

fn load_shell_script(script: Option<String>, script_file: Option<PathBuf>) -> Result<String> {
    match (script, script_file) {
        (Some(_), Some(_)) => anyhow::bail!("use either --script or --script-file, not both"),
        (Some(script), None) => {
            anyhow::ensure!(!script.trim().is_empty(), "--script is empty");
            Ok(script)
        }
        (None, Some(path)) => {
            let script = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read shell script {}", path.display()))?;
            anyhow::ensure!(!script.trim().is_empty(), "--script-file is empty");
            Ok(script)
        }
        (None, None) => anyhow::bail!("job-shell requires --script or --script-file"),
    }
}

pub(crate) fn job_targets(api_url: &str, token: Option<&str>, job_id: String) -> Result<()> {
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    println!(
        "{}",
        http_get(api_url, &format!("/api/v1/jobs/{job_id}/targets"), token)?
    );
    Ok(())
}

pub(crate) fn job_target_status_download(
    api_url: &str,
    token: Option<&str>,
    job_id: String,
    output_file: PathBuf,
) -> Result<()> {
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    let size_bytes = http_get_to_file(
        api_url,
        &format!("/api/v1/jobs/{job_id}/targets/download"),
        token,
        &output_file,
    )?;
    println!(
        "{}",
        serde_json::json!({
            "job_id": job_id,
            "output": output_file,
            "size_bytes": size_bytes,
        })
    );
    Ok(())
}

pub(crate) fn job_outputs(api_url: &str, token: Option<&str>, job_id: String) -> Result<()> {
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    let outputs = fetch_all_job_outputs(api_url, token, job_id, None, None)?;
    println!("{}", serde_json::to_string(&outputs_as_json(&outputs))?);
    Ok(())
}

pub(crate) fn job_follow(
    api_url: &str,
    token: Option<&str>,
    job_id: String,
    interval_ms: u64,
    max_polls: u32,
    json: bool,
) -> Result<()> {
    print!(
        "{}",
        job_follow_output(api_url, token, job_id, interval_ms, max_polls, json)?
    );
    Ok(())
}

pub(crate) fn job_follow_output(
    api_url: &str,
    token: Option<&str>,
    job_id: String,
    interval_ms: u64,
    max_polls: u32,
    json: bool,
) -> Result<String> {
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    let interval = Duration::from_millis(interval_ms.clamp(100, 10_000));
    let max_polls = if max_polls == 0 {
        None
    } else {
        Some(max_polls.clamp(1, 100_000))
    };
    let mut seen = BTreeSet::new();
    let mut rendered = String::new();
    let mut cursor = None;

    let mut poll = 0_u32;
    loop {
        loop {
            let page =
                fetch_job_output_page(api_url, token, job_id, cursor.as_deref(), None, None)?;
            for output in &page.items {
                if seen.insert((output.client_id.clone(), output.seq)) {
                    rendered.push_str(&render_job_output(output, json)?);
                }
            }
            if let Some(next_cursor) = page.next_cursor {
                cursor = Some(next_cursor);
            }
            if !page.has_more {
                break;
            }
        }

        let job_json = http_get(api_url, &format!("/api/v1/jobs/{job_id}"), token)?;
        let job =
            serde_json::from_str::<JobHistoryRecord>(&job_json).context("failed to parse job")?;
        if JobStatus::parse(&job.status).is_some_and(JobStatus::is_terminal) {
            if json {
                rendered.push_str(
                    &serde_json::json!({
                        "event": "job_follow_complete",
                        "job_id": job.id,
                        "status": job.status,
                        "outputs": seen.len(),
                    })
                    .to_string(),
                );
                rendered.push('\n');
            } else {
                rendered.push_str(&format!(
                    "[job {}] status={} outputs={}\n",
                    job.id,
                    job.status,
                    seen.len()
                ));
            }
            return Ok(rendered);
        }
        poll = poll.saturating_add(1);
        if max_polls.is_some_and(|max_polls| poll >= max_polls) {
            anyhow::bail!(
                "job-follow exceeded max polls; last status was {}",
                job.status
            );
        }
        thread::sleep(interval);
    }
}

pub(crate) fn job_output_download(
    api_url: &str,
    token: Option<&str>,
    job_id: String,
    client_id: String,
    seq: i32,
    output_file: PathBuf,
) -> Result<()> {
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    anyhow::ensure!(seq >= 0, "--seq must be non-negative");
    let size_bytes = http_get_to_file(
        api_url,
        &format!(
            "/api/v1/jobs/{job_id}/outputs/{}/{seq}/download",
            percent_encode_path_segment(&client_id),
        ),
        token,
        &output_file,
    )?;
    println!(
        "{}",
        serde_json::json!({
            "job_id": job_id,
            "client_id": client_id,
            "seq": seq,
            "output": output_file,
            "size_bytes": size_bytes,
        })
    );
    Ok(())
}

pub(crate) fn server_jobs(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/server-jobs?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn artifact_cleanup_preview(
    api_url: &str,
    token: Option<&str>,
    expression: String,
    domains: Vec<String>,
) -> Result<()> {
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/server-jobs/artifact-cleanup/preview",
            token,
            &serde_json::json!({
                "expression": expression,
                "domains": domains,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn artifact_cleanup_create(
    api_url: &str,
    token: Option<&str>,
    expression: String,
    domains: Vec<String>,
    preview_hash: String,
    confirmed: bool,
) -> Result<()> {
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/server-jobs/artifact-cleanup",
            token,
            &serde_json::json!({
                "expression": expression,
                "domains": domains,
                "preview_hash": preview_hash,
                "confirmed": confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn server_job_cancel(
    api_url: &str,
    token: Option<&str>,
    job_id: String,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "server-job-cancel requires --confirmed");
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/server-jobs/{job_id}/cancel"),
            token,
            &serde_json::json!({ "confirmed": confirmed }),
        )?
    );
    Ok(())
}

#[derive(Debug, Deserialize)]
struct JobHistoryRecord {
    id: Uuid,
    status: String,
}

#[derive(Debug, Deserialize)]
struct JobOutputRecord {
    client_id: String,
    seq: i32,
    stream: String,
    data_base64: String,
    storage: Option<String>,
    artifact_object_key: Option<String>,
    artifact_sha256_hex: Option<String>,
    artifact_size_bytes: Option<i64>,
    done: bool,
}

#[derive(Debug, Deserialize)]
struct JobOutputListPage {
    items: Vec<JobOutputRecord>,
    next_cursor: Option<String>,
    has_more: bool,
}

fn fetch_all_job_outputs(
    api_url: &str,
    token: Option<&str>,
    job_id: Uuid,
    client_id: Option<&str>,
    stream: Option<&str>,
) -> Result<Vec<JobOutputRecord>> {
    let mut cursor = None;
    let mut outputs = Vec::new();
    loop {
        let page =
            fetch_job_output_page(api_url, token, job_id, cursor.as_deref(), client_id, stream)?;
        outputs.extend(page.items);
        if !page.has_more {
            break;
        }
        cursor = page.next_cursor;
        anyhow::ensure!(cursor.is_some(), "job output page omitted next cursor");
    }
    Ok(outputs)
}

fn fetch_job_output_page(
    api_url: &str,
    token: Option<&str>,
    job_id: Uuid,
    cursor: Option<&str>,
    client_id: Option<&str>,
    stream: Option<&str>,
) -> Result<JobOutputListPage> {
    let mut params = vec!["limit=1000".to_string(), "include_data=true".to_string()];
    if let Some(cursor) = cursor {
        params.push(format!("cursor={}", percent_encode_query_value(cursor)));
    }
    if let Some(client_id) = client_id {
        params.push(format!(
            "client_id={}",
            percent_encode_query_value(client_id)
        ));
    }
    if let Some(stream) = stream {
        params.push(format!("stream={}", percent_encode_query_value(stream)));
    }
    let outputs_json = http_get(
        api_url,
        &format!("/api/v1/jobs/{job_id}/outputs?{}", params.join("&")),
        token,
    )?;
    serde_json::from_str::<JobOutputListPage>(&outputs_json)
        .context("failed to parse job output page")
}

fn render_job_output(output: &JobOutputRecord, json: bool) -> Result<String> {
    if json {
        return Ok(serde_json::to_string(&output_as_json(output))? + "\n");
    }
    let bytes = BASE64
        .decode(&output.data_base64)
        .context("job output data is not valid base64")?;
    let text = String::from_utf8_lossy(&bytes);
    let done = if output.done { " done" } else { "" };
    let artifact_deleted = output.storage.as_deref() == Some("artifact_deleted");
    let deleted = if artifact_deleted {
        let size = output
            .artifact_size_bytes
            .map(|size| format!(" full_size={size}"))
            .unwrap_or_default();
        format!(" artifact_deleted preview_only{size}")
    } else {
        String::new()
    };
    Ok(format!(
        "[{} {} #{}{}{}] {}\n",
        output.client_id,
        output.stream,
        output.seq,
        done,
        deleted,
        text.trim_end_matches(['\r', '\n'])
    ))
}

fn outputs_as_json(outputs: &[JobOutputRecord]) -> Vec<serde_json::Value> {
    outputs.iter().map(output_as_json).collect()
}

fn output_as_json(output: &JobOutputRecord) -> serde_json::Value {
    serde_json::json!({
        "event": "job_output",
        "client_id": &output.client_id,
        "seq": output.seq,
        "stream": &output.stream,
        "data_base64": &output.data_base64,
        "storage": &output.storage,
        "artifact_object_key": &output.artifact_object_key,
        "artifact_sha256_hex": &output.artifact_sha256_hex,
        "artifact_size_bytes": output.artifact_size_bytes,
        "done": output.done,
    })
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn percent_encode_query_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b',') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub(crate) fn audit(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/audit?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn history_retention(api_url: &str, token: Option<&str>) -> Result<()> {
    println!(
        "{}",
        http_get(api_url, "/api/v1/history/retention-policies", token)?
    );
    Ok(())
}

pub(crate) struct HistoryRetentionUpsertOptions {
    pub(crate) domain: String,
    pub(crate) retention_days: Option<i32>,
    pub(crate) prune_limit: Option<i32>,
    pub(crate) enabled: Option<bool>,
    pub(crate) metadata_only: Option<bool>,
    pub(crate) export_enabled: Option<bool>,
    pub(crate) notes: Option<String>,
    pub(crate) clear_notes: bool,
    pub(crate) confirmed: bool,
}

pub(crate) fn history_retention_upsert(
    api_url: &str,
    token: Option<&str>,
    options: HistoryRetentionUpsertOptions,
) -> Result<()> {
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/history/retention-policies",
            token,
            &serde_json::json!({
                "domain": options.domain,
                "retention_days": options.retention_days,
                "prune_limit": options.prune_limit,
                "enabled": options.enabled,
                "metadata_only": options.metadata_only,
                "export_enabled": options.export_enabled,
                "notes": options.notes,
                "clear_notes": options.clear_notes,
                "confirmed": options.confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) struct HistoryRetentionPruneOptions {
    pub(crate) domain: Option<String>,
    pub(crate) dry_run: bool,
    pub(crate) metadata_only: Option<bool>,
    pub(crate) preview_hash: Option<String>,
    pub(crate) confirmed: bool,
}

pub(crate) fn history_retention_prune(
    api_url: &str,
    token: Option<&str>,
    options: HistoryRetentionPruneOptions,
) -> Result<()> {
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/history/retention-prune",
            token,
            &serde_json::json!({
                "domain": options.domain,
                "dry_run": options.dry_run,
                "metadata_only": options.metadata_only,
                "preview_hash": options.preview_hash,
                "confirmed": options.confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn history_export(
    api_url: &str,
    token: Option<&str>,
    domains: Option<String>,
    limit: u16,
    client_id: Option<String>,
    job_id: Option<String>,
) -> Result<()> {
    if let Some(job_id) = job_id.as_deref() {
        Uuid::parse_str(job_id).context("invalid --job-id UUID")?;
    }
    let mut params = vec![format!("limit={}", limit.clamp(1, 200))];
    if let Some(domains) = domains {
        params.push(format!("domains={}", percent_encode_query_value(&domains)));
    }
    if let Some(client_id) = client_id {
        params.push(format!(
            "client_id={}",
            percent_encode_query_value(&client_id)
        ));
    }
    if let Some(job_id) = job_id {
        params.push(format!("job_id={job_id}"));
    }
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/history/export?{}", params.join("&")),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn network_observations(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/network/observations?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn network_trends(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/network/observation-trends?limit={}",
                limit.clamp(1, 200)
            ),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn network_ospf_recommendations(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/network/ospf-recommendations?limit={}",
                limit.clamp(1, 200)
            ),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn network_ospf_update_plans(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/network/ospf-update-plans?limit={}",
                limit.clamp(1, 200)
            ),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn topology_graph(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/network/topology-graph?limit={}",
                limit.clamp(1, 200)
            ),
            token,
        )?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_followed_job_output_as_text_and_json() {
        let output = JobOutputRecord {
            client_id: "edge-a".to_string(),
            seq: 7,
            stream: "pty".to_string(),
            storage: None,
            artifact_object_key: None,
            artifact_sha256_hex: None,
            artifact_size_bytes: None,
            data_base64: BASE64.encode("hello\r\n"),
            done: true,
        };

        let text = render_job_output(&output, false).unwrap();
        assert_eq!(text, "[edge-a pty #7 done] hello\n");

        let json = render_job_output(&output, true).unwrap();
        let value = serde_json::from_str::<serde_json::Value>(&json).unwrap();
        assert_eq!(value["event"], "job_output");
        assert_eq!(value["client_id"], "edge-a");
        assert_eq!(value["stream"], "pty");
        assert_eq!(value["done"], true);
    }

    #[test]
    fn job_follow_uses_common_terminal_statuses() {
        for status in vpsman_common::job_terminal_statuses() {
            assert!(JobStatus::parse(status).is_some_and(JobStatus::is_terminal));
        }
        for status in vpsman_common::job_statuses()
            .iter()
            .filter(|status| !vpsman_common::job_terminal_statuses().contains(status))
        {
            assert!(!JobStatus::parse(status).is_some_and(JobStatus::is_terminal));
        }
    }

    #[test]
    fn staged_rollout_requires_explicit_resolved_canaries() {
        let targets = vec!["client-a".to_string(), "client-b".to_string()];
        let policy =
            build_job_rollout_policy(&["client-a".to_string()], None, None, None, false, &targets)
                .unwrap()
                .unwrap();
        assert_eq!(policy.canary_client_ids, vec!["client-a"]);
        assert_eq!(policy.batch_size, 5);
        assert_eq!(policy.max_failures, 0);
        assert!(policy.pause_after_canary);

        assert!(build_job_rollout_policy(
            &["client-missing".to_string()],
            Some(1),
            Some(0),
            Some(0),
            false,
            &targets,
        )
        .unwrap_err()
        .to_string()
        .contains("not in the resolved target snapshot"));
    }

    #[test]
    fn rollout_modifiers_without_canary_are_rejected() {
        let targets = vec!["client-a".to_string(), "client-b".to_string()];
        assert!(
            build_job_rollout_policy(&[], Some(10), None, None, false, &targets)
                .unwrap_err()
                .to_string()
                .contains("explicit --rollout-canary")
        );
        assert!(
            build_job_rollout_policy(&[], None, None, None, false, &targets)
                .unwrap()
                .is_none()
        );
    }
}
