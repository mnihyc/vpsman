use std::{collections::BTreeSet, path::PathBuf, thread, time::Duration};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::JobCommand;

use crate::{
    commands_schedules::selector_expression_from_targets,
    http::{http_get, http_get_bytes, http_post_json},
    jobs::{resolve_target_ids, submit_privileged_operation, PrivilegedOperationRequest},
    privilege::{build_privilege_for_job_command, load_super_password, load_super_salt_hex},
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn job_create(
    api_url: &str,
    token: Option<&str>,
    command: String,
    argv: Vec<String>,
    pty: bool,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    privileged: bool,
    destructive: bool,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    let effective_argv = if argv.is_empty() {
        vec![command.clone()]
    } else {
        argv.clone()
    };
    let operation = pty.then(|| JobCommand::Shell {
        argv: effective_argv.clone(),
        pty: true,
    });
    let selector_expression = selector_expression_from_targets(&clients, &tags);
    let privilege_assertion = if privileged {
        let password = load_super_password(&password_env)?;
        let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
        let target_ids = resolve_target_ids(api_url, token, &clients, &tags)?;
        let assertion_command = if let Some(operation) = &operation {
            operation.clone()
        } else {
            JobCommand::Shell {
                argv: effective_argv.clone(),
                pty: false,
            }
        };
        Some(
            build_privilege_for_job_command(
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
                privilege_ttl_secs,
                timeout_secs,
                force_unprivileged,
                true,
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
                "command": command,
                "argv": if operation.is_some() { Vec::<String>::new() } else { argv },
                "operation": operation,
                "selector_expression": selector_expression,
                "privileged": privileged,
                "destructive": destructive,
                "confirmed": confirmed,
                "force_unprivileged": force_unprivileged,
                "timeout_secs": timeout_secs,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
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
    timeout_secs: u64,
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
            timeout_secs,
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

pub(crate) fn job_outputs(api_url: &str, token: Option<&str>, job_id: String) -> Result<()> {
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    println!(
        "{}",
        http_get(api_url, &format!("/api/v1/jobs/{job_id}/outputs"), token)?
    );
    Ok(())
}

pub(crate) fn job_follow(
    api_url: &str,
    token: Option<&str>,
    job_id: String,
    interval_ms: u64,
    max_polls: u16,
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
    max_polls: u16,
    json: bool,
) -> Result<String> {
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    let interval = Duration::from_millis(interval_ms.clamp(100, 10_000));
    let max_polls = max_polls.clamp(1, 10_000);
    let mut seen = BTreeSet::new();
    let mut rendered = String::new();
    let mut last_status = None;

    for poll in 0..max_polls {
        let outputs_json = http_get(api_url, &format!("/api/v1/jobs/{job_id}/outputs"), token)?;
        let mut outputs = serde_json::from_str::<Vec<JobOutputRecord>>(&outputs_json)
            .context("failed to parse job outputs")?;
        outputs.sort_by(|left, right| {
            left.client_id
                .cmp(&right.client_id)
                .then_with(|| left.seq.cmp(&right.seq))
        });
        for output in &outputs {
            if seen.insert((output.client_id.clone(), output.seq)) {
                rendered.push_str(&render_job_output(output, json)?);
            }
        }

        let job_json = http_get(api_url, &format!("/api/v1/jobs/{job_id}"), token)?;
        let job =
            serde_json::from_str::<JobHistoryRecord>(&job_json).context("failed to parse job")?;
        last_status = Some(job.status.clone());
        if is_terminal_job_status(&job.status) {
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
        if poll + 1 < max_polls {
            thread::sleep(interval);
        }
    }
    anyhow::bail!(
        "job-follow exceeded max polls; last status was {}",
        last_status.unwrap_or_else(|| "unknown".to_string())
    );
}

pub(crate) fn job_output_artifact(
    api_url: &str,
    token: Option<&str>,
    job_id: String,
    client_id: String,
    seq: i32,
    output_file: PathBuf,
) -> Result<()> {
    let job_id = Uuid::parse_str(&job_id).context("invalid --job-id UUID")?;
    anyhow::ensure!(seq >= 0, "--seq must be non-negative");
    let bytes = http_get_bytes(
        api_url,
        &format!(
            "/api/v1/jobs/{job_id}/outputs/{}/{seq}/artifact",
            percent_encode_path_segment(&client_id),
        ),
        token,
    )?;
    std::fs::write(&output_file, &bytes)
        .with_context(|| format!("failed to write artifact {}", output_file.display()))?;
    println!(
        "{}",
        serde_json::json!({
            "job_id": job_id,
            "client_id": client_id,
            "seq": seq,
            "output": output_file,
            "size_bytes": bytes.len(),
        })
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
    done: bool,
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
    Ok(format!(
        "[{} {} #{}{}] {}\n",
        output.client_id,
        output.stream,
        output.seq,
        done,
        text.trim_end_matches(['\r', '\n'])
    ))
}

fn output_as_json(output: &JobOutputRecord) -> serde_json::Value {
    serde_json::json!({
        "event": "job_output",
        "client_id": &output.client_id,
        "seq": output.seq,
        "stream": &output.stream,
        "data_base64": &output.data_base64,
        "done": output.done,
    })
}

fn is_terminal_job_status(status: &str) -> bool {
    matches!(
        status,
        "completed"
            | "partially_completed"
            | "failed"
            | "timed_out"
            | "dispatch_failed"
            | "degraded_unprivileged"
            | "accepted"
            | "rejected_authorization_required"
            | "schedule_no_targets"
            | "rejected_by_agent"
    )
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
    fn classifies_terminal_follow_statuses() {
        for status in [
            "completed",
            "partially_completed",
            "failed",
            "timed_out",
            "dispatch_failed",
            "degraded_unprivileged",
            "accepted",
            "rejected_authorization_required",
            "schedule_no_targets",
            "rejected_by_agent",
        ] {
            assert!(is_terminal_job_status(status));
        }
        for status in ["dispatching", "running"] {
            assert!(!is_terminal_job_status(status));
        }
    }
}
