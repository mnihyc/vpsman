use anyhow::{Context, Result};
use vpsman_common::{
    parse_and_validate_alert_event_expression, validate_alert_event_argv_template, JobCommand,
};

use crate::{
    commands_schedules::{resolve_schedule_target_ids, selector_expression_from_targets},
    http::http_post_json,
    privilege::{build_privilege_for_schedule, SchedulePrivilegePayload, SchedulePrivilegeRequest},
    vty_jobs::{VtyJobSelection, VtyPrivilegeContext},
};

pub(crate) struct VtyScheduleCreateRequest<'a> {
    pub(crate) api_url: &'a str,
    pub(crate) token: Option<&'a str>,
    pub(crate) name: &'a str,
    pub(crate) cron_expr: &'a str,
    pub(crate) command: &'a str,
    pub(crate) selection: VtyJobSelection,
    pub(crate) options: &'a VtyScheduleCreateOptions,
    pub(crate) privilege_context: &'a VtyPrivilegeContext,
}

pub(crate) fn submit_vty_schedule_create(request: VtyScheduleCreateRequest<'_>) -> Result<String> {
    anyhow::ensure!(
        request.options.confirmed,
        "schedule-create requires --confirmed"
    );
    validate_schedule_policy(
        &request.options.catch_up_policy,
        request.options.catch_up_limit,
        request.options.retry_delay_secs,
        request.options.max_failures,
    )?;
    let operation = JobCommand::Shell {
        argv: vec![request.command.to_string()],
        pty: false,
    };
    let selector_expression =
        selector_expression_from_targets(&request.selection.clients, &request.selection.tags);
    anyhow::ensure!(
        !selector_expression.is_empty(),
        "schedule-create requires at least one target selector"
    );
    let target_ids =
        resolve_schedule_target_ids(request.api_url, request.token, &selector_expression)?;
    let privilege_assertion = build_privilege_for_schedule(
        SchedulePrivilegeRequest {
            action: "schedule.create",
            schedule_id: None,
            definition_revision: None,
            name: request.name,
            payload: SchedulePrivilegePayload::Operation(&operation),
            command_type: "shell_argv",
            selector_expression: &selector_expression,
            resolved_targets: &target_ids,
            trigger_kind: "cron",
            cron_expr: Some(request.cron_expr),
            timezone: Some("UTC"),
            event_expression: None,
            enabled: !request.options.disabled,
            catch_up_policy: Some(&request.options.catch_up_policy),
            catch_up_limit: Some(request.options.catch_up_limit),
            retry_delay_secs: Some(request.options.retry_delay_secs),
            max_failures: request.options.max_failures,
            deferred_until: None,
            deleted: false,
        },
        &request.privilege_context.password,
        &request.privilege_context.salt_hex,
        300,
    )?;
    http_post_json(
        request.api_url,
        "/api/v1/schedules",
        request.token,
        &serde_json::json!({
            "name": request.name,
            "operation": operation,
            "event_argv_template": null,
            "selector_expression": selector_expression,
            "target_client_ids": target_ids,
            "trigger_kind": "cron",
            "cron_expr": request.cron_expr,
            "timezone": "UTC",
            "event_expression": null,
            "enabled": !request.options.disabled,
            "catch_up_policy": &request.options.catch_up_policy,
            "catch_up_limit": request.options.catch_up_limit,
            "retry_delay_secs": request.options.retry_delay_secs,
            "max_failures": request.options.max_failures,
            "confirmed": request.options.confirmed,
            "privilege_assertion": privilege_assertion,
        }),
    )
}

pub(crate) struct VtyEventScheduleCreateRequest<'a> {
    pub(crate) api_url: &'a str,
    pub(crate) token: Option<&'a str>,
    pub(crate) name: &'a str,
    pub(crate) event_expression: &'a str,
    pub(crate) selection: VtyJobSelection,
    pub(crate) options: &'a VtyEventScheduleCreateOptions,
    pub(crate) privilege_context: &'a VtyPrivilegeContext,
}

