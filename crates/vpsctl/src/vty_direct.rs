use anyhow::Result;

use crate::http::{http_delete, http_get, http_post_json};

pub(crate) fn submit_vty_direct_command(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<Option<String>> {
    match command {
        "health" => Ok(Some(http_get(api_url, "/health", None)?)),
        "summary" => Ok(Some(http_get(api_url, "/api/v1/fleet/summary", token)?)),
        "agents" => Ok(Some(http_get(api_url, "/api/v1/agents", token)?)),
        "operators" => Ok(Some(http_get(api_url, "/api/v1/operators", token)?)),
        "client-key-revocations" => Ok(Some(http_get(
            api_url,
            "/api/v1/client-key-revocations?limit=50",
            token,
        )?)),
        "key-lifecycle-report" => Ok(Some(http_get(
            api_url,
            "/api/v1/key-lifecycle/report",
            token,
        )?)),
        "operator-sessions" => Ok(Some(http_get(
            api_url,
            "/api/v1/operator-sessions?limit=50",
            token,
        )?)),
        "tags" => Ok(Some(http_get(api_url, "/api/v1/tags", token)?)),
        "jobs" => Ok(Some(http_get(api_url, "/api/v1/jobs", token)?)),
        "server-jobs" => Ok(Some(http_get(
            api_url,
            "/api/v1/server-jobs?limit=50",
            token,
        )?)),
        "schedules" => Ok(Some(http_get(api_url, "/api/v1/schedules", token)?)),
        "audit" => Ok(Some(http_get(api_url, "/api/v1/audit", token)?)),
        "history-retention" => Ok(Some(http_get(
            api_url,
            "/api/v1/history/retention-policies",
            token,
        )?)),
        "backups" => Ok(Some(http_get(api_url, "/api/v1/backups", token)?)),
        "backup-artifacts" => Ok(Some(http_get(api_url, "/api/v1/backup-artifacts", token)?)),
        "backup-policies" => Ok(Some(http_get(api_url, "/api/v1/backup-policies", token)?)),
        "restore-plans" => Ok(Some(http_get(api_url, "/api/v1/restore-plans", token)?)),
        "migration-links" => Ok(Some(http_get(api_url, "/api/v1/migration-links", token)?)),
        "tunnel-plans" => Ok(Some(http_get(api_url, "/api/v1/tunnel-plans", token)?)),
        command if command.starts_with("agent-identity-upsert ") => {
            Ok(Some(submit_agent_identity_upsert(api_url, token, command)?))
        }
        command if command.starts_with("client-key-revoke ") => {
            Ok(Some(submit_client_key_revoke(api_url, token, command)?))
        }
        command if command.starts_with("operator-create ") => {
            Ok(Some(submit_operator_create(api_url, token, command)?))
        }
        command if command.starts_with("operator-sessions ") => {
            Ok(Some(submit_operator_sessions(api_url, token, command)?))
        }
        command if command.starts_with("operator-session-revoke ") => Ok(Some(
            submit_operator_session_revoke(api_url, token, command)?,
        )),
        command if command.starts_with("history-retention-upsert ") => Ok(Some(
            submit_history_retention_upsert(api_url, token, command)?,
        )),
        command if command.starts_with("history-retention-prune") => Ok(Some(
            submit_history_retention_prune(api_url, token, command)?,
        )),
        command if command.starts_with("server-jobs ") => {
            Ok(Some(submit_server_jobs(api_url, token, command)?))
        }
        command if command.starts_with("artifact-cleanup-preview ") => Ok(Some(
            submit_artifact_cleanup_preview(api_url, token, command)?,
        )),
        command if command.starts_with("artifact-cleanup-create ") => Ok(Some(
            submit_artifact_cleanup_create(api_url, token, command)?,
        )),
        command if command.starts_with("server-job-cancel ") => {
            Ok(Some(submit_server_job_cancel(api_url, token, command)?))
        }
        command if command.starts_with("backup-policy-prune") => {
            Ok(Some(submit_backup_policy_prune(api_url, token, command)?))
        }
        command if command.starts_with("history-export") => {
            Ok(Some(submit_history_export(api_url, token, command)?))
        }
        _ => Ok(None),
    }
}

fn submit_agent_identity_upsert(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let client_id = required_flag(&parts, "--client-id")?;
    let client_public_key_hex = required_flag(&parts, "--client-public-key-hex")?;
    let display_name = optional_flag(&parts, "--display-name");
    let tags = optional_flag(&parts, "--tags")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    http_post_json(
        api_url,
        "/api/v1/agent-identities",
        token,
        &serde_json::json!({
            "client_id": client_id,
            "client_public_key_hex": client_public_key_hex,
            "display_name": display_name,
            "tags": tags,
            "replace_existing_key": has_flag(&parts, "--replace-existing-key"),
            "confirmed": has_flag(&parts, "--confirmed"),
        }),
    )
}

fn submit_client_key_revoke(api_url: &str, token: Option<&str>, command: &str) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let client_id = required_flag(&parts, "--client-id")?;
    http_post_json(
        api_url,
        &format!("/api/v1/clients/{client_id}/key-revocations"),
        token,
        &serde_json::json!({
            "reason": optional_flag(&parts, "--reason"),
            "confirmed": has_flag(&parts, "--confirmed"),
        }),
    )
}

