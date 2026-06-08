use anyhow::{Context, Result};
use vpsman_common::JobCommand;

use crate::{
    commands_schedules::{resolve_schedule_target_ids, selector_expression_from_targets},
    http::http_post_json,
    privilege::build_privilege_for_schedule,
    vty_jobs::{VtyJobSelection, VtyPrivilegeContext},
};

pub(crate) fn submit_vty_schedule_create(
    api_url: &str,
    token: Option<&str>,
    name: &str,
    cron_expr: &str,
    command: &str,
    selection: VtyJobSelection,
    options: &VtyScheduleCreateOptions,
    privilege_context: &VtyPrivilegeContext,
) -> Result<String> {
    validate_schedule_policy(
        &options.catch_up_policy,
        options.catch_up_limit,
        options.retry_delay_secs,
        options.max_failures,
    )?;
    let operation = JobCommand::Shell {
        argv: vec![command.to_string()],
        pty: false,
    };
    let selector_expression = selector_expression_from_targets(&selection.clients, &selection.tags);
    anyhow::ensure!(
        !selector_expression.is_empty(),
        "schedule-create requires at least one target selector"
    );
    let target_ids = resolve_schedule_target_ids(api_url, token, &selector_expression)?;
    let privilege_assertion = build_privilege_for_schedule(
        "schedule.create",
        None,
        name,
        &operation,
        "shell_argv",
        &selector_expression,
        &target_ids,
        cron_expr,
        "UTC",
        true,
        &options.catch_up_policy,
        options.catch_up_limit,
        options.retry_delay_secs,
        options.max_failures,
        None,
        false,
        &privilege_context.password,
        &privilege_context.salt_hex,
        300,
    )?;
    http_post_json(
        api_url,
        "/api/v1/schedules",
        token,
        &serde_json::json!({
            "name": name,
            "operation": operation,
            "selector_expression": selector_expression,
            "cron_expr": cron_expr,
            "timezone": "UTC",
            "enabled": true,
            "catch_up_policy": &options.catch_up_policy,
            "catch_up_limit": options.catch_up_limit,
            "retry_delay_secs": options.retry_delay_secs,
            "max_failures": options.max_failures,
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
    pub(crate) target_tokens: Vec<String>,
}

impl Default for VtyScheduleCreateOptions {
    fn default() -> Self {
        Self {
            catch_up_policy: "skip_missed".to_string(),
            catch_up_limit: 1,
            retry_delay_secs: 300,
            max_failures: 3,
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
mod tests {
    use super::parse_vty_schedule_create_options;

    #[test]
    fn parses_schedule_create_policy_options() {
        let options = parse_vty_schedule_create_options(&[
            "--catch-up-policy",
            "run_all_limited",
            "--catch-up-limit=4",
            "--retry-delay-secs",
            "120",
            "--max-failures=7",
            "tag:edge",
        ])
        .unwrap();

        assert_eq!(options.catch_up_policy, "run_all_limited");
        assert_eq!(options.catch_up_limit, 4);
        assert_eq!(options.retry_delay_secs, 120);
        assert_eq!(options.max_failures, 7);
        assert_eq!(options.target_tokens, vec!["tag:edge"]);
        assert!(parse_vty_schedule_create_options(&["--catch-up-policy", "bad"]).is_err());
    }
}
