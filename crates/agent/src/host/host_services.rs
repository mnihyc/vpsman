use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::{process::Command, sync::Semaphore, task::JoinSet, time};
use vpsman_common::{
    CommandOutput, HostServiceAction, HostServiceActionResult, HostServiceCapability,
    HostServiceCapabilityStatus, HostServiceLogSnapshot, HostServiceProvider, HostServiceRecord,
    HostServiceSnapshot, OutputStream,
};

use crate::{
    child_process::{run_child_with_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult},
    command_worker::CommandCancelToken,
};

const MAX_SERVICE_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_SERVICE_OUTPUT_CHUNK_BYTES: usize = 64 * 1024;
const SERVICE_STATUS_CONCURRENCY: usize = 8;

#[derive(Clone, Debug)]
struct ServiceEnvironment {
    proc_root: PathBuf,
    run_root: PathBuf,
    etc_root: PathBuf,
    effective_uid: u32,
    systemctl: Option<PathBuf>,
    journalctl: Option<PathBuf>,
    rc_service: Option<PathBuf>,
    rc_status: Option<PathBuf>,
    rc_update: Option<PathBuf>,
    service: Option<PathBuf>,
    update_rc_d: Option<PathBuf>,
    chkconfig: Option<PathBuf>,
}

impl ServiceEnvironment {
    fn discover() -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
            run_root: PathBuf::from("/run"),
            etc_root: PathBuf::from("/etc"),
            effective_uid: unsafe { libc::geteuid() } as u32,
            systemctl: resolve_executable(&[
                "/usr/bin/systemctl",
                "/bin/systemctl",
                "/usr/sbin/systemctl",
                "/sbin/systemctl",
            ]),
            journalctl: resolve_executable(&[
                "/usr/bin/journalctl",
                "/bin/journalctl",
                "/usr/sbin/journalctl",
                "/sbin/journalctl",
            ]),
            rc_service: resolve_executable(&[
                "/usr/bin/rc-service",
                "/bin/rc-service",
                "/usr/sbin/rc-service",
                "/sbin/rc-service",
            ]),
            rc_status: resolve_executable(&[
                "/usr/bin/rc-status",
                "/bin/rc-status",
                "/usr/sbin/rc-status",
                "/sbin/rc-status",
            ]),
            rc_update: resolve_executable(&[
                "/usr/bin/rc-update",
                "/bin/rc-update",
                "/usr/sbin/rc-update",
                "/sbin/rc-update",
            ]),
            service: resolve_executable(&[
                "/usr/sbin/service",
                "/sbin/service",
                "/usr/bin/service",
                "/bin/service",
            ]),
            update_rc_d: resolve_executable(&["/usr/sbin/update-rc.d", "/sbin/update-rc.d"]),
            chkconfig: resolve_executable(&["/usr/sbin/chkconfig", "/sbin/chkconfig"]),
        }
    }

    fn init_dir(&self) -> PathBuf {
        self.etc_root.join("init.d")
    }
}

#[derive(Clone, Debug)]
struct CommandResult {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

pub(crate) async fn execute_service_inventory(
    job_id: uuid::Uuid,
    expected_provider: Option<HostServiceProvider>,
    limit: u16,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    let environment = ServiceEnvironment::discover();
    let mut snapshot = time::timeout(
        Duration::from_secs(max_timeout_secs.max(1)),
        collect_service_snapshot(
            environment,
            expected_provider,
            limit.clamp(1, 1024),
            cancel_token,
        ),
    )
    .await
    .context("service inventory timed out")??;
    snapshot.r#type = "service_inventory".to_string();
    let payload = serde_json::to_vec(&snapshot)?;
    let mut outputs = chunked_output(job_id, OutputStream::Stdout, &payload);
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "service_inventory",
            "capability_status": snapshot.capability.status,
            "provider": snapshot.capability.provider,
            "count": snapshot.services.len(),
            "truncated": snapshot.truncated,
        }))?,
        exit_code: Some(0),
        done: true,
    });
    Ok(outputs)
}

