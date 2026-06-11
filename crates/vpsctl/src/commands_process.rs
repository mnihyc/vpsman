use std::collections::BTreeMap;

use anyhow::{Context, Result};
use vpsman_common::{JobCommand, ProcessResourceLimits, ProcessRestartPolicy, ProcessRunPolicy};

use crate::{
    http::http_get,
    jobs::{submit_privileged_operation, PrivilegedOperationRequest},
};

pub(crate) struct ProcessStartOptions {
    pub(crate) name: String,
    pub(crate) argv: Vec<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) env: Vec<String>,
    pub(crate) restart_policy: String,
    pub(crate) restart_max_retries: u16,
    pub(crate) restart_backoff_secs: u64,
    pub(crate) graceful_stop_secs: u64,
    pub(crate) memory_max_bytes: Option<u64>,
    pub(crate) pids_max: Option<u32>,
    pub(crate) open_files_max: Option<u64>,
    pub(crate) cpu_shares: Option<u32>,
    pub(crate) no_new_privileges: bool,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) password_env: String,
    pub(crate) super_salt_hex: Option<String>,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

pub(crate) fn user_sessions(
    api_url: &str,
    token: Option<&str>,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &JobCommand::UserSessions,
            command_label: "user_sessions",
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

pub(crate) fn process_list(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        (1..=512).contains(&limit),
        "process list limit must be between 1 and 512"
    );
    let operation = JobCommand::ProcessList { limit };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "process_list",
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

pub(crate) fn process_start(
    api_url: &str,
    token: Option<&str>,
    options: ProcessStartOptions,
) -> Result<()> {
    anyhow::ensure!(
        !options.argv.is_empty(),
        "process start requires at least one --argv"
    );
    anyhow::ensure!(
        options.argv[0].starts_with('/'),
        "process executable must be an absolute path"
    );
    let operation = JobCommand::ProcessStart {
        name: options.name,
        argv: options.argv,
        cwd: options.cwd,
        env: parse_env_pairs(&options.env)?,
        policy: ProcessRunPolicy {
            restart: parse_restart_policy(&options.restart_policy)?,
            restart_max_retries: options.restart_max_retries,
            restart_backoff_secs: options.restart_backoff_secs,
            graceful_stop_secs: options.graceful_stop_secs,
        },
        limits: ProcessResourceLimits {
            memory_max_bytes: options.memory_max_bytes,
            pids_max: options.pids_max,
            open_files_max: options.open_files_max,
            cpu_shares: options.cpu_shares,
            no_new_privileges: options.no_new_privileges,
        },
    };
    submit_process_operation(
        api_url,
        token,
        &operation,
        "process_start",
        options.clients,
        options.tags,
        options.password_env,
        options.super_salt_hex,
        options.privilege_ttl_secs,
        options.timeout_secs,
        options.confirmed,
        options.force_unprivileged,
    )
}

fn parse_restart_policy(value: &str) -> Result<ProcessRestartPolicy> {
    match value {
        "never" => Ok(ProcessRestartPolicy::Never),
        "on_failure" | "on-failure" => Ok(ProcessRestartPolicy::OnFailure),
        "always" => Ok(ProcessRestartPolicy::Always),
        _ => anyhow::bail!("process --restart-policy must be never, on-failure, or always"),
    }
}

pub(crate) fn process_stop(
    api_url: &str,
    token: Option<&str>,
    name: String,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    let operation = JobCommand::ProcessStop { name };
    submit_process_operation(
        api_url,
        token,
        &operation,
        "process_stop",
        clients,
        tags,
        password_env,
        super_salt_hex,
        privilege_ttl_secs,
        timeout_secs,
        confirmed,
        false,
    )
}

pub(crate) fn process_restart(
    api_url: &str,
    token: Option<&str>,
    name: String,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    let operation = JobCommand::ProcessRestart { name };
    submit_process_operation(
        api_url,
        token,
        &operation,
        "process_restart",
        clients,
        tags,
        password_env,
        super_salt_hex,
        privilege_ttl_secs,
        timeout_secs,
        confirmed,
        false,
    )
}

pub(crate) fn process_status(
    api_url: &str,
    token: Option<&str>,
    name: Option<String>,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    let operation = JobCommand::ProcessStatus { name };
    submit_process_operation(
        api_url,
        token,
        &operation,
        "process_status",
        clients,
        tags,
        password_env,
        super_salt_hex,
        privilege_ttl_secs,
        timeout_secs,
        confirmed,
        false,
    )
}

pub(crate) fn process_logs(
    api_url: &str,
    token: Option<&str>,
    name: String,
    max_bytes: u32,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        (1..=512 * 1024).contains(&max_bytes),
        "process logs --max-bytes must be between 1 and 524288"
    );
    let operation = JobCommand::ProcessLogs { name, max_bytes };
    submit_process_operation(
        api_url,
        token,
        &operation,
        "process_logs",
        clients,
        tags,
        password_env,
        super_salt_hex,
        privilege_ttl_secs,
        timeout_secs,
        confirmed,
        false,
    )
}

pub(crate) fn process_supervisor_inventory(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/process-supervisor/inventory?limit={}",
                limit.clamp(1, 200)
            ),
            token,
        )?
    );
    Ok(())
}

fn submit_process_operation(
    api_url: &str,
    token: Option<&str>,
    operation: &JobCommand,
    command_label: &str,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation,
            command_label,
            clients: &clients,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        })?
    );
    Ok(())
}

fn parse_env_pairs(values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for value in values {
        let (key, env_value) = value
            .split_once('=')
            .with_context(|| format!("invalid --env {value}; expected KEY=VALUE"))?;
        anyhow::ensure!(!key.is_empty(), "process env key is empty");
        env.insert(key.to_string(), env_value.to_string());
    }
    Ok(env)
}