pub(crate) fn submit_vty_event_schedule_create(
    request: VtyEventScheduleCreateRequest<'_>,
) -> Result<String> {
    anyhow::ensure!(
        request.options.confirmed,
        "schedule-event-create requires --confirmed"
    );
    parse_and_validate_alert_event_expression(request.event_expression)
        .map_err(anyhow::Error::msg)
        .context(
            "invalid alert event expression; use the Schedule web UI for per-edge server preview",
        )?;
    anyhow::ensure!(
        (1..=100).contains(&request.options.max_failures),
        "max failures must be between 1 and 100"
    );
    let event_argv_template = if request.options.event_argv_template.is_empty() {
        None
    } else {
        Some(request.options.event_argv_template.as_slice())
    };
    validate_alert_event_argv_template(event_argv_template)
        .map_err(anyhow::Error::msg)
        .context(
            "invalid event argv template; use the Schedule web UI for per-edge server preview",
        )?;
    let selector_expression =
        selector_expression_from_targets(&request.selection.clients, &request.selection.tags);
    anyhow::ensure!(
        !selector_expression.is_empty(),
        "schedule-event-create requires at least one target selector"
    );
    let target_ids =
        resolve_schedule_target_ids(request.api_url, request.token, &selector_expression)?;
    let privilege_assertion = build_privilege_for_schedule(
        SchedulePrivilegeRequest {
            action: "schedule.create",
            schedule_id: None,
            definition_revision: None,
            name: request.name,
            payload: SchedulePrivilegePayload::AlertEventArgv(event_argv_template),
            command_type: "shell",
            selector_expression: &selector_expression,
            resolved_targets: &target_ids,
            trigger_kind: "event",
            cron_expr: None,
            timezone: None,
            event_expression: Some(request.event_expression),
            enabled: !request.options.disabled,
            catch_up_policy: None,
            catch_up_limit: None,
            retry_delay_secs: None,
            max_failures: request.options.max_failures,
            deferred_until: None,
            deleted: false,
        },
        &request.privilege_context.password,
        &request.privilege_context.salt_hex,
        300,
    )?;
    http_post_json(
        request.api_url,
        "/api/v1/schedules",
        request.token,
        &serde_json::json!({
            "name": request.name,
            "operation": null,
            "event_argv_template": event_argv_template,
            "selector_expression": selector_expression,
            "target_client_ids": target_ids,
            "trigger_kind": "event",
            "cron_expr": null,
            "timezone": null,
            "event_expression": request.event_expression,
            "enabled": !request.options.disabled,
            "catch_up_policy": null,
            "catch_up_limit": null,
            "retry_delay_secs": null,
            "max_failures": request.options.max_failures,
            "confirmed": request.options.confirmed,
            "privilege_assertion": privilege_assertion,
        }),
    )
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyScheduleCreateOptions {
    pub(crate) catch_up_policy: String,
    pub(crate) catch_up_limit: i32,
    pub(crate) retry_delay_secs: i64,
    pub(crate) max_failures: i32,
    pub(crate) disabled: bool,
    pub(crate) confirmed: bool,
    pub(crate) target_tokens: Vec<String>,
}

impl Default for VtyScheduleCreateOptions {
    fn default() -> Self {
        Self {
            catch_up_policy: "skip_missed".to_string(),
            catch_up_limit: 1,
            retry_delay_secs: 300,
            max_failures: 3,
            disabled: false,
            confirmed: false,
            target_tokens: Vec::new(),
        }
    }
}

pub(crate) fn parse_vty_schedule_create_options(
    tokens: &[&str],
) -> Result<VtyScheduleCreateOptions> {
    let mut options = VtyScheduleCreateOptions::default();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--catch-up-policy" => {
                options.catch_up_policy = tokens
                    .get(index + 1)
                    .context("--catch-up-policy requires a value")?
                    .to_string();
                index += 2;
            }
            value if value.starts_with("--catch-up-policy=") => {
                options.catch_up_policy =
                    value.trim_start_matches("--catch-up-policy=").to_string();
                index += 1;
            }
            "--catch-up-limit" => {
                options.catch_up_limit = parse_bounded_i32(
                    tokens
                        .get(index + 1)
                        .context("--catch-up-limit requires a value")?,
                    "--catch-up-limit",
                    1,
                    25,
                )?;
                index += 2;
            }
            value if value.starts_with("--catch-up-limit=") => {
                options.catch_up_limit = parse_bounded_i32(
                    value.trim_start_matches("--catch-up-limit="),
                    "--catch-up-limit",
                    1,
                    25,
                )?;
                index += 1;
            }
            "--retry-delay-secs" => {
                options.retry_delay_secs = parse_bounded_i64(
                    tokens
                        .get(index + 1)
                        .context("--retry-delay-secs requires a value")?,
                    "--retry-delay-secs",
                    1,
                    86_400,
                )?;
                index += 2;
            }
            value if value.starts_with("--retry-delay-secs=") => {
                options.retry_delay_secs = parse_bounded_i64(
                    value.trim_start_matches("--retry-delay-secs="),
                    "--retry-delay-secs",
                    1,
                    86_400,
                )?;
                index += 1;
            }
            "--max-failures" => {
                options.max_failures = parse_bounded_i32(
                    tokens
                        .get(index + 1)
                        .context("--max-failures requires a value")?,
                    "--max-failures",
                    1,
                    100,
                )?;
                index += 2;
            }
            value if value.starts_with("--max-failures=") => {
                options.max_failures = parse_bounded_i32(
                    value.trim_start_matches("--max-failures="),
                    "--max-failures",
                    1,
                    100,
                )?;
                index += 1;
            }
            "--confirmed" => {
                options.confirmed = true;
                index += 1;
            }
            "--disabled" => {
                options.disabled = true;
                index += 1;
            }
            value => {
                options.target_tokens.push(value.to_string());
                index += 1;
            }
        }
    }
    validate_schedule_policy(
        &options.catch_up_policy,
        options.catch_up_limit,
        options.retry_delay_secs,
        options.max_failures,
    )?;
    Ok(options)
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyEventScheduleCreateOptions {
    pub(crate) event_argv_template: Vec<String>,
    pub(crate) max_failures: i32,
    pub(crate) disabled: bool,
    pub(crate) confirmed: bool,
    pub(crate) target_tokens: Vec<String>,
}