pub(crate) async fn execute_service_action(
    job_id: uuid::Uuid,
    provider: HostServiceProvider,
    service: &str,
    action: HostServiceAction,
    expected_active_state: &str,
    expected_enabled_state: &str,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    let environment = ServiceEnvironment::discover();
    let result = time::timeout(
        Duration::from_secs(max_timeout_secs.max(1)),
        apply_service_action(
            &environment,
            provider,
            service,
            action,
            expected_active_state,
            expected_enabled_state,
            max_timeout_secs,
            cancel_token,
        ),
    )
    .await
    .context("service action timed out")??;
    let payload = serde_json::to_vec(&result)?;
    let mut outputs = chunked_output(job_id, OutputStream::Stdout, &payload);
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "service_action",
            "provider": provider,
            "service": service,
            "action": action,
            "active_state": result.after.active_state,
            "enabled_state": result.after.enabled_state,
        }))?,
        exit_code: Some(0),
        done: true,
    });
    Ok(outputs)
}

pub(crate) async fn execute_service_logs(
    job_id: uuid::Uuid,
    provider: HostServiceProvider,
    service: &str,
    max_lines: u16,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    let environment = ServiceEnvironment::discover();
    let snapshot = time::timeout(
        Duration::from_secs(max_timeout_secs.max(1)),
        collect_service_logs(
            &environment,
            provider,
            service,
            max_lines.clamp(1, 2000),
            max_timeout_secs,
            cancel_token,
        ),
    )
    .await
    .context("service logs timed out")??;
    let payload = serde_json::to_vec(&snapshot)?;
    let mut outputs = chunked_output(job_id, OutputStream::Stdout, &payload);
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "service_logs",
            "provider": provider,
            "service": service,
            "line_count": snapshot.lines.len(),
            "truncated": snapshot.truncated,
        }))?,
        exit_code: Some(0),
        done: true,
    });
    Ok(outputs)
}

async fn collect_service_snapshot(
    environment: ServiceEnvironment,
    expected_provider: Option<HostServiceProvider>,
    limit: u16,
    cancel_token: CommandCancelToken,
) -> Result<HostServiceSnapshot> {
    cancel_token.check("service_inventory")?;
    let capability = probe_service_capability(&environment).await;
    if let Some(expected_provider) = expected_provider {
        if capability.provider != Some(expected_provider) {
            anyhow::bail!(
                "host_service_provider_changed: expected {}, observed {}: {}",
                provider_token(expected_provider),
                capability
                    .provider
                    .map(provider_token)
                    .unwrap_or("unsupported"),
                capability
                    .reason
                    .as_deref()
                    .unwrap_or("provider probe returned no reason")
            );
        }
    }
    if !capability.supported() {
        return Ok(HostServiceSnapshot {
            r#type: "service_inventory".to_string(),
            capability,
            truncated: false,
            services: Vec::new(),
        });
    }
    let provider = capability.provider.context("supported provider missing")?;
    let mut services = match provider {
        HostServiceProvider::Systemd => {
            collect_systemd_services(&environment, cancel_token.clone()).await?
        }
        HostServiceProvider::Openrc | HostServiceProvider::Sysv => {
            collect_script_services(&environment, provider, cancel_token.clone()).await?
        }
    };
    services.sort_by(|left, right| left.name.cmp(&right.name));
    let limit = limit as usize;
    let truncated = services.len() > limit;
    services.truncate(limit);
    cancel_token.check("service_inventory")?;
    Ok(HostServiceSnapshot {
        r#type: "service_inventory".to_string(),
        capability,
        truncated,
        services,
    })
}

