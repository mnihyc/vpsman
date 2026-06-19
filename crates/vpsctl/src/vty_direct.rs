use anyhow::Result;
use vpsman_common::{operator_db_payload_hash, OperatorDbPayloadInput};

use crate::http::{http_get, http_post_json, http_put_json};
use crate::privilege::{build_privilege_for_db, DbPrivilegeRequest};
use crate::vty_jobs::VtyPrivilegeContext;

pub(crate) fn submit_vty_direct_command(
    api_url: &str,
    token: Option<&str>,
    command: &str,
    privilege_context: &VtyPrivilegeContext,
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
        "operator-auth-events" => Ok(Some(http_get(
            api_url,
            "/api/v1/operator-auth-events?limit=50",
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
        command if command.starts_with("agent-identity-upsert ") => Ok(Some(
            submit_agent_identity_upsert(api_url, token, command, privilege_context)?,
        )),
        command if command.starts_with("client-key-revoke ") => Ok(Some(submit_client_key_revoke(
            api_url,
            token,
            command,
            privilege_context,
        )?)),
        command if command.starts_with("operator-create ") => Ok(Some(submit_operator_create(
            api_url,
            token,
            command,
            privilege_context,
        )?)),
        command if command.starts_with("operator-update ") => Ok(Some(submit_operator_update(
            api_url,
            token,
            command,
            privilege_context,
        )?)),
        command if command.starts_with("operator-disable ") => Ok(Some(submit_operator_lifecycle(
            api_url,
            token,
            command,
            "disable",
            privilege_context,
        )?)),
        command if command.starts_with("operator-enable ") => Ok(Some(submit_operator_lifecycle(
            api_url,
            token,
            command,
            "enable",
            privilege_context,
        )?)),
        command if command.starts_with("operator-delete ") => Ok(Some(submit_operator_lifecycle(
            api_url,
            token,
            command,
            "delete",
            privilege_context,
        )?)),
        command if command.starts_with("operator-password-reset ") => Ok(Some(
            submit_operator_password_reset(api_url, token, command, privilege_context)?,
        )),
        command if command.starts_with("operator-totp-clear ") => Ok(Some(
            submit_operator_lifecycle(api_url, token, command, "totp-clear", privilege_context)?,
        )),
        command if command.starts_with("operator-sessions ") => {
            Ok(Some(submit_operator_sessions(api_url, token, command)?))
        }
        command if command.starts_with("operator-session-revoke ") => Ok(Some(
            submit_operator_session_revoke(api_url, token, command, privilege_context)?,
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
    privilege_context: &VtyPrivilegeContext,
) -> Result<String> {
    anyhow::ensure!(
        privilege_context.enabled,
        "enter privileged mode first with: enable"
    );
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let client_id = required_flag(&parts, "--client-id")?;
    let client_public_key_hex = required_flag(&parts, "--client-public-key-hex")?;
    let display_name = optional_flag(&parts, "--display-name");
    let replace_existing_key = has_flag(&parts, "--replace-existing-key");
    let confirmed = has_flag(&parts, "--confirmed");
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
    let targets = vec![client_id.clone()];
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: if replace_existing_key {
                "agent_identity.rotate"
            } else {
                "agent_identity.import"
            },
            target: &client_id,
            selector_expression: None,
            resolved_targets: &targets,
            confirmed,
            payload_hash: None,
        },
        &privilege_context.password,
        &privilege_context.salt_hex,
        300,
    )?;
    http_post_json(
        api_url,
        "/api/v1/agent-identities",
        token,
        &serde_json::json!({
            "client_id": client_id,
            "client_public_key_hex": client_public_key_hex,
            "display_name": display_name,
            "tags": tags,
            "replace_existing_key": replace_existing_key,
            "confirmed": confirmed,
            "privilege_assertion": privilege_assertion,
        }),
    )
}

fn submit_client_key_revoke(
    api_url: &str,
    token: Option<&str>,
    command: &str,
    privilege_context: &VtyPrivilegeContext,
) -> Result<String> {
    anyhow::ensure!(
        privilege_context.enabled,
        "enter privileged mode first with: enable"
    );
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let client_id = required_flag(&parts, "--client-id")?;
    let confirmed = has_flag(&parts, "--confirmed");
    let targets = vec![client_id.clone()];
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: "client_key.revoke",
            target: &client_id,
            selector_expression: None,
            resolved_targets: &targets,
            confirmed,
            payload_hash: None,
        },
        &privilege_context.password,
        &privilege_context.salt_hex,
        300,
    )?;
    http_post_json(
        api_url,
        &format!("/api/v1/clients/{client_id}/key-revocations"),
        token,
        &serde_json::json!({
            "reason": optional_flag(&parts, "--reason"),
            "confirmed": confirmed,
            "privilege_assertion": privilege_assertion,
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

fn submit_operator_create(
    api_url: &str,
    token: Option<&str>,
    command: &str,
    privilege_context: &VtyPrivilegeContext,
) -> Result<String> {
    anyhow::ensure!(
        privilege_context.enabled,
        "enter privileged mode first with: enable"
    );
    let parts = command.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 4 {
        return Ok(
            "usage: operator-create <username> <role> <password_env> [scope,scope] [--session-refresh-ttl-secs <secs>] [--admin-risk-acknowledged] --confirmed".to_string(),
        );
    }
    let confirmed = has_flag(&parts, "--confirmed");
    anyhow::ensure!(confirmed, "operator-create requires --confirmed");
    let password = match std::env::var(parts[3]) {
        Ok(password) => password,
        Err(error) => {
            return Ok(format!(
                "usage error: password env {} unavailable: {error}",
                parts[3]
            ));
        }
    };
    let scopes = parts
        .get(4)
        .filter(|value| !value.starts_with("--"))
        .map(|scopes| {
            scopes
                .split(',')
                .filter(|scope| !scope.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let session_refresh_ttl_secs = optional_flag(&parts, "--session-refresh-ttl-secs")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(31_536_000);
    let admin_risk_acknowledged = has_flag(&parts, "--admin-risk-acknowledged");
    let privilege_assertion = operator_management_privilege(
        privilege_context,
        "operator.create",
        parts[1],
        Some(parts[1]),
        Some(parts[2]),
        &scopes,
        Some(session_refresh_ttl_secs),
        None,
        admin_risk_acknowledged,
        confirmed,
    )?;
    http_post_json(
        api_url,
        "/api/v1/operators",
        token,
        &serde_json::json!({
            "username": parts[1],
            "role": parts[2],
            "password": password,
            "scopes": scopes,
            "session_refresh_ttl_secs": session_refresh_ttl_secs,
            "confirmed": confirmed,
            "admin_risk_acknowledged": admin_risk_acknowledged,
            "privilege_assertion": privilege_assertion,
        }),
    )
}

fn submit_operator_update(
    api_url: &str,
    token: Option<&str>,
    command: &str,
    privilege_context: &VtyPrivilegeContext,
) -> Result<String> {
    anyhow::ensure!(
        privilege_context.enabled,
        "enter privileged mode first with: enable"
    );
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let operator_id = required_flag(&parts, "--operator-id")?;
    let role = required_flag(&parts, "--role")?;
    let scopes = optional_flag(&parts, "--scopes")
        .map(|value| split_csv(&value))
        .unwrap_or_default();
    let session_refresh_ttl_secs = optional_flag(&parts, "--session-refresh-ttl-secs")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(31_536_000);
    let admin_risk_acknowledged = has_flag(&parts, "--admin-risk-acknowledged");
    let confirmed = has_flag(&parts, "--confirmed");
    anyhow::ensure!(confirmed, "operator-update requires --confirmed");
    let privilege_assertion = operator_management_privilege(
        privilege_context,
        "operator.update",
        &operator_id,
        None,
        Some(&role),
        &scopes,
        Some(session_refresh_ttl_secs),
        None,
        admin_risk_acknowledged,
        confirmed,
    )?;
    http_put_json(
        api_url,
        &format!("/api/v1/operators/{operator_id}"),
        token,
        &serde_json::json!({
            "role": role,
            "scopes": scopes,
            "session_refresh_ttl_secs": session_refresh_ttl_secs,
            "confirmed": confirmed,
            "admin_risk_acknowledged": admin_risk_acknowledged,
            "privilege_assertion": privilege_assertion,
        }),
    )
}

fn submit_operator_lifecycle(
    api_url: &str,
    token: Option<&str>,
    command: &str,
    action: &str,
    privilege_context: &VtyPrivilegeContext,
) -> Result<String> {
    anyhow::ensure!(
        privilege_context.enabled,
        "enter privileged mode first with: enable"
    );
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let operator_id = required_flag(&parts, "--operator-id").or_else(|_| {
        parts
            .get(1)
            .map(|value| (*value).to_string())
            .ok_or_else(|| anyhow::anyhow!("missing operator id"))
    })?;
    let confirmed = has_flag(&parts, "--confirmed");
    anyhow::ensure!(confirmed, "operator-{action} requires --confirmed");
    let admin_risk_acknowledged = has_flag(&parts, "--admin-risk-acknowledged");
    let (privilege_action, status) = operator_lifecycle_privilege_action(action)?;
    let privilege_assertion = operator_management_privilege(
        privilege_context,
        privilege_action,
        &operator_id,
        None,
        None,
        &[],
        None,
        status,
        admin_risk_acknowledged,
        confirmed,
    )?;
    http_post_json(
        api_url,
        &format!("/api/v1/operators/{operator_id}/{action}"),
        token,
        &serde_json::json!({
            "confirmed": confirmed,
            "admin_risk_acknowledged": admin_risk_acknowledged,
            "privilege_assertion": privilege_assertion,
        }),
    )
}

fn submit_operator_password_reset(
    api_url: &str,
    token: Option<&str>,
    command: &str,
    privilege_context: &VtyPrivilegeContext,
) -> Result<String> {
    anyhow::ensure!(
        privilege_context.enabled,
        "enter privileged mode first with: enable"
    );
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let operator_id = required_flag(&parts, "--operator-id").or_else(|_| {
        parts
            .get(1)
            .map(|value| (*value).to_string())
            .ok_or_else(|| anyhow::anyhow!("missing operator id"))
    })?;
    let password_env = optional_flag(&parts, "--password-env")
        .or_else(|| parts.get(2).map(|value| (*value).to_string()))
        .unwrap_or_else(|| "VPSMAN_NEW_OPERATOR_PASSWORD".to_string());
    let confirmed = has_flag(&parts, "--confirmed");
    anyhow::ensure!(confirmed, "operator-password-reset requires --confirmed");
    let admin_risk_acknowledged = has_flag(&parts, "--admin-risk-acknowledged");
    let password = std::env::var(&password_env)?;
    let privilege_assertion = operator_management_privilege(
        privilege_context,
        "operator.password_reset",
        &operator_id,
        None,
        None,
        &[],
        None,
        None,
        admin_risk_acknowledged,
        confirmed,
    )?;
    http_post_json(
        api_url,
        &format!("/api/v1/operators/{operator_id}/password-reset"),
        token,
        &serde_json::json!({
            "password": password,
            "confirmed": confirmed,
            "admin_risk_acknowledged": admin_risk_acknowledged,
            "privilege_assertion": privilege_assertion,
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
    privilege_context: &VtyPrivilegeContext,
) -> Result<String> {
    anyhow::ensure!(
        privilege_context.enabled,
        "enter privileged mode first with: enable"
    );
    let parts = command.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 2 {
        return Ok(
            "usage: operator-session-revoke <session_uuid> [--admin-risk-acknowledged] --confirmed"
                .to_string(),
        );
    }
    let confirmed = has_flag(&parts, "--confirmed");
    anyhow::ensure!(confirmed, "operator-session-revoke requires --confirmed");
    let admin_risk_acknowledged = has_flag(&parts, "--admin-risk-acknowledged");
    let privilege_assertion = operator_management_privilege(
        privilege_context,
        "operator_session.revoke",
        parts[1],
        None,
        None,
        &[],
        None,
        None,
        admin_risk_acknowledged,
        confirmed,
    )?;
    http_post_json(
        api_url,
        &format!("/api/v1/operator-sessions/{}/revoke", parts[1]),
        token,
        &serde_json::json!({
            "confirmed": confirmed,
            "admin_risk_acknowledged": admin_risk_acknowledged,
            "privilege_assertion": privilege_assertion,
        }),
    )
}

fn operator_management_privilege(
    privilege_context: &VtyPrivilegeContext,
    action: &str,
    target: &str,
    username: Option<&str>,
    role: Option<&str>,
    scopes: &[String],
    session_refresh_ttl_secs: Option<u64>,
    status: Option<&str>,
    admin_risk_acknowledged: bool,
    confirmed: bool,
) -> Result<vpsman_common::PrivilegeAssertion> {
    let normalized_scopes = normalized_requested_scopes(scopes);
    let payload_hash = operator_db_payload_hash(OperatorDbPayloadInput {
        action,
        target,
        username,
        role,
        scopes: &normalized_scopes,
        session_refresh_ttl_secs,
        status,
        admin_risk_acknowledged,
    })?;
    let targets = vec![target.to_string()];
    build_privilege_for_db(
        DbPrivilegeRequest {
            action,
            target,
            selector_expression: None,
            resolved_targets: &targets,
            confirmed,
            payload_hash: Some(&payload_hash),
        },
        &privilege_context.password,
        &privilege_context.salt_hex,
        300,
    )
}

fn operator_lifecycle_privilege_action(
    action: &str,
) -> Result<(&'static str, Option<&'static str>)> {
    match action {
        "enable" => Ok(("operator.enable", Some("active"))),
        "disable" => Ok(("operator.disable", Some("disabled"))),
        "delete" => Ok(("operator.delete", Some("deleted"))),
        "totp-clear" => Ok(("operator.totp_clear", None)),
        _ => anyhow::bail!("invalid operator lifecycle action {action}"),
    }
}

fn normalized_requested_scopes(scopes: &[String]) -> Vec<String> {
    let mut scopes = scopes
        .iter()
        .map(|scope| scope.trim())
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    scopes.sort();
    scopes.dedup();
    scopes
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
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
    let confirmed = has_flag(&parts, "--confirmed");
    if !confirmed {
        return Ok(server_job_cancel_usage());
    }
    http_post_json(
        api_url,
        &format!("/api/v1/server-jobs/{job_id}/cancel"),
        token,
        &serde_json::json!({ "confirmed": confirmed }),
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
    "usage: server-job-cancel --job-id <uuid> --confirmed".to_string()
}

fn backup_policy_prune_usage() -> String {
    "usage: backup-policy-prune [--schedule-id <uuid>] [--dry-run] [--metadata-only true|false] [--confirmed]".to_string()
}
