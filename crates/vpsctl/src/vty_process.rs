use std::collections::BTreeMap;

use anyhow::{Context, Result};
use vpsman_common::{
    JobCommand, ProcessResourceLimits, ProcessRestartPolicy, ProcessRunPolicy,
    DEFAULT_MAX_JOB_TIMEOUT_SECS, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
};

use crate::vty_jobs::VtyJobSelection;

#[derive(Debug)]
pub(crate) struct VtyProcessSupervisorRequest {
    pub(crate) command_label: &'static str,
    pub(crate) operation: JobCommand,
    pub(crate) selection: VtyJobSelection,
    pub(crate) max_timeout_secs: u64,
    pub(crate) force_unprivileged: bool,
}

pub(crate) fn process_supervisor_usage(command: &str) -> &'static str {
    match command {
        "process-start" => {
            "usage: process-start <name> --argv <absolute_executable> [--argv arg ...] <target ...> [--cwd <abs>] [--env KEY=VALUE] [--restart-policy never|on-failure|always] [--restart-max-retries <0-100>] [--restart-backoff-secs <0-3600>] [--graceful-stop-secs <1-300>] [--memory-max-bytes <bytes>] [--pids-max <n>] [--open-files-max <n>] [--cpu-shares <2-262144>] [--no-new-privileges] [--max-timeout <secs>] [--force-unprivileged] [--confirmed]"
        }
        "process-stop" => "usage: process-stop <name> <target ...> [--max-timeout <secs>] [--confirmed]",
        "process-restart" => {
            "usage: process-restart <name> <target ...> [--max-timeout <secs>] [--confirmed]"
        }
        "process-status" => {
            "usage: process-status [--name <name>] <target ...> [--max-timeout <secs>] [--confirmed]"
        }
        "process-logs" => {
            "usage: process-logs <name> <target ...> [--max-bytes <1-524288>] [--max-timeout <secs>] [--confirmed]"
        }
        _ => "usage: process-start|process-stop|process-restart|process-status|process-logs ...",
    }
}

pub(crate) fn parse_vty_process_supervisor(
    command: &str,
    tokens: &[&str],
) -> Result<VtyProcessSupervisorRequest> {
    match command {
        "process-start" => parse_process_start(tokens),
        "process-stop" => parse_named_process_command(tokens, "process_stop", |name| {
            JobCommand::ProcessStop { name }
        }),
        "process-restart" => parse_named_process_command(tokens, "process_restart", |name| {
            JobCommand::ProcessRestart { name }
        }),
        "process-status" => parse_process_status(tokens),
        "process-logs" => parse_process_logs(tokens),
        _ => anyhow::bail!("unknown process supervisor command {command}"),
    }
}

pub(crate) fn parse_vty_process_list(tokens: &[&str]) -> Result<VtyProcessSupervisorRequest> {
    let mut limit = 50_u16;
    let mut target_tokens = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--limit" => {
                let value = tokens
                    .get(index + 1)
                    .context("--limit requires a value between 1 and 512")?;
                limit = value.parse().context("--limit must be an integer")?;
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            "--destructive" => {
                anyhow::bail!("process-list is not destructive; omit --destructive");
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    anyhow::ensure!(
        (1..=512).contains(&limit),
        "process-list --limit must be between 1 and 512"
    );
    Ok(VtyProcessSupervisorRequest {
        command_label: "process_list",
        operation: JobCommand::ProcessList { limit },
        selection: VtyJobSelection::parse(&target_tokens)?,
        max_timeout_secs: DEFAULT_MAX_JOB_TIMEOUT_SECS,
        force_unprivileged: false,
    })
}

pub(crate) fn is_vty_process_supervisor_inventory_command(command: &str) -> bool {
    command == "process-supervisor-inventory"
        || command.starts_with("process-supervisor-inventory ")
}

pub(crate) fn process_supervisor_inventory_path(command: &str) -> Result<String> {
    let mut limit = 50_u16;
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    anyhow::ensure!(
        tokens.first() == Some(&"process-supervisor-inventory"),
        "expected process-supervisor-inventory command"
    );
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--limit" => {
                limit = next_value(&tokens, index, "--limit")?
                    .parse()
                    .context("--limit must be an integer")?;
                index += 2;
            }
            value if value.starts_with("--limit=") => {
                limit = value
                    .trim_start_matches("--limit=")
                    .parse()
                    .context("--limit must be an integer")?;
                index += 1;
            }
            value => anyhow::bail!("unexpected argument {value}"),
        }
    }
    anyhow::ensure!(
        (1..=200).contains(&limit),
        "process-supervisor-inventory --limit must be between 1 and 200"
    );
    Ok(format!(
        "/api/v1/process-supervisor/inventory?limit={limit}"
    ))
}