async fn probe_service_capability(environment: &ServiceEnvironment) -> HostServiceCapability {
    let pid1_path = environment.proc_root.join("1/comm");
    let pid1 = match std::fs::read_to_string(&pid1_path) {
        Ok(value) => value.trim().to_ascii_lowercase(),
        Err(error) => {
            return HostServiceCapability {
                status: HostServiceCapabilityStatus::ProbeFailed,
                reason: Some(format!("cannot read {}: {error}", pid1_path.display())),
                ..HostServiceCapability::default()
            };
        }
    };
    let systemd_marker = environment.run_root.join("systemd/system").is_dir();
    let openrc_marker = environment.run_root.join("openrc").exists();
    if systemd_marker && openrc_marker {
        return HostServiceCapability {
            status: HostServiceCapabilityStatus::Ambiguous,
            reason: Some(
                "both /run/systemd/system and /run/openrc are present; select no provider until the host init state is unambiguous"
                    .to_string(),
            ),
            ..HostServiceCapability::default()
        };
    }

    let provider = if pid1 == "systemd" && systemd_marker && environment.systemctl.is_some() {
        Some(HostServiceProvider::Systemd)
    } else if matches!(pid1.as_str(), "init" | "openrc" | "openrc-init")
        && openrc_marker
        && environment.rc_service.is_some()
        && environment.rc_status.is_some()
    {
        Some(HostServiceProvider::Openrc)
    } else if pid1 == "init"
        && !systemd_marker
        && !openrc_marker
        && environment.init_dir().is_dir()
        && environment.service.is_some()
    {
        Some(HostServiceProvider::Sysv)
    } else {
        None
    };
    let Some(provider) = provider else {
        return HostServiceCapability {
            status: HostServiceCapabilityStatus::Unsupported,
            reason: Some(format!(
                "PID 1 is {pid1:?}; no active supported provider was confirmed (systemd, OpenRC, or SysV init)"
            )),
            ..HostServiceCapability::default()
        };
    };

    let root = environment.effective_uid == 0;
    let (can_enable_disable, enable_backend) = match provider {
        HostServiceProvider::Systemd => (root, Some("systemctl".to_string())),
        HostServiceProvider::Openrc => (
            root && environment.rc_update.is_some(),
            environment
                .rc_update
                .as_ref()
                .map(|_| "rc-update".to_string()),
        ),
        HostServiceProvider::Sysv => {
            let update_rc_d = environment.update_rc_d.is_some();
            let chkconfig = environment.chkconfig.is_some();
            match (root, update_rc_d, chkconfig) {
                (true, true, false) => (true, Some("update-rc.d".to_string())),
                (true, false, true) => (true, Some("chkconfig".to_string())),
                (true, true, true) => (false, Some("ambiguous".to_string())),
                _ => (false, None),
            }
        }
    };
    let reason = if root {
        match (provider, can_enable_disable) {
            (HostServiceProvider::Sysv, false)
                if environment.update_rc_d.is_some() && environment.chkconfig.is_some() =>
            {
                Some(
                    "SysV start/stop/restart is supported; enable/disable is unavailable because both update-rc.d and chkconfig are present"
                        .to_string(),
                )
            }
            (_, false) => Some(
                "service inventory and runtime actions are supported; no unambiguous enable/disable backend is available"
                    .to_string(),
            ),
            _ => None,
        }
    } else {
        Some(format!(
            "service inventory is supported; mutations require root (effective UID {})",
            environment.effective_uid
        ))
    };
    HostServiceCapability {
        status: HostServiceCapabilityStatus::Supported,
        provider: Some(provider),
        provider_version: None,
        can_inventory: true,
        can_start_stop_restart: root,
        can_enable_disable,
        can_read_logs: provider == HostServiceProvider::Systemd && environment.journalctl.is_some(),
        enable_backend,
        reason,
    }
}

