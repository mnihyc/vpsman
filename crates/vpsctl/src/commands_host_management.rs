use anyhow::Result;
use clap::Args;
use vpsman_common::{
    HostPackageProvider, HostServiceAction, HostServiceProvider, JobCommand,
    DEFAULT_MAX_JOB_TIMEOUT_SECS,
};

use crate::{
    http::http_get,
    jobs::{
        submit_privileged_operation, submit_unprivileged_operation, PrivilegedOperationRequest,
    },
    util::percent_encode_path_segment,
};

#[derive(Debug, Args)]
pub(crate) struct ReadOnlyTargets {
    #[arg(long, value_delimiter = ',')]
    clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    tags: Vec<String>,
    #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
    max_timeout_secs: u64,
}

#[derive(Debug, Args)]
pub(crate) struct PrivilegedTargets {
    #[arg(long, value_delimiter = ',')]
    clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    tags: Vec<String>,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    password_env: String,
    #[arg(long)]
    super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    privilege_ttl_secs: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
    max_timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct HostProcessRefreshCommand {
    #[arg(long, default_value_t = 200)]
    limit: u16,
    #[command(flatten)]
    targets: ReadOnlyTargets,
}

#[derive(Debug, Args)]
pub(crate) struct HostProcessViewCommand {
    #[arg(long)]
    client_id: String,
    #[arg(long, default_value_t = 200)]
    limit: u16,
}

#[derive(Debug, Args)]
pub(crate) struct HostServiceRefreshCommand {
    #[arg(long)]
    expected_provider: Option<String>,
    #[arg(long, default_value_t = 500)]
    limit: u16,
    #[command(flatten)]
    targets: ReadOnlyTargets,
}

#[derive(Debug, Args)]
pub(crate) struct HostServiceViewCommand {
    #[arg(long)]
    client_id: String,
    #[arg(long, default_value_t = 500)]
    limit: u16,
}

#[derive(Debug, Args)]
pub(crate) struct HostServiceLogsCommand {
    #[arg(long)]
    provider: String,
    #[arg(long)]
    service: String,
    #[arg(long, default_value_t = 500)]
    max_lines: u16,
    #[command(flatten)]
    targets: ReadOnlyTargets,
}

#[derive(Debug, Args)]
pub(crate) struct HostServiceActionCommand {
    #[arg(long)]
    provider: String,
    #[arg(long)]
    service: String,
    #[arg(long)]
    action: String,
    #[arg(long)]
    expected_active_state: String,
    #[arg(long)]
    expected_enabled_state: String,
    #[command(flatten)]
    targets: PrivilegedTargets,
}

#[derive(Debug, Args)]
pub(crate) struct OsUpdateCheckCommand {
    #[arg(long)]
    expected_provider: Option<String>,
    #[command(flatten)]
    targets: ReadOnlyTargets,
}

#[derive(Debug, Args)]
pub(crate) struct OsUpdateRefreshCommand {
    #[arg(long)]
    expected_provider: Option<String>,
    #[command(flatten)]
    targets: PrivilegedTargets,
}

#[derive(Debug, Args)]
pub(crate) struct OsUpdatePlanCommand {
    #[arg(long)]
    client_id: String,
}

#[derive(Debug, Args)]
pub(crate) struct OsUpdateApplyCommand {
    #[arg(long)]
    client_id: String,
    #[arg(long)]
    provider: String,
    #[arg(long)]
    plan_hash: String,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    password_env: String,
    #[arg(long)]
    super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    privilege_ttl_secs: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
    max_timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    confirmed: bool,
}

pub(crate) fn host_process_refresh(
    api_url: &str,
    token: Option<&str>,
    command: HostProcessRefreshCommand,
) -> Result<()> {
    anyhow::ensure!(
        (1..=512).contains(&command.limit),
        "host process limit must be between 1 and 512"
    );
    submit_read_only(
        api_url,
        token,
        JobCommand::ProcessList {
            limit: command.limit,
        },
        "process_list",
        command.targets,
    )
}

pub(crate) fn host_processes(
    api_url: &str,
    token: Option<&str>,
    command: HostProcessViewCommand,
) -> Result<()> {
    anyhow::ensure!(
        (1..=512).contains(&command.limit),
        "host process limit must be between 1 and 512"
    );
    print_client_view(
        api_url,
        token,
        "/api/v1/host-processes",
        &command.client_id,
        Some(command.limit),
    )
}

pub(crate) fn host_service_refresh(
    api_url: &str,
    token: Option<&str>,
    command: HostServiceRefreshCommand,
) -> Result<()> {
    anyhow::ensure!(
        (1..=1024).contains(&command.limit),
        "host service limit must be between 1 and 1024"
    );
    let expected_provider = command
        .expected_provider
        .as_deref()
        .map(parse_service_provider)
        .transpose()?;
    submit_read_only(
        api_url,
        token,
        JobCommand::ServiceInventory {
            expected_provider,
            limit: command.limit,
        },
        "service_inventory",
        command.targets,
    )
}

pub(crate) fn host_services(
    api_url: &str,
    token: Option<&str>,
    command: HostServiceViewCommand,
) -> Result<()> {
    anyhow::ensure!(
        (1..=1024).contains(&command.limit),
        "host service limit must be between 1 and 1024"
    );
    print_client_view(
        api_url,
        token,
        "/api/v1/host-services",
        &command.client_id,
        Some(command.limit),
    )
}

pub(crate) fn host_service_logs(
    api_url: &str,
    token: Option<&str>,
    command: HostServiceLogsCommand,
) -> Result<()> {
    anyhow::ensure!(
        (1..=2000).contains(&command.max_lines),
        "host service log limit must be between 1 and 2000"
    );
    let operation = JobCommand::ServiceLogs {
        provider: parse_service_provider(&command.provider)?,
        service: required_value(&command.service, "host service name")?,
        max_lines: command.max_lines,
    };
    submit_read_only(api_url, token, operation, "service_logs", command.targets)
}

pub(crate) fn host_service_action(
    api_url: &str,
    token: Option<&str>,
    command: HostServiceActionCommand,
) -> Result<()> {
    let operation = JobCommand::ServiceAction {
        provider: parse_service_provider(&command.provider)?,
        service: required_value(&command.service, "host service name")?,
        action: parse_service_action(&command.action)?,
        expected_active_state: required_value(
            &command.expected_active_state,
            "expected active state",
        )?,
        expected_enabled_state: required_value(
            &command.expected_enabled_state,
            "expected enabled state",
        )?,
    };
    submit_privileged(
        api_url,
        token,
        operation,
        "service_action",
        command.targets,
        "host-service-action",
    )
}

pub(crate) fn os_update_check(
    api_url: &str,
    token: Option<&str>,
    command: OsUpdateCheckCommand,
) -> Result<()> {
    let operation = JobCommand::PackageUpdatePlan {
        expected_provider: command
            .expected_provider
            .as_deref()
            .map(parse_package_provider)
            .transpose()?,
        refresh_metadata: false,
    };
    submit_read_only(
        api_url,
        token,
        operation,
        "package_update_plan",
        command.targets,
    )
}

pub(crate) fn os_update_refresh(
    api_url: &str,
    token: Option<&str>,
    command: OsUpdateRefreshCommand,
) -> Result<()> {
    let operation = JobCommand::PackageUpdatePlan {
        expected_provider: command
            .expected_provider
            .as_deref()
            .map(parse_package_provider)
            .transpose()?,
        refresh_metadata: true,
    };
    submit_privileged(
        api_url,
        token,
        operation,
        "package_update_plan",
        command.targets,
        "os-update-refresh",
    )
}

pub(crate) fn os_update_plans(api_url: &str, token: Option<&str>) -> Result<()> {
    println!(
        "{}",
        http_get(api_url, "/api/v1/host-package-updates", token)?
    );
    Ok(())
}

pub(crate) fn os_update_plan(
    api_url: &str,
    token: Option<&str>,
    command: OsUpdatePlanCommand,
) -> Result<()> {
    print_client_view(
        api_url,
        token,
        "/api/v1/host-package-updates",
        &command.client_id,
        None,
    )
}

pub(crate) fn os_update_apply(
    api_url: &str,
    token: Option<&str>,
    command: OsUpdateApplyCommand,
) -> Result<()> {
    let client_id = required_value(&command.client_id, "OS update client ID")?;
    let plan_hash = required_value(&command.plan_hash, "OS update plan hash")?;
    anyhow::ensure!(
        plan_hash.len() == 64 && plan_hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "OS update plan hash must be 64 hexadecimal characters"
    );
    let operation = JobCommand::PackageUpdateApply {
        provider: parse_package_provider(&command.provider)?,
        plan_hash,
    };
    submit_privileged(
        api_url,
        token,
        operation,
        "package_update_apply",
        PrivilegedTargets {
            clients: vec![client_id],
            tags: Vec::new(),
            password_env: command.password_env,
            super_salt_hex: command.super_salt_hex,
            privilege_ttl_secs: command.privilege_ttl_secs,
            max_timeout_secs: command.max_timeout_secs,
            confirmed: command.confirmed,
        },
        "os-update-apply",
    )
}

fn submit_read_only(
    api_url: &str,
    token: Option<&str>,
    operation: JobCommand,
    command_label: &str,
    targets: ReadOnlyTargets,
) -> Result<()> {
    println!(
        "{}",
        submit_unprivileged_operation(
            api_url,
            token,
            &operation,
            command_label,
            &targets.clients,
            &targets.tags,
            targets.max_timeout_secs,
        )?
    );
    Ok(())
}

fn submit_privileged(
    api_url: &str,
    token: Option<&str>,
    operation: JobCommand,
    command_label: &str,
    targets: PrivilegedTargets,
    cli_name: &str,
) -> Result<()> {
    anyhow::ensure!(
        targets.confirmed,
        "{cli_name} requires --confirmed after reviewing the current snapshot"
    );
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label,
            clients: &targets.clients,
            tags: &targets.tags,
            password_env: &targets.password_env,
            super_salt_hex: targets.super_salt_hex.as_deref(),
            privilege_ttl_secs: targets.privilege_ttl_secs,
            max_timeout_secs: targets.max_timeout_secs,
            confirmed: true,
            force_unprivileged: false,
        })?
    );
    Ok(())
}