impl Default for VtyEventScheduleCreateOptions {
    fn default() -> Self {
        Self {
            event_argv_template: Vec::new(),
            max_failures: 3,
            disabled: false,
            confirmed: false,
            target_tokens: Vec::new(),
        }
    }
}

pub(crate) fn parse_vty_event_schedule_create_options(
    tokens: &[&str],
) -> Result<VtyEventScheduleCreateOptions> {
    let mut options = VtyEventScheduleCreateOptions::default();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--event-argv-template" => {
                options.event_argv_template.push(
                    tokens
                        .get(index + 1)
                        .context("--event-argv-template requires one argv element")?
                        .to_string(),
                );
                index += 2;
            }
            value if value.starts_with("--event-argv-template=") => {
                options.event_argv_template.push(
                    value
                        .trim_start_matches("--event-argv-template=")
                        .to_string(),
                );
                index += 1;
            }
            "--max-failures" => {
                options.max_failures = parse_bounded_i32(
                    tokens
                        .get(index + 1)
                        .context("--max-failures requires a value")?,
                    "--max-failures",
                    1,
                    100,
                )?;
                index += 2;
            }
            value if value.starts_with("--max-failures=") => {
                options.max_failures = parse_bounded_i32(
                    value.trim_start_matches("--max-failures="),
                    "--max-failures",
                    1,
                    100,
                )?;
                index += 1;
            }
            "--disabled" => {
                options.disabled = true;
                index += 1;
            }
            "--confirmed" => {
                options.confirmed = true;
                index += 1;
            }
            value if value.starts_with("--") => {
                anyhow::bail!("unsupported schedule-event-create option {value}")
            }
            value => {
                options.target_tokens.push(value.to_string());
                index += 1;
            }
        }
    }
    validate_alert_event_argv_template(
        (!options.event_argv_template.is_empty()).then_some(options.event_argv_template.as_slice()),
    )
    .map_err(anyhow::Error::msg)
    .context("invalid event argv template; use the Schedule web UI for per-edge server preview")?;
    Ok(options)
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
        "catch-up policy must be skip_missed, run_once, or run_all_limited"
    );
    anyhow::ensure!(
        (1..=25).contains(&catch_up_limit),
        "catch-up limit must be between 1 and 25"
    );
    anyhow::ensure!(
        (1..=86_400).contains(&retry_delay_secs),
        "retry delay must be between 1 and 86400 seconds"
    );
    anyhow::ensure!(
        (1..=100).contains(&max_failures),
        "max failures must be between 1 and 100"
    );
    Ok(())
}

fn parse_bounded_i32(value: &str, flag: &str, min: i32, max: i32) -> Result<i32> {
    let parsed = value
        .parse::<i32>()
        .with_context(|| format!("{flag} must be an integer"))?;
    anyhow::ensure!(
        (min..=max).contains(&parsed),
        "{flag} must be between {min} and {max}"
    );
    Ok(parsed)
}

fn parse_bounded_i64(value: &str, flag: &str, min: i64, max: i64) -> Result<i64> {
    let parsed = value
        .parse::<i64>()
        .with_context(|| format!("{flag} must be an integer"))?;
    anyhow::ensure!(
        (min..=max).contains(&parsed),
        "{flag} must be between {min} and {max}"
    );
    Ok(parsed)
}

#[cfg(test)]
#[path = "tests_vty_schedules.rs"]
mod tests;