async fn collect_systemd_services(
    environment: &ServiceEnvironment,
    cancel_token: CommandCancelToken,
) -> Result<Vec<HostServiceRecord>> {
    let systemctl = environment
        .systemctl
        .as_ref()
        .context("systemctl disappeared after provider probe")?;
    let units = run_command(
        systemctl,
        &[
            "list-units",
            "--type=service",
            "--all",
            "--no-legend",
            "--no-pager",
            "--plain",
        ],
        20,
        MAX_SERVICE_OUTPUT_BYTES,
        cancel_token.clone(),
    )
    .await?;
    ensure_command_success("systemctl list-units", &units)?;
    let files = run_command(
        systemctl,
        &[
            "list-unit-files",
            "--type=service",
            "--no-legend",
            "--no-pager",
        ],
        20,
        MAX_SERVICE_OUTPUT_BYTES,
        cancel_token,
    )
    .await?;
    ensure_command_success("systemctl list-unit-files", &files)?;
    if units.stdout_truncated || files.stdout_truncated {
        anyhow::bail!("service_inventory_output_limit_exceeded: systemctl output exceeded 2 MiB");
    }
    Ok(parse_systemd_inventory(&units.stdout, &files.stdout))
}

fn parse_systemd_inventory(units: &[u8], unit_files: &[u8]) -> Vec<HostServiceRecord> {
    let mut records = BTreeMap::<String, HostServiceRecord>::new();
    for line in String::from_utf8_lossy(units).lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 4 || !fields[0].ends_with(".service") {
            continue;
        }
        records.insert(
            fields[0].to_string(),
            HostServiceRecord {
                name: fields[0].to_string(),
                description: fields.get(4..).unwrap_or_default().join(" "),
                load_state: fields[1].to_string(),
                active_state: fields[2].to_string(),
                sub_state: fields[3].to_string(),
                enabled_state: "unknown".to_string(),
                state_reason: None,
            },
        );
    }
    for line in String::from_utf8_lossy(unit_files).lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 2 || !fields[0].ends_with(".service") {
            continue;
        }
        let record = records
            .entry(fields[0].to_string())
            .or_insert_with(|| HostServiceRecord {
                name: fields[0].to_string(),
                description: String::new(),
                load_state: "not-loaded".to_string(),
                active_state: "inactive".to_string(),
                sub_state: "not-loaded".to_string(),
                enabled_state: "unknown".to_string(),
                state_reason: Some("unit file is installed but not loaded".to_string()),
            });
        record.enabled_state = fields[1].to_string();
    }
    records.into_values().collect()
}

async fn collect_script_services(
    environment: &ServiceEnvironment,
    provider: HostServiceProvider,
    cancel_token: CommandCancelToken,
) -> Result<Vec<HostServiceRecord>> {
    let names = list_init_script_names(&environment.init_dir())?;
    let enabled = match provider {
        HostServiceProvider::Openrc => {
            openrc_enabled_services(environment, cancel_token.clone()).await?
        }
        HostServiceProvider::Sysv => {
            sysv_enabled_services(environment, cancel_token.clone()).await?
        }
        HostServiceProvider::Systemd => BTreeSet::new(),
    };
    let semaphore = Arc::new(Semaphore::new(SERVICE_STATUS_CONCURRENCY));
    let mut tasks = JoinSet::new();
    for name in names {
        let permit = semaphore.clone().acquire_owned().await?;
        let environment = environment.clone();
        let cancel_token = cancel_token.clone();
        let enabled_state = if enabled.contains(&name) {
            "enabled"
        } else {
            "disabled"
        }
        .to_string();
        tasks.spawn(async move {
            let _permit = permit;
            inspect_script_service(&environment, provider, &name, enabled_state, cancel_token).await
        });
    }
    let mut records = Vec::new();
    while let Some(result) = tasks.join_next().await {
        records.push(result.context("service status task failed")??);
    }
    Ok(records)
}