pub(crate) fn parse_vty_user_sessions(tokens: &[&str]) -> Result<VtyProcessSupervisorRequest> {
    let (selection, max_timeout_secs) = parse_selection_and_timeout(tokens, "user-sessions")?;
    Ok(VtyProcessSupervisorRequest {
        command_label: "user_sessions",
        operation: JobCommand::UserSessions,
        selection,
        max_timeout_secs,
        force_unprivileged: false,
    })
}

fn parse_process_start(tokens: &[&str]) -> Result<VtyProcessSupervisorRequest> {
    let name = tokens
        .first()
        .context("process-start requires a managed process name")?
        .to_string();
    let mut argv = Vec::<String>::new();
    let mut cwd = None::<String>;
    let mut env = BTreeMap::<String, String>::new();
    let mut policy = ProcessRunPolicy::default();
    let mut limits = ProcessResourceLimits::default();
    let mut target_tokens = Vec::<&str>::new();
    let mut max_timeout_secs = DEFAULT_MAX_JOB_TIMEOUT_SECS;
    let mut force_unprivileged = false;
    let mut index = 1;

    while index < tokens.len() {
        match tokens[index] {
            "--argv" => {
                argv.push(next_value(tokens, index, "--argv")?.to_string());
                index += 2;
            }
            value if value.starts_with("--argv=") => {
                argv.push(value.trim_start_matches("--argv=").to_string());
                index += 1;
            }
            "--cwd" => {
                cwd = Some(next_value(tokens, index, "--cwd")?.to_string());
                index += 2;
            }
            value if value.starts_with("--cwd=") => {
                cwd = Some(value.trim_start_matches("--cwd=").to_string());
                index += 1;
            }
            "--env" => {
                insert_env(&mut env, next_value(tokens, index, "--env")?)?;
                index += 2;
            }
            value if value.starts_with("--env=") => {
                insert_env(&mut env, value.trim_start_matches("--env="))?;
                index += 1;
            }
            "--restart-policy" => {
                policy.restart =
                    parse_restart_policy(next_value(tokens, index, "--restart-policy")?)?;
                index += 2;
            }
            value if value.starts_with("--restart-policy=") => {
                policy.restart =
                    parse_restart_policy(value.trim_start_matches("--restart-policy="))?;
                index += 1;
            }
            "--restart-max-retries" => {
                policy.restart_max_retries = parse_bounded_u64(
                    next_value(tokens, index, "--restart-max-retries")?,
                    0,
                    100,
                    "--restart-max-retries",
                )? as u16;
                index += 2;
            }
            value if value.starts_with("--restart-max-retries=") => {
                policy.restart_max_retries = parse_bounded_u64(
                    value.trim_start_matches("--restart-max-retries="),
                    0,
                    100,
                    "--restart-max-retries",
                )? as u16;
                index += 1;
            }
            "--restart-backoff-secs" => {
                policy.restart_backoff_secs = parse_bounded_u64(
                    next_value(tokens, index, "--restart-backoff-secs")?,
                    0,
                    3600,
                    "--restart-backoff-secs",
                )?;
                index += 2;
            }
            value if value.starts_with("--restart-backoff-secs=") => {
                policy.restart_backoff_secs = parse_bounded_u64(
                    value.trim_start_matches("--restart-backoff-secs="),
                    0,
                    3600,
                    "--restart-backoff-secs",
                )?;
                index += 1;
            }
            "--graceful-stop-secs" => {
                policy.graceful_stop_secs = parse_bounded_u64(
                    next_value(tokens, index, "--graceful-stop-secs")?,
                    1,
                    300,
                    "--graceful-stop-secs",
                )?;
                index += 2;
            }
            value if value.starts_with("--graceful-stop-secs=") => {
                policy.graceful_stop_secs = parse_bounded_u64(
                    value.trim_start_matches("--graceful-stop-secs="),
                    1,
                    300,
                    "--graceful-stop-secs",
                )?;
                index += 1;
            }
            "--memory-max-bytes" => {
                limits.memory_max_bytes = Some(parse_bounded_u64(
                    next_value(tokens, index, "--memory-max-bytes")?,
                    1024 * 1024,
                    1024_u64.pow(4),
                    "--memory-max-bytes",
                )?);
                index += 2;
            }
            value if value.starts_with("--memory-max-bytes=") => {
                limits.memory_max_bytes = Some(parse_bounded_u64(
                    value.trim_start_matches("--memory-max-bytes="),
                    1024 * 1024,
                    1024_u64.pow(4),
                    "--memory-max-bytes",
                )?);
                index += 1;
            }
            "--pids-max" => {
                limits.pids_max = Some(parse_bounded_u64(
                    next_value(tokens, index, "--pids-max")?,
                    1,
                    65_535,
                    "--pids-max",
                )? as u32);
                index += 2;
            }
            value if value.starts_with("--pids-max=") => {
                limits.pids_max = Some(parse_bounded_u64(
                    value.trim_start_matches("--pids-max="),
                    1,
                    65_535,
                    "--pids-max",
                )? as u32);
                index += 1;
            }
            "--open-files-max" => {
                limits.open_files_max = Some(parse_bounded_u64(
                    next_value(tokens, index, "--open-files-max")?,
                    16,
                    1_048_576,
                    "--open-files-max",
                )?);
                index += 2;
            }
            value if value.starts_with("--open-files-max=") => {
                limits.open_files_max = Some(parse_bounded_u64(
                    value.trim_start_matches("--open-files-max="),
                    16,
                    1_048_576,
                    "--open-files-max",
                )?);
                index += 1;
            }
            "--cpu-shares" => {
                limits.cpu_shares = Some(parse_bounded_u64(
                    next_value(tokens, index, "--cpu-shares")?,
                    2,
                    262_144,
                    "--cpu-shares",
                )? as u32);
                index += 2;
            }
            value if value.starts_with("--cpu-shares=") => {
                limits.cpu_shares = Some(parse_bounded_u64(
                    value.trim_start_matches("--cpu-shares="),
                    2,
                    262_144,
                    "--cpu-shares",
                )? as u32);
                index += 1;
            }
            "--no-new-privileges" => {
                limits.no_new_privileges = true;
                index += 1;
            }
            "--force-unprivileged" => {
                force_unprivileged = true;
                index += 1;
            }
            "--max-timeout" => {
                max_timeout_secs = parse_timeout(next_value(tokens, index, "--max-timeout")?)?;
                index += 2;
            }
            value if value.starts_with("--max-timeout=") => {
                max_timeout_secs = parse_timeout(value.trim_start_matches("--max-timeout="))?;
                index += 1;
            }
            "--destructive" => {
                anyhow::bail!("process supervisor commands do not accept --destructive")
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    anyhow::ensure!(
        !argv.is_empty(),
        "process-start requires at least one --argv"
    );
    anyhow::ensure!(
        argv[0].starts_with('/'),
        "process executable must be an absolute path"
    );
    let selection = VtyJobSelection::parse(&target_tokens)?;
    Ok(VtyProcessSupervisorRequest {
        command_label: "process_start",
        operation: JobCommand::ProcessStart {
            name,
            argv,
            cwd,
            env,
            policy,
            limits,
        },
        selection,
        max_timeout_secs,
        force_unprivileged,
    })
}

fn parse_named_process_command(
    tokens: &[&str],
    command_label: &'static str,
    build: impl FnOnce(String) -> JobCommand,
) -> Result<VtyProcessSupervisorRequest> {
    let name = tokens
        .first()
        .context("process command requires a managed process name")?
        .to_string();
    let (selection, max_timeout_secs) = parse_selection_and_timeout(&tokens[1..], command_label)?;
    Ok(VtyProcessSupervisorRequest {
        command_label,
        operation: build(name),
        selection,
        max_timeout_secs,
        force_unprivileged: false,
    })
}

fn parse_process_status(tokens: &[&str]) -> Result<VtyProcessSupervisorRequest> {
    let mut name = None::<String>;
    let mut target_tokens = Vec::<&str>::new();
    let mut max_timeout_secs = DEFAULT_MAX_JOB_TIMEOUT_SECS;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--name" => {
                name = Some(next_value(tokens, index, "--name")?.to_string());
                index += 2;
            }
            value if value.starts_with("--name=") => {
                name = Some(value.trim_start_matches("--name=").to_string());
                index += 1;
            }
            "--max-timeout" => {
                max_timeout_secs = parse_timeout(next_value(tokens, index, "--max-timeout")?)?;
                index += 2;
            }
            value if value.starts_with("--max-timeout=") => {
                max_timeout_secs = parse_timeout(value.trim_start_matches("--max-timeout="))?;
                index += 1;
            }
            "--destructive" => {
                anyhow::bail!("process-status is not destructive; omit --destructive")
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    let selection = VtyJobSelection::parse(&target_tokens)?;
    Ok(VtyProcessSupervisorRequest {
        command_label: "process_status",
        operation: JobCommand::ProcessStatus { name },
        selection,
        max_timeout_secs,
        force_unprivileged: false,
    })
}

fn parse_process_logs(tokens: &[&str]) -> Result<VtyProcessSupervisorRequest> {
    let name = tokens
        .first()
        .context("process-logs requires a managed process name")?
        .to_string();
    let mut max_bytes = 65_536_u32;
    let mut target_tokens = Vec::<&str>::new();
    let mut max_timeout_secs = DEFAULT_MAX_JOB_TIMEOUT_SECS;
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--max-bytes" => {
                max_bytes = parse_max_bytes(next_value(tokens, index, "--max-bytes")?)?;
                index += 2;
            }
            value if value.starts_with("--max-bytes=") => {
                max_bytes = parse_max_bytes(value.trim_start_matches("--max-bytes="))?;
                index += 1;
            }
            "--max-timeout" => {
                max_timeout_secs = parse_timeout(next_value(tokens, index, "--max-timeout")?)?;
                index += 2;
            }
            value if value.starts_with("--max-timeout=") => {
                max_timeout_secs = parse_timeout(value.trim_start_matches("--max-timeout="))?;
                index += 1;
            }
            "--destructive" => anyhow::bail!("process-logs is not destructive; omit --destructive"),
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    let selection = VtyJobSelection::parse(&target_tokens)?;
    Ok(VtyProcessSupervisorRequest {
        command_label: "process_logs",
        operation: JobCommand::ProcessLogs { name, max_bytes },
        selection,
        max_timeout_secs,
        force_unprivileged: false,
    })
}

fn parse_selection_and_timeout(
    tokens: &[&str],
    command_label: &str,
) -> Result<(VtyJobSelection, u64)> {
    let mut target_tokens = Vec::<&str>::new();
    let mut max_timeout_secs = DEFAULT_MAX_JOB_TIMEOUT_SECS;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--max-timeout" => {
                max_timeout_secs = parse_timeout(next_value(tokens, index, "--max-timeout")?)?;
                index += 2;
            }
            value if value.starts_with("--max-timeout=") => {
                max_timeout_secs = parse_timeout(value.trim_start_matches("--max-timeout="))?;
                index += 1;
            }
            "--destructive" => {
                anyhow::bail!("{command_label} does not accept --destructive")
            }
            value => {
                target_tokens.push(value);
                index += 1;
            }
        }
    }
    Ok((VtyJobSelection::parse(&target_tokens)?, max_timeout_secs))
}