fn required_flag(parts: &[&str], name: &str) -> Result<String> {
    optional_flag(parts, name).ok_or_else(|| anyhow::anyhow!("missing required flag {name}"))
}

fn optional_flag(parts: &[&str], name: &str) -> Option<String> {
    parts
        .windows(2)
        .find_map(|pair| (pair[0] == name).then(|| pair[1].to_string()))
}

fn has_flag(parts: &[&str], name: &str) -> bool {
    parts.contains(&name)
}

fn submit_operator_create(api_url: &str, token: Option<&str>, command: &str) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    if !(4..=5).contains(&parts.len()) {
        return Ok(
            "usage: operator-create <username> <role> <password_env> [scope,scope]".to_string(),
        );
    }
    let password = match std::env::var(parts[3]) {
        Ok(password) => password,
        Err(error) => {
            return Ok(format!(
                "usage error: password env {} unavailable: {error}",
                parts[3]
            ));
        }
    };
    http_post_json(
        api_url,
        "/api/v1/operators",
        token,
        &serde_json::json!({
            "username": parts[1],
            "role": parts[2],
            "password": password,
            "scopes": parts.get(4).map(|scopes| scopes.split(',').filter(|scope| !scope.is_empty()).collect::<Vec<_>>()).unwrap_or_default(),
        }),
    )
}

fn submit_operator_sessions(api_url: &str, token: Option<&str>, command: &str) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    if parts.len() == 3 && parts[1] == "--limit" {
        http_get(
            api_url,
            &format!("/api/v1/operator-sessions?limit={}", parts[2]),
            token,
        )
    } else {
        Ok("usage: operator-sessions [--limit <1-200>]".to_string())
    }
}

fn submit_operator_session_revoke(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 2 {
        return Ok("usage: operator-session-revoke <session_uuid>".to_string());
    }
    http_delete(
        api_url,
        &format!("/api/v1/operator-sessions/{}", parts[1]),
        token,
    )
}

