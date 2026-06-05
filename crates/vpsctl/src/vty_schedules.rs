use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::JobCommand;

use crate::{
    commands_schedules::schedule_dispatch_with_material, http::http_post_json,
    vty_jobs::VtyJobSelection,
};

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct VtyScheduleDispatchRequest {
    pub(crate) job_id: String,
    pub(crate) proof_ttl_secs: u64,
    pub(crate) timeout_secs: u64,
    pub(crate) force_unprivileged: bool,
    pub(crate) confirmed: bool,
}

pub(crate) fn parse_vty_schedule_dispatch(tokens: &[&str]) -> Result<VtyScheduleDispatchRequest> {
    anyhow::ensure!(
        !tokens.is_empty(),
        "schedule-dispatch requires <job_uuid> [--confirmed]"
    );
    let job_id = tokens[0].to_string();
    Uuid::parse_str(&job_id).context("schedule-dispatch job id must be a UUID")?;
    let mut timeout_secs = 30_u64;
    let mut proof_ttl_secs = 300_u64;
    let mut force_unprivileged = false;
    let mut confirmed = false;
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--confirmed" => {
                confirmed = true;
                index += 1;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            "--timeout" => {
                let value = tokens
                    .get(index + 1)
                    .context("--timeout requires a value between 1 and 3600")?;
                timeout_secs = parse_bounded_u64(value, "--timeout", 1, 3600)?;
                index += 2;
            }
            value if value.starts_with("--timeout=") => {
                timeout_secs = parse_bounded_u64(
                    value.trim_start_matches("--timeout="),
                    "--timeout",
                    1,
                    3600,
                )?;
                index += 1;
            }
            "--proof-ttl" => {
                let value = tokens
                    .get(index + 1)
                    .context("--proof-ttl requires a value between 1 and 3600")?;
                proof_ttl_secs = parse_bounded_u64(value, "--proof-ttl", 1, 3600)?;
                index += 2;
            }
            value if value.starts_with("--proof-ttl=") => {
                proof_ttl_secs = parse_bounded_u64(
                    value.trim_start_matches("--proof-ttl="),
                    "--proof-ttl",
                    1,
                    3600,
                )?;
                index += 1;
            }
            value => anyhow::bail!("unknown schedule-dispatch option {value}"),
        }
    }
    anyhow::ensure!(
        confirmed,
        "schedule-dispatch requires --confirmed because it executes a frozen privileged run"
    );
    Ok(VtyScheduleDispatchRequest {
        job_id,
        proof_ttl_secs,
        timeout_secs,
        force_unprivileged,
        confirmed,
    })
}

pub(crate) fn submit_vty_schedule_dispatch(
    api_url: &str,
    token: Option<&str>,
    password: &str,
    salt_hex: &str,
    request: VtyScheduleDispatchRequest,
) -> Result<String> {
    schedule_dispatch_with_material(
        api_url,
        token,
        &request.job_id,
        password,
        salt_hex,
        request.proof_ttl_secs,
        request.timeout_secs,
        request.force_unprivileged,
        request.confirmed,
    )
}

pub(crate) fn submit_vty_schedule_create(
    api_url: &str,
    token: Option<&str>,
    name: &str,
    interval_secs: u64,
    command: &str,
    selection: VtyJobSelection,
    options: &VtyScheduleCreateOptions,
) -> Result<String> {
    validate_schedule_policy(
        &options.catch_up_policy,
        options.catch_up_limit,
        options.retry_delay_secs,
        options.max_failures,
    )?;
    http_post_json(
        api_url,
        "/api/v1/schedules",
        token,
        &serde_json::json!({
            "name": name,
            "operation": JobCommand::Shell {
                argv: vec![command.to_string()],
                pty: false,
            },
            "clients": selection.clients,
            "tags": selection.tags,
            "interval_secs": interval_secs,
            "start_at_unix": null,
            "enabled": true,
            "catch_up_policy": &options.catch_up_policy,
            "catch_up_limit": options.catch_up_limit,
            "retry_delay_secs": options.retry_delay_secs,
            "max_failures": options.max_failures,
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

fn parse_bounded_u64(value: &str, flag: &str, min: u64, max: u64) -> Result<u64> {
    let parsed = value
        .parse::<u64>()
        .with_context(|| format!("{flag} must be an integer"))?;
    anyhow::ensure!(
        (min..=max).contains(&parsed),
        "{flag} must be between {min} and {max}"
    );
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::{parse_vty_schedule_create_options, parse_vty_schedule_dispatch};

    #[test]
    fn parses_schedule_dispatch_request() {
        let request = parse_vty_schedule_dispatch(&[
            "8f38c322-7987-4ffe-9206-9a01144ef9d9",
            "--timeout",
            "45",
            "--proof-ttl=120",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();

        assert_eq!(request.job_id, "8f38c322-7987-4ffe-9206-9a01144ef9d9");
        assert_eq!(request.timeout_secs, 45);
        assert_eq!(request.proof_ttl_secs, 120);
        assert!(request.force_unprivileged);
        assert!(request.confirmed);
    }

    #[test]
    fn rejects_schedule_dispatch_without_confirmation_or_bad_values() {
        assert!(parse_vty_schedule_dispatch(&["8f38c322-7987-4ffe-9206-9a01144ef9d9"]).is_err());
        assert!(parse_vty_schedule_dispatch(&[
            "8f38c322-7987-4ffe-9206-9a01144ef9d9",
            "--timeout",
            "0",
            "--confirmed",
        ])
        .is_err());
        assert!(parse_vty_schedule_dispatch(&["not-a-uuid", "--confirmed"]).is_err());
    }

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