fn next_value<'a>(tokens: &'a [&str], index: usize, flag: &str) -> Result<&'a str> {
    tokens
        .get(index + 1)
        .copied()
        .with_context(|| format!("{flag} requires a value"))
}

fn insert_env(env: &mut BTreeMap<String, String>, value: &str) -> Result<()> {
    let (key, env_value) = value
        .split_once('=')
        .with_context(|| format!("invalid --env {value}; expected KEY=VALUE"))?;
    anyhow::ensure!(!key.is_empty(), "process env key is empty");
    env.insert(key.to_string(), env_value.to_string());
    Ok(())
}

fn parse_timeout(value: &str) -> Result<u64> {
    let timeout = value
        .parse::<u64>()
        .context("--max-timeout must be an integer")?;
    anyhow::ensure!(
        (1..=MAX_CONFIGURABLE_JOB_TIMEOUT_SECS).contains(&timeout),
        "--max-timeout must be between 1 and {MAX_CONFIGURABLE_JOB_TIMEOUT_SECS}"
    );
    Ok(timeout)
}

fn parse_restart_policy(value: &str) -> Result<ProcessRestartPolicy> {
    match value {
        "never" => Ok(ProcessRestartPolicy::Never),
        "on-failure" | "on_failure" => Ok(ProcessRestartPolicy::OnFailure),
        "always" => Ok(ProcessRestartPolicy::Always),
        _ => anyhow::bail!("--restart-policy must be never, on-failure, or always"),
    }
}