fn submit_history_retention_upsert(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let mut domain = None::<String>;
    let mut retention_days = None::<i32>;
    let mut prune_limit = None::<i32>;
    let mut enabled = None::<bool>;
    let mut metadata_only = None::<bool>;
    let mut export_enabled = None::<bool>;
    let mut notes = None::<String>;
    let mut clear_notes = false;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--domain" if index + 1 < parts.len() => {
                domain = Some(parts[index + 1].to_string());
                index += 2;
            }
            "--retention-days" if index + 1 < parts.len() => {
                retention_days = Some(parts[index + 1].parse()?);
                index += 2;
            }
            "--prune-limit" if index + 1 < parts.len() => {
                prune_limit = Some(parts[index + 1].parse()?);
                index += 2;
            }
            "--enabled" if index + 1 < parts.len() => {
                enabled = Some(parse_bool(parts[index + 1])?);
                index += 2;
            }
            "--metadata-only" if index + 1 < parts.len() => {
                metadata_only = Some(parse_bool(parts[index + 1])?);
                index += 2;
            }
            "--export-enabled" if index + 1 < parts.len() => {
                export_enabled = Some(parse_bool(parts[index + 1])?);
                index += 2;
            }
            "--notes" if index + 1 < parts.len() => {
                notes = Some(parts[index + 1].to_string());
                index += 2;
            }
            "--clear-notes" => {
                clear_notes = true;
                index += 1;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            _ => return Ok(history_retention_upsert_usage()),
        }
    }
    let Some(domain) = domain else {
        return Ok(history_retention_upsert_usage());
    };
    http_post_json(
        api_url,
        "/api/v1/history/retention-policies",
        token,
        &serde_json::json!({
            "domain": domain,
            "retention_days": retention_days,
            "prune_limit": prune_limit,
            "enabled": enabled,
            "metadata_only": metadata_only,
            "export_enabled": export_enabled,
            "notes": notes,
            "clear_notes": clear_notes,
            "confirmed": confirmed,
        }),
    )
}

fn submit_history_retention_prune(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let mut domain = None::<String>;
    let mut dry_run = false;
    let mut metadata_only = None::<bool>;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--domain" if index + 1 < parts.len() => {
                domain = Some(parts[index + 1].to_string());
                index += 2;
            }
            "--dry-run" => {
                dry_run = true;
                index += 1;
            }
            "--metadata-only" if index + 1 < parts.len() => {
                metadata_only = Some(parse_bool(parts[index + 1])?);
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            _ => return Ok(history_retention_prune_usage()),
        }
    }
    http_post_json(
        api_url,
        "/api/v1/history/retention-prune",
        token,
        &serde_json::json!({
            "domain": domain,
            "dry_run": dry_run,
            "metadata_only": metadata_only,
            "confirmed": confirmed,
        }),
    )
}

fn submit_server_jobs(api_url: &str, token: Option<&str>, command: &str) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let mut limit = 50_u16;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--limit" if index + 1 < parts.len() => {
                limit = parts[index + 1].parse::<u16>()?.clamp(1, 200);
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse::<u16>()?
                    .clamp(1, 200);
                index += 1;
            }
            _ => return Ok(server_jobs_usage()),
        }
    }
    http_get(
        api_url,
        &format!("/api/v1/server-jobs?limit={limit}"),
        token,
    )
}

fn submit_artifact_cleanup_preview(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let Some(expression) = collect_option_value(&parts, "--expression") else {
        return Ok(artifact_cleanup_preview_usage());
    };
    http_post_json(
        api_url,
        "/api/v1/server-jobs/artifact-cleanup/preview",
        token,
        &serde_json::json!({ "expression": expression }),
    )
}

fn submit_artifact_cleanup_create(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let Some(expression) = collect_option_value(&parts, "--expression") else {
        return Ok(artifact_cleanup_create_usage());
    };
    let Some(preview_hash) = option_value(&parts, "--preview-hash") else {
        return Ok(artifact_cleanup_create_usage());
    };
    http_post_json(
        api_url,
        "/api/v1/server-jobs/artifact-cleanup",
        token,
        &serde_json::json!({
            "expression": expression,
            "preview_hash": preview_hash,
            "confirmed": has_flag(&parts, "--confirmed"),
        }),
    )
}

fn submit_server_job_cancel(api_url: &str, token: Option<&str>, command: &str) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let Some(job_id) = option_value(&parts, "--job-id") else {
        return Ok(server_job_cancel_usage());
    };
    http_post_json(
        api_url,
        &format!("/api/v1/server-jobs/{job_id}/cancel"),
        token,
        &serde_json::json!({}),
    )
}