fn list_init_script_names(init_dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(init_dir)
        .with_context(|| format!("failed to read {}", init_dir.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_portable_service_name(&name)
            || matches!(
                name.as_str(),
                "README" | "functions" | "skeleton" | "rc" | "rcS"
            )
        {
            continue;
        }
        let metadata = entry.metadata()?;
        if metadata.is_file() || metadata.permissions().mode() & 0o111 != 0 {
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

async fn inspect_script_service(
    environment: &ServiceEnvironment,
    provider: HostServiceProvider,
    name: &str,
    enabled_state: String,
    cancel_token: CommandCancelToken,
) -> Result<HostServiceRecord> {
    let (binary, args): (&Path, Vec<&str>) = match provider {
        HostServiceProvider::Openrc => (
            environment
                .rc_service
                .as_deref()
                .context("rc-service disappeared after provider probe")?,
            vec![name, "status"],
        ),
        HostServiceProvider::Sysv => (
            environment
                .service
                .as_deref()
                .context("service command disappeared after provider probe")?,
            vec![name, "status"],
        ),
        HostServiceProvider::Systemd => anyhow::bail!("script service provider required"),
    };
    let result = run_command(binary, &args, 4, 16 * 1024, cancel_token).await?;
    let (active_state, sub_state) = script_service_state(&result);
    Ok(HostServiceRecord {
        name: name.to_string(),
        description: String::new(),
        load_state: "loaded".to_string(),
        active_state,
        sub_state,
        enabled_state,
        state_reason: first_diagnostic_line(&result),
    })
}

fn script_service_state(result: &CommandResult) -> (String, String) {
    let output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    )
    .to_ascii_lowercase();
    match result.exit_code {
        Some(0) => ("active".to_string(), "running".to_string()),
        Some(3) if !output.contains("crash") && !output.contains("fail") => {
            ("inactive".to_string(), "stopped".to_string())
        }
        Some(_) if output.contains("not running") || output.contains("stopped") => {
            ("inactive".to_string(), "stopped".to_string())
        }
        Some(_) if output.contains("crash") || output.contains("fail") => {
            ("failed".to_string(), "failed".to_string())
        }
        _ => ("unknown".to_string(), "status-unknown".to_string()),
    }
}

async fn openrc_enabled_services(
    environment: &ServiceEnvironment,
    cancel_token: CommandCancelToken,
) -> Result<BTreeSet<String>> {
    let Some(rc_update) = environment.rc_update.as_deref() else {
        return Ok(BTreeSet::new());
    };
    let result = run_command(rc_update, &["show"], 10, 256 * 1024, cancel_token).await?;
    ensure_command_success("rc-update show", &result)?;
    Ok(String::from_utf8_lossy(&result.stdout)
        .lines()
        .filter_map(|line| {
            line.split_once('|')
                .map(|(name, _)| name.trim().to_string())
        })
        .filter(|name| is_portable_service_name(name))
        .collect())
}

async fn sysv_enabled_services(
    environment: &ServiceEnvironment,
    cancel_token: CommandCancelToken,
) -> Result<BTreeSet<String>> {
    if let Some(chkconfig) = environment.chkconfig.as_deref() {
        let result = run_command(chkconfig, &["--list"], 10, 512 * 1024, cancel_token).await?;
        ensure_command_success("chkconfig --list", &result)?;
        return Ok(parse_chkconfig_enabled(&result.stdout));
    }
    let mut enabled = BTreeSet::new();
    for runlevel in 2..=5 {
        let directory = environment.etc_root.join(format!("rc{runlevel}.d"));
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('S') && name.len() > 3 {
                let service = name[3..].to_string();
                if is_portable_service_name(&service) {
                    enabled.insert(service);
                }
            }
        }
    }
    Ok(enabled)
}

fn parse_chkconfig_enabled(output: &[u8]) -> BTreeSet<String> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let name = fields.first()?.to_string();
            (is_portable_service_name(&name)
                && fields.iter().skip(1).any(|field| {
                    field.split_once(':').is_some_and(|(level, state)| {
                        matches!(level, "2" | "3" | "4" | "5") && state == "on"
                    })
                }))
            .then_some(name)
        })
        .collect()
}