fn parse_bounded_u64(value: &str, min: u64, max: u64, flag: &str) -> Result<u64> {
    let parsed = value
        .parse::<u64>()
        .with_context(|| format!("{flag} must be an integer"))?;
    anyhow::ensure!(
        (min..=max).contains(&parsed),
        "{flag} must be between {min} and {max}"
    );
    Ok(parsed)
}

fn parse_max_bytes(value: &str) -> Result<u32> {
    let max_bytes = value
        .parse::<u32>()
        .context("--max-bytes must be an integer")?;
    anyhow::ensure!(
        (1..=512 * 1024).contains(&max_bytes),
        "--max-bytes must be between 1 and 524288"
    );
    Ok(max_bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        parse_vty_process_list, parse_vty_process_supervisor, parse_vty_user_sessions,
        process_supervisor_inventory_path,
    };
    use vpsman_common::{
        JobCommand, ProcessResourceLimits, ProcessRestartPolicy, ProcessRunPolicy,
    };

    const TEST_PROCESS_ARGV_SLEEP: &str = "/bin/sleep";

    #[test]
    fn parses_vty_process_list_targets_and_limit() {
        let request = parse_vty_process_list(&["id:client-a", "tag:bgp", "--limit", "25"]).unwrap();

        assert_eq!(request.command_label, "process_list");
        assert!(request.selection.clients.is_empty());
        assert_eq!(request.selection.tags, vec!["bgp", "id:client-a"]);
        assert!(!request.selection.destructive);
        match request.operation {
            JobCommand::ProcessList { limit } => assert_eq!(limit, 25),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_vty_process_list_limit() {
        assert!(parse_vty_process_list(&["tag:bgp", "--limit", "0"]).is_err());
        assert!(parse_vty_process_list(&["tag:bgp", "--limit=600"]).is_err());
        assert!(parse_vty_process_list(&["tag:bgp", "--destructive"]).is_err());
    }

    #[test]
    fn parses_vty_user_sessions_targets_and_timeout() {
        let request =
            parse_vty_user_sessions(&["id:client-a", "tag:bgp", "--confirmed", "--max-timeout=45"])
                .unwrap();

        assert_eq!(request.command_label, "user_sessions");
        assert_eq!(request.max_timeout_secs, 45);
        assert!(request.selection.clients.is_empty());
        assert_eq!(request.selection.tags, vec!["bgp", "id:client-a"]);
        assert!(request.selection.confirmed);
        match request.operation {
            JobCommand::UserSessions => {}
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_process_supervisor_inventory_limit() {
        assert_eq!(
            process_supervisor_inventory_path("process-supervisor-inventory --limit 25").unwrap(),
            "/api/v1/process-supervisor/inventory?limit=25"
        );
        assert!(
            process_supervisor_inventory_path("process-supervisor-inventory --limit=0").is_err()
        );
        assert!(
            process_supervisor_inventory_path("process-supervisor-inventory tag:edge").is_err()
        );
    }

    #[test]
    fn rejects_invalid_vty_user_sessions_flags() {
        assert!(parse_vty_user_sessions(&["tag:bgp", "--max-timeout=0"]).is_err());
        assert!(parse_vty_user_sessions(&["tag:bgp", "--destructive"]).is_err());
        assert!(parse_vty_user_sessions(&["--confirmed"]).is_err());
    }

    #[test]
    fn parses_vty_process_start_with_targets_and_options() {
        let request = parse_vty_process_supervisor(
            "process-start",
            &[
                "edge-worker",
                "--argv",
                TEST_PROCESS_ARGV_SLEEP,
                "--argv=60",
                "--cwd",
                "/tmp",
                "--env=KEY=value",
                "id:client-a",
                "tag:bgp",
                "--confirmed",
                "--max-timeout=45",
            ],
        )
        .unwrap();

        assert_eq!(request.command_label, "process_start");
        assert_eq!(request.max_timeout_secs, 45);
        assert!(request.selection.clients.is_empty());
        assert_eq!(request.selection.tags, vec!["bgp", "id:client-a"]);
        assert!(request.selection.confirmed);
        match request.operation {
            JobCommand::ProcessStart {
                name,
                argv,
                cwd,
                env,
                policy,
                limits,
            } => {
                assert_eq!(name, "edge-worker");
                assert_eq!(argv, vec!["/bin/sleep", "60"]);
                assert_eq!(cwd.as_deref(), Some("/tmp"));
                assert_eq!(env.get("KEY").map(String::as_str), Some("value"));
                assert_eq!(policy, ProcessRunPolicy::default());
                assert_eq!(limits, ProcessResourceLimits::default());
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_vty_process_start_policy_and_limits() {
        let request = parse_vty_process_supervisor(
            "process-start",
            &[
                "limited-worker",
                "--argv=/bin/sleep",
                "--argv=60",
                "--restart-policy=on-failure",
                "--restart-max-retries=3",
                "--restart-backoff-secs=10",
                "--graceful-stop-secs=15",
                "--memory-max-bytes=134217728",
                "--pids-max=32",
                "--open-files-max=256",
                "--cpu-shares=1024",
                "--no-new-privileges",
                "--force-unprivileged",
                "id:client-a",
                "--confirmed",
            ],
        )
        .unwrap();

        assert!(request.force_unprivileged);
        match request.operation {
            JobCommand::ProcessStart { policy, limits, .. } => {
                assert_eq!(policy.restart, ProcessRestartPolicy::OnFailure);
                assert_eq!(policy.restart_max_retries, 3);
                assert_eq!(policy.restart_backoff_secs, 10);
                assert_eq!(policy.graceful_stop_secs, 15);
                assert_eq!(limits.memory_max_bytes, Some(134217728));
                assert_eq!(limits.pids_max, Some(32));
                assert_eq!(limits.open_files_max, Some(256));
                assert_eq!(limits.cpu_shares, Some(1024));
                assert!(limits.no_new_privileges);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_vty_process_status_and_logs() {
        let status =
            parse_vty_process_supervisor("process-status", &["--name=demo", "pool:pool-a"])
                .unwrap();
        assert_eq!(status.command_label, "process_status");
        match status.operation {
            JobCommand::ProcessStatus { name } => assert_eq!(name.as_deref(), Some("demo")),
            other => panic!("unexpected command: {other:?}"),
        }

        let logs = parse_vty_process_supervisor(
            "process-logs",
            &["demo", "--max-bytes", "4096", "tag:edge"],
        )
        .unwrap();
        assert_eq!(logs.command_label, "process_logs");
        match logs.operation {
            JobCommand::ProcessLogs { name, max_bytes } => {
                assert_eq!(name, "demo");
                assert_eq!(max_bytes, 4096);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn rejects_bad_vty_process_supervisor_requests() {
        assert!(parse_vty_process_supervisor(
            "process-start",
            &["demo", "--argv", "sleep", "tag:edge"]
        )
        .is_err());
        assert!(parse_vty_process_supervisor(
            "process-logs",
            &["demo", "--max-bytes", "0", "tag:edge"]
        )
        .is_err());
        assert!(parse_vty_process_supervisor(
            "process-stop",
            &["demo", "--destructive", "tag:edge"]
        )
        .is_err());
    }
}