fn submit_backup_policy_prune(api_url: &str, token: Option<&str>, command: &str) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let mut schedule_id = None::<String>;
    let mut dry_run = false;
    let mut metadata_only = None::<bool>;
    let mut confirmed = false;
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--schedule-id" if index + 1 < parts.len() => {
                schedule_id = Some(parts[index + 1].to_string());
                index += 2;
            }
            "--dry-run" => {
                dry_run = true;
                index += 1;
            }
            "--metadata-only" if index + 1 < parts.len() => {
                metadata_only = Some(parse_bool(parts[index + 1])?);
                index += 2;
            }
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            _ => return Ok(backup_policy_prune_usage()),
        }
    }
    http_post_json(
        api_url,
        "/api/v1/backup-policies/prune",
        token,
        &serde_json::json!({
            "schedule_id": schedule_id,
            "dry_run": dry_run,
            "metadata_only": metadata_only,
            "confirmed": confirmed,
        }),
    )
}

fn submit_history_export(api_url: &str, token: Option<&str>, command: &str) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let mut params = vec!["limit=50".to_string()];
    let mut index = 1;
    while index < parts.len() {
        match parts[index] {
            "--domains" if index + 1 < parts.len() => {
                params.push(format!(
                    "domains={}",
                    percent_encode_query_value(parts[index + 1])
                ));
                index += 2;
            }
            "--limit" if index + 1 < parts.len() => {
                let limit = parts[index + 1].parse::<u16>()?.clamp(1, 200);
                params[0] = format!("limit={limit}");
                index += 2;
            }
            "--client-id" if index + 1 < parts.len() => {
                params.push(format!(
                    "client_id={}",
                    percent_encode_query_value(parts[index + 1])
                ));
                index += 2;
            }
            "--job-id" if index + 1 < parts.len() => {
                params.push(format!("job_id={}", parts[index + 1]));
                index += 2;
            }
            _ => {
                return Ok(
                    "usage: history-export [--domains audit_logs,job_outputs] [--limit <1-200>] [--client-id <id>] [--job-id <uuid>]"
                        .to_string(),
                )
            }
        }
    }
    http_get(
        api_url,
        &format!("/api/v1/history/export?{}", params.join("&")),
        token,
    )
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" => Ok(false),
        _ => anyhow::bail!("invalid boolean {value}"),
    }
}

fn option_value(parts: &[&str], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    let mut index = 1;
    while index < parts.len() {
        if parts[index] == name {
            return parts.get(index + 1).map(|value| (*value).to_string());
        }
        if parts[index].starts_with(&prefix) {
            return Some(parts[index].trim_start_matches(&prefix).to_string());
        }
        index += 1;
    }
    None
}

fn collect_option_value(parts: &[&str], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    let mut index = 1;
    while index < parts.len() {
        if parts[index].starts_with(&prefix) {
            return Some(parts[index].trim_start_matches(&prefix).to_string());
        }
        if parts[index] == name {
            let mut values = Vec::new();
            index += 1;
            while index < parts.len() && !parts[index].starts_with("--") {
                values.push(parts[index]);
                index += 1;
            }
            return (!values.is_empty()).then(|| values.join(" "));
        }
        index += 1;
    }
    None
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

fn history_retention_upsert_usage() -> String {
    "usage: history-retention-upsert --domain <domain> [--retention-days <1-3650>] [--prune-limit <1-100000>] [--enabled true|false] [--metadata-only true|false] [--export-enabled true|false] [--notes <text>|--clear-notes] --confirmed".to_string()
}

fn history_retention_prune_usage() -> String {
    "usage: history-retention-prune [--domain <domain>] [--dry-run] [--metadata-only true|false] [--confirmed]".to_string()
}

fn server_jobs_usage() -> String {
    "usage: server-jobs [--limit <1-200>]".to_string()
}

fn artifact_cleanup_preview_usage() -> String {
    "usage: artifact-cleanup-preview --expression <expr>".to_string()
}

fn artifact_cleanup_create_usage() -> String {
    "usage: artifact-cleanup-create --expression <expr> --preview-hash <sha256> --confirmed"
        .to_string()
}

fn server_job_cancel_usage() -> String {
    "usage: server-job-cancel --job-id <uuid>".to_string()
}

fn backup_policy_prune_usage() -> String {
    "usage: backup-policy-prune [--schedule-id <uuid>] [--dry-run] [--metadata-only true|false] [--confirmed]".to_string()
}