async fn apply_service_action(
    environment: &ServiceEnvironment,
    provider: HostServiceProvider,
    service: &str,
    action: HostServiceAction,
    expected_active_state: &str,
    expected_enabled_state: &str,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<HostServiceActionResult> {
    let capability = probe_service_capability(environment).await;
    require_provider(&capability, provider)?;
    if !is_portable_service_name(service)
        || (provider == HostServiceProvider::Systemd && !service.ends_with(".service"))
    {
        anyhow::bail!("host_service_name_invalid: {service:?}");
    }
    let mutation_supported = match action {
        HostServiceAction::Start | HostServiceAction::Stop | HostServiceAction::Restart => {
            capability.can_start_stop_restart
        }
        HostServiceAction::Enable | HostServiceAction::Disable => capability.can_enable_disable,
    };
    if !mutation_supported {
        anyhow::bail!(
            "host_service_action_unsupported: {}: {}",
            action_token(action),
            capability
                .reason
                .as_deref()
                .unwrap_or("agent capability does not permit this action")
        );
    }
    let before = inspect_one_service(environment, provider, service, cancel_token.clone()).await?;
    if before.active_state != expected_active_state
        || before.enabled_state != expected_enabled_state
    {
        anyhow::bail!(
            "host_service_confirmation_stale: expected active={} enabled={}, observed active={} enabled={}",
            expected_active_state,
            expected_enabled_state,
            before.active_state,
            before.enabled_state
        );
    }
    let (binary, args) = action_command(environment, provider, service, action)?;
    let result = run_command(
        binary,
        &args,
        max_timeout_secs.clamp(1, 120),
        256 * 1024,
        cancel_token.clone(),
    )
    .await?;
    ensure_command_success("service action", &result)?;
    let after = inspect_one_service(environment, provider, service, cancel_token).await?;
    Ok(HostServiceActionResult {
        r#type: "service_action".to_string(),
        provider,
        service: service.to_string(),
        action,
        before,
        after,
    })
}

async fn inspect_one_service(
    environment: &ServiceEnvironment,
    provider: HostServiceProvider,
    service: &str,
    cancel_token: CommandCancelToken,
) -> Result<HostServiceRecord> {
    match provider {
        HostServiceProvider::Systemd => {
            let systemctl = environment
                .systemctl
                .as_deref()
                .context("systemctl disappeared after provider probe")?;
            let result = run_command(
                systemctl,
                &[
                    "show",
                    service,
                    "--no-pager",
                    "--property=Id,Description,LoadState,ActiveState,SubState,UnitFileState",
                ],
                10,
                64 * 1024,
                cancel_token,
            )
            .await?;
            ensure_command_success("systemctl show", &result)?;
            parse_systemd_show(service, &result.stdout)
        }
        HostServiceProvider::Openrc => {
            let enabled = openrc_enabled_services(environment, cancel_token.clone()).await?;
            inspect_script_service(
                environment,
                provider,
                service,
                if enabled.contains(service) {
                    "enabled"
                } else {
                    "disabled"
                }
                .to_string(),
                cancel_token,
            )
            .await
        }
        HostServiceProvider::Sysv => {
            let enabled = sysv_enabled_services(environment, cancel_token.clone()).await?;
            inspect_script_service(
                environment,
                provider,
                service,
                if enabled.contains(service) {
                    "enabled"
                } else {
                    "disabled"
                }
                .to_string(),
                cancel_token,
            )
            .await
        }
    }
}

fn parse_systemd_show(service: &str, output: &[u8]) -> Result<HostServiceRecord> {
    let values = String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<BTreeMap<_, _>>();
    let name = values
        .get("Id")
        .cloned()
        .unwrap_or_else(|| service.to_string());
    let load_state = values
        .get("LoadState")
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    if load_state == "not-found" {
        anyhow::bail!("host_service_not_found: {service}");
    }
    Ok(HostServiceRecord {
        name,
        description: values.get("Description").cloned().unwrap_or_default(),
        load_state,
        active_state: values
            .get("ActiveState")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        sub_state: values
            .get("SubState")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        enabled_state: values
            .get("UnitFileState")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        state_reason: None,
    })
}

fn action_command<'a>(
    environment: &'a ServiceEnvironment,
    provider: HostServiceProvider,
    service: &'a str,
    action: HostServiceAction,
) -> Result<(&'a Path, Vec<&'a str>)> {
    let action_name = action_token(action);
    match (provider, action) {
        (HostServiceProvider::Systemd, _) => Ok((
            environment
                .systemctl
                .as_deref()
                .context("systemctl disappeared after provider probe")?,
            vec![action_name, service],
        )),
        (HostServiceProvider::Openrc, HostServiceAction::Enable) => Ok((
            environment
                .rc_update
                .as_deref()
                .context("rc-update unavailable")?,
            vec!["add", service, "default"],
        )),
        (HostServiceProvider::Openrc, HostServiceAction::Disable) => Ok((
            environment
                .rc_update
                .as_deref()
                .context("rc-update unavailable")?,
            vec!["del", service, "default"],
        )),
        (HostServiceProvider::Openrc, _) => Ok((
            environment
                .rc_service
                .as_deref()
                .context("rc-service unavailable")?,
            vec![service, action_name],
        )),
        (HostServiceProvider::Sysv, HostServiceAction::Enable) => {
            sysv_enable_command(environment, service, true)
        }
        (HostServiceProvider::Sysv, HostServiceAction::Disable) => {
            sysv_enable_command(environment, service, false)
        }
        (HostServiceProvider::Sysv, _) => Ok((
            environment
                .service
                .as_deref()
                .context("service unavailable")?,
            vec![service, action_name],
        )),
    }
}