fn print_client_view(
    api_url: &str,
    token: Option<&str>,
    route: &str,
    client_id: &str,
    limit: Option<u16>,
) -> Result<()> {
    let client_id = client_id.trim();
    anyhow::ensure!(!client_id.is_empty(), "client ID is required");
    let mut path = format!("{route}/{}", percent_encode_path_segment(client_id));
    if let Some(limit) = limit {
        path.push_str(&format!("?limit={limit}"));
    }
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

fn required_value(value: &str, label: &str) -> Result<String> {
    let value = value.trim();
    anyhow::ensure!(!value.is_empty(), "{label} is required");
    Ok(value.to_string())
}

fn parse_service_provider(value: &str) -> Result<HostServiceProvider> {
    match value.trim().to_ascii_lowercase().as_str() {
        "systemd" => Ok(HostServiceProvider::Systemd),
        "openrc" => Ok(HostServiceProvider::Openrc),
        "sysv" => Ok(HostServiceProvider::Sysv),
        _ => anyhow::bail!("service provider must be systemd, openrc, or sysv"),
    }
}

fn parse_service_action(value: &str) -> Result<HostServiceAction> {
    match value.trim().to_ascii_lowercase().as_str() {
        "start" => Ok(HostServiceAction::Start),
        "stop" => Ok(HostServiceAction::Stop),
        "restart" => Ok(HostServiceAction::Restart),
        "enable" => Ok(HostServiceAction::Enable),
        "disable" => Ok(HostServiceAction::Disable),
        _ => anyhow::bail!("service action must be start, stop, restart, enable, or disable"),
    }
}

fn parse_package_provider(value: &str) -> Result<HostPackageProvider> {
    match value.trim().to_ascii_lowercase().as_str() {
        "apt" => Ok(HostPackageProvider::Apt),
        "dnf" => Ok(HostPackageProvider::Dnf),
        "yum" => Ok(HostPackageProvider::Yum),
        "pacman" => Ok(HostPackageProvider::Pacman),
        _ => anyhow::bail!("package provider must be apt, dnf, yum, or pacman"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_and_action_names_are_explicit() {
        assert_eq!(
            parse_service_provider("openrc").unwrap(),
            HostServiceProvider::Openrc
        );
        assert_eq!(
            parse_service_action("restart").unwrap(),
            HostServiceAction::Restart
        );
        assert_eq!(
            parse_package_provider("yum").unwrap(),
            HostPackageProvider::Yum
        );
        assert!(parse_service_provider("auto").is_err());
        assert!(parse_package_provider("auto").is_err());
    }
}