fn sysv_enable_command<'a>(
    environment: &'a ServiceEnvironment,
    service: &'a str,
    enable: bool,
) -> Result<(&'a Path, Vec<&'a str>)> {
    match (
        environment.update_rc_d.as_deref(),
        environment.chkconfig.as_deref(),
    ) {
        (Some(update_rc_d), None) => Ok((
            update_rc_d,
            vec![service, if enable { "enable" } else { "disable" }],
        )),
        (None, Some(chkconfig)) => {
            Ok((chkconfig, vec![service, if enable { "on" } else { "off" }]))
        }
        (Some(_), Some(_)) => anyhow::bail!("host_service_enable_backend_ambiguous"),
        (None, None) => anyhow::bail!("host_service_enable_backend_unsupported"),
    }
}

async fn collect_service_logs(
    environment: &ServiceEnvironment,
    provider: HostServiceProvider,
    service: &str,
    max_lines: u16,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<HostServiceLogSnapshot> {
    let capability = probe_service_capability(environment).await;
    require_provider(&capability, provider)?;
    if !capability.can_read_logs {
        anyhow::bail!(
            "host_service_logs_unsupported: {} does not expose a portable per-service log backend",
            provider_token(provider)
        );
    }
    if !is_portable_service_name(service)
        || (provider == HostServiceProvider::Systemd && !service.ends_with(".service"))
    {
        anyhow::bail!("host_service_name_invalid: {service:?}");
    }
    let journalctl = environment
        .journalctl
        .as_deref()
        .context("journalctl disappeared after provider probe")?;
    let line_count = max_lines.to_string();
    let result = run_command(
        journalctl,
        &[
            "--unit",
            service,
            "--no-pager",
            "--output=short-iso",
            "--lines",
            &line_count,
        ],
        max_timeout_secs.clamp(1, 60),
        512 * 1024,
        cancel_token,
    )
    .await?;
    ensure_command_success("journalctl", &result)?;
    let mut lines = String::from_utf8_lossy(&result.stdout)
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let truncated = result.stdout_truncated || lines.len() > max_lines as usize;
    if lines.len() > max_lines as usize {
        lines = lines.split_off(lines.len() - max_lines as usize);
    }
    Ok(HostServiceLogSnapshot {
        r#type: "service_logs".to_string(),
        provider,
        service: service.to_string(),
        truncated,
        lines,
    })
}

fn require_provider(
    capability: &HostServiceCapability,
    expected: HostServiceProvider,
) -> Result<()> {
    if capability.provider != Some(expected) || !capability.supported() {
        anyhow::bail!(
            "host_service_provider_mismatch: expected {}, observed {}: {}",
            provider_token(expected),
            capability
                .provider
                .map(provider_token)
                .unwrap_or("unsupported"),
            capability
                .reason
                .as_deref()
                .unwrap_or("provider probe returned no reason")
        );
    }
    Ok(())
}

async fn run_command(
    binary: &Path,
    args: &[&str],
    timeout_secs: u64,
    max_output_bytes: usize,
    cancel_token: CommandCancelToken,
) -> Result<CommandResult> {
    let mut command = Command::new(binary);
    command.args(args);
    command.env("LC_ALL", "C");
    command.env("LANG", "C");
    command.env("SYSTEMD_COLORS", "0");
    command.stdin(Stdio::null());
    match run_child_with_bounded_output_cancelable(
        command,
        timeout_secs.max(1),
        max_output_bytes,
        ChildCleanupPolicy::ProcessGroup,
        cancel_token,
    )
    .await
    .with_context(|| format!("failed to execute {}", binary.display()))?
    {
        ChildRunResult::Completed(output) => Ok(CommandResult {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
            stdout_truncated: output.stdout_truncated,
            stderr_truncated: output.stderr_truncated,
        }),
        ChildRunResult::TimedOut(_) => {
            anyhow::bail!("host_service_command_timed_out: {}", binary.display())
        }
        ChildRunResult::Canceled { reason, .. } => {
            anyhow::bail!("host_service_command_canceled: {reason}")
        }
    }
}

fn ensure_command_success(label: &str, result: &CommandResult) -> Result<()> {
    if result.exit_code == Some(0) && !result.stderr_truncated {
        return Ok(());
    }
    let diagnostic = first_diagnostic_line(result)
        .unwrap_or_else(|| format!("exit code {:?}", result.exit_code));
    anyhow::bail!("host_service_command_failed: {label}: {diagnostic}")
}

fn first_diagnostic_line(result: &CommandResult) -> Option<String> {
    result
        .stderr
        .split(|byte| *byte == b'\n')
        .chain(result.stdout.split(|byte| *byte == b'\n'))
        .find(|line| !line.is_empty())
        .map(|line| {
            let mut value = String::from_utf8_lossy(line).trim().to_string();
            if value.len() > 240 {
                value.truncate(237);
                value.push_str("...");
            }
            value
        })
}

fn resolve_executable(candidates: &[&str]) -> Option<PathBuf> {
    for candidate in candidates {
        let path = PathBuf::from(candidate);
        if path
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            return Some(path);
        }
    }
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        for candidate in candidates {
            let Some(name) = Path::new(candidate).file_name() else {
                continue;
            };
            let path = directory.join(name);
            if path.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            }) {
                return Some(path);
            }
        }
    }
    None
}

fn is_portable_service_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.chars().enumerate().all(|(index, ch)| {
            ch.is_ascii_alphanumeric()
                || (index > 0 && matches!(ch, '.' | '_' | '@' | ':' | '+' | '-' | '\\'))
        })
}

fn provider_token(provider: HostServiceProvider) -> &'static str {
    match provider {
        HostServiceProvider::Systemd => "systemd",
        HostServiceProvider::Openrc => "openrc",
        HostServiceProvider::Sysv => "sysv",
    }
}

fn action_token(action: HostServiceAction) -> &'static str {
    match action {
        HostServiceAction::Start => "start",
        HostServiceAction::Stop => "stop",
        HostServiceAction::Restart => "restart",
        HostServiceAction::Enable => "enable",
        HostServiceAction::Disable => "disable",
    }
}

fn chunked_output(job_id: uuid::Uuid, stream: OutputStream, data: &[u8]) -> Vec<CommandOutput> {
    data.chunks(MAX_SERVICE_OUTPUT_CHUNK_BYTES)
        .map(|chunk| CommandOutput {
            job_id,
            stream,
            data: chunk.to_vec(),
            exit_code: None,
            done: false,
        })
        .collect()
}

#[cfg(test)]
#[path = "tests_host_services.rs"]
mod tests;
