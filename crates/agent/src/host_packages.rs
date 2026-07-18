use std::{
    collections::BTreeMap,
    env,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::{process::Command, time};
use vpsman_common::{
    payload_hash, CommandOutput, HostPackageCapability, HostPackageCapabilityStatus,
    HostPackageProvider, HostPackageUpdateApplyResult, HostPackageUpdatePlanSnapshot,
    HostPackageUpdateRecord, OutputStream,
};

use crate::{
    child_process::{run_child_with_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult},
    command_worker::CommandCancelToken,
};

const MAX_PACKAGE_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PACKAGE_OUTPUT_CHUNK_BYTES: usize = 64 * 1024;
const MAX_REVIEWABLE_PACKAGES: usize = 4096;

#[derive(Clone, Debug)]
struct PackageEnvironment {
    etc_root: PathBuf,
    var_root: PathBuf,
    usr_root: PathBuf,
    run_root: PathBuf,
    effective_uid: u32,
    apt_get: Option<PathBuf>,
    dnf: Option<PathBuf>,
    yum: Option<PathBuf>,
    pacman: Option<PathBuf>,
    rpm: Option<PathBuf>,
}

impl PackageEnvironment {
    fn discover() -> Self {
        Self {
            etc_root: PathBuf::from("/etc"),
            var_root: PathBuf::from("/var"),
            usr_root: PathBuf::from("/usr"),
            run_root: PathBuf::from("/run"),
            effective_uid: unsafe { libc::geteuid() } as u32,
            apt_get: resolve_executable(&["/usr/bin/apt-get", "/bin/apt-get"]),
            dnf: resolve_executable(&["/usr/bin/dnf", "/bin/dnf"]),
            yum: resolve_executable(&["/usr/bin/yum", "/bin/yum"]),
            pacman: resolve_executable(&["/usr/bin/pacman", "/bin/pacman"]),
            rpm: resolve_executable(&["/usr/bin/rpm", "/bin/rpm"]),
        }
    }

    fn dpkg_database_present(&self) -> bool {
        self.var_root.join("lib/dpkg/status").is_file()
    }

    fn rpm_database_present(&self) -> bool {
        self.var_root.join("lib/rpm").is_dir() || self.usr_root.join("lib/sysimage/rpm").is_dir()
    }

    fn pacman_database_present(&self) -> bool {
        self.var_root.join("lib/pacman/local").is_dir()
    }

    fn reboot_required(&self, provider: HostPackageProvider) -> Option<bool> {
        (provider == HostPackageProvider::Apt).then(|| {
            self.run_root.join("reboot-required").exists()
                || self.var_root.join("run/reboot-required").exists()
        })
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

#[derive(Clone, Debug)]
struct DistroIdentity {
    id: String,
    version: Option<String>,
    source: String,
}

#[derive(Serialize)]
struct CanonicalPackagePlan<'a> {
    provider: HostPackageProvider,
    distro_id: &'a str,
    distro_version: Option<&'a str>,
    packages: &'a [HostPackageUpdateRecord],
}

pub(crate) async fn execute_package_update_plan(
    job_id: uuid::Uuid,
    expected_provider: Option<HostPackageProvider>,
    refresh_metadata: bool,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    let environment = PackageEnvironment::discover();
    let snapshot = time::timeout(
        Duration::from_secs(max_timeout_secs.max(1)),
        collect_package_update_plan(
            &environment,
            expected_provider,
            refresh_metadata,
            max_timeout_secs,
            cancel_token,
        ),
    )
    .await
    .context("package update plan timed out")??;
    let payload = serde_json::to_vec(&snapshot)?;
    let mut outputs = chunked_output(job_id, OutputStream::Stdout, &payload);
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "package_update_plan",
            "capability_status": snapshot.capability.status,
            "provider": snapshot.capability.provider,
            "package_count": snapshot.packages.len(),
            "metadata_refreshed": snapshot.metadata_refreshed,
            "plan_hash": snapshot.plan_hash,
            "truncated": snapshot.truncated,
        }))?,
        exit_code: Some(0),
        done: true,
    });
    Ok(outputs)
}

pub(crate) async fn execute_package_update_apply(
    job_id: uuid::Uuid,
    provider: HostPackageProvider,
    plan_hash: &str,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    let environment = PackageEnvironment::discover();
    let result = time::timeout(
        Duration::from_secs(max_timeout_secs.max(1)),
        apply_package_update_plan(
            &environment,
            provider,
            plan_hash,
            max_timeout_secs,
            cancel_token,
        ),
    )
    .await
    .context("package update apply timed out")??;
    let payload = serde_json::to_vec(&result)?;
    let mut outputs = chunked_output(job_id, OutputStream::Stdout, &payload);
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "package_update_apply",
            "provider": result.provider,
            "accepted_plan_hash": result.accepted_plan_hash,
            "applied_package_count": result.applied_package_count,
            "remaining_package_count": result.remaining_packages.len(),
            "completed": result.completed,
            "reboot_required_after": result.reboot_required_after,
        }))?,
        exit_code: Some(if result.completed { 0 } else { 1 }),
        done: true,
    });
    Ok(outputs)
}

async fn collect_package_update_plan(
    environment: &PackageEnvironment,
    expected_provider: Option<HostPackageProvider>,
    refresh_metadata: bool,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<HostPackageUpdatePlanSnapshot> {
    cancel_token.check("package_update_plan")?;
    let capability = probe_package_capability(environment);
    if let Some(expected_provider) = expected_provider {
        if capability.provider != Some(expected_provider) {
            anyhow::bail!(
                "host_package_provider_changed: expected {}, observed {}: {}",
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
        return Ok(HostPackageUpdatePlanSnapshot {
            r#type: "package_update_plan".to_string(),
            capability,
            metadata_refresh_requested: refresh_metadata,
            metadata_refreshed: false,
            plan_hash: None,
            truncated: false,
            packages: Vec::new(),
            reboot_required_before: None,
        });
    }
    let provider = capability.provider.context("supported provider missing")?;
    if refresh_metadata {
        if !capability.can_refresh_metadata {
            anyhow::bail!(
                "package_metadata_refresh_unsupported: {}",
                capability
                    .reason
                    .as_deref()
                    .unwrap_or("metadata refresh requires root")
            );
        }
        refresh_package_metadata(
            environment,
            provider,
            max_timeout_secs,
            cancel_token.clone(),
        )
        .await?;
    }
    let mut packages = query_package_updates(
        environment,
        provider,
        max_timeout_secs,
        cancel_token.clone(),
    )
    .await?;
    packages.sort();
    packages.dedup();
    let plan_hash = package_plan_hash(&capability, provider, &packages)?;
    let truncated = packages.len() > MAX_REVIEWABLE_PACKAGES;
    packages.truncate(MAX_REVIEWABLE_PACKAGES);
    cancel_token.check("package_update_plan")?;
    Ok(HostPackageUpdatePlanSnapshot {
        r#type: "package_update_plan".to_string(),
        reboot_required_before: environment.reboot_required(provider),
        capability,
        metadata_refresh_requested: refresh_metadata,
        metadata_refreshed: refresh_metadata,
        plan_hash: Some(plan_hash),
        truncated,
        packages,
    })
}

async fn apply_package_update_plan(
    environment: &PackageEnvironment,
    provider: HostPackageProvider,
    accepted_plan_hash: &str,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<HostPackageUpdateApplyResult> {
    let normalized_hash = normalize_sha256(accepted_plan_hash)?;
    let capability = probe_package_capability(environment);
    require_provider(&capability, provider)?;
    if !capability.can_apply {
        anyhow::bail!(
            "package_update_apply_unsupported: {}",
            capability
                .reason
                .as_deref()
                .unwrap_or("package application requires root")
        );
    }
    let mut before = query_package_updates(
        environment,
        provider,
        max_timeout_secs,
        cancel_token.clone(),
    )
    .await?;
    before.sort();
    before.dedup();
    if before.len() > MAX_REVIEWABLE_PACKAGES {
        anyhow::bail!(
            "package_update_plan_too_large: {} packages exceeds review limit {}",
            before.len(),
            MAX_REVIEWABLE_PACKAGES
        );
    }
    let observed_hash = package_plan_hash(&capability, provider, &before)?;
    if observed_hash != normalized_hash {
        anyhow::bail!(
            "package_update_confirmation_stale: expected plan {}, observed {}",
            normalized_hash,
            observed_hash
        );
    }
    if before.is_empty() {
        return Ok(HostPackageUpdateApplyResult {
            r#type: "package_update_apply".to_string(),
            provider,
            accepted_plan_hash: normalized_hash,
            applied_package_count: 0,
            remaining_packages: Vec::new(),
            completed: true,
            reboot_required_after: environment.reboot_required(provider),
        });
    }
    apply_native_package_update(
        environment,
        provider,
        max_timeout_secs,
        cancel_token.clone(),
    )
    .await?;
    let mut remaining =
        query_package_updates(environment, provider, max_timeout_secs, cancel_token).await?;
    remaining.sort();
    remaining.dedup();
    let applied_package_count = before.len().saturating_sub(remaining.len());
    let completed = remaining.is_empty();
    remaining.truncate(MAX_REVIEWABLE_PACKAGES);
    Ok(HostPackageUpdateApplyResult {
        r#type: "package_update_apply".to_string(),
        provider,
        accepted_plan_hash: normalized_hash,
        applied_package_count,
        remaining_packages: remaining,
        completed,
        reboot_required_after: environment.reboot_required(provider),
    })
}

fn probe_package_capability(environment: &PackageEnvironment) -> HostPackageCapability {
    let identity = match read_distro_identity(environment) {
        Ok(identity) => identity,
        Err(error) => {
            return HostPackageCapability {
                status: HostPackageCapabilityStatus::ProbeFailed,
                reason: Some(error.to_string()),
                ..HostPackageCapability::default()
            };
        }
    };
    let provider = match identity.id.as_str() {
        "debian" | "ubuntu" => Some(HostPackageProvider::Apt),
        "arch" => Some(HostPackageProvider::Pacman),
        "fedora" => Some(HostPackageProvider::Dnf),
        "centos" | "rhel" | "rocky" | "almalinux" | "ol" => {
            match identity.version.as_deref().and_then(distro_major_version) {
                Some(0..=7) => Some(HostPackageProvider::Yum),
                Some(8..) => Some(HostPackageProvider::Dnf),
                None => {
                    return HostPackageCapability {
                        status: HostPackageCapabilityStatus::Unsupported,
                        distro_id: identity.id,
                        distro_version: identity.version,
                        reason: Some(format!(
                            "{} does not report a usable major version; yum versus dnf cannot be selected safely",
                            identity.source
                        )),
                        ..HostPackageCapability::default()
                    };
                }
            }
        }
        _ => None,
    };
    let Some(provider) = provider else {
        return HostPackageCapability {
            status: HostPackageCapabilityStatus::Unsupported,
            distro_id: identity.id.clone(),
            distro_version: identity.version,
            reason: Some(format!(
                "distribution ID {:?} from {} has no supported native package provider",
                identity.id, identity.source
            )),
            ..HostPackageCapability::default()
        };
    };
    let database_present = match provider {
        HostPackageProvider::Apt => environment.dpkg_database_present(),
        HostPackageProvider::Dnf | HostPackageProvider::Yum => environment.rpm_database_present(),
        HostPackageProvider::Pacman => environment.pacman_database_present(),
    };
    let binary_present = provider_binary(environment, provider).is_some();
    let rpm_present = !matches!(
        provider,
        HostPackageProvider::Dnf | HostPackageProvider::Yum
    ) || environment.rpm.as_deref().is_some_and(is_executable);
    if !database_present || !binary_present || !rpm_present {
        let mut missing = Vec::new();
        if !database_present {
            missing.push("native package database");
        }
        if !binary_present {
            missing.push(provider_token(provider));
        }
        if !rpm_present {
            missing.push("rpm");
        }
        return HostPackageCapability {
            status: HostPackageCapabilityStatus::Unsupported,
            provider: Some(provider),
            distro_id: identity.id,
            distro_version: identity.version,
            reason: Some(format!(
                "{} provider requirements are missing: {}",
                provider_token(provider),
                missing.join(", ")
            )),
            ..HostPackageCapability::default()
        };
    }
    let root = environment.effective_uid == 0;
    let split_refresh_supported = provider != HostPackageProvider::Pacman;
    let reason = if !root {
        Some(format!(
            "cached update planning is supported; metadata refresh and package application require root (effective UID {})",
            environment.effective_uid
        ))
    } else if !split_refresh_supported {
        Some(
            "Pacman metadata refresh is unsupported as a separate action because Arch requires it to be followed immediately by a full system upgrade; cached planning and application remain available"
                .to_string(),
        )
    } else {
        None
    };
    HostPackageCapability {
        status: HostPackageCapabilityStatus::Supported,
        provider: Some(provider),
        distro_id: identity.id,
        distro_version: identity.version,
        can_plan_cached: true,
        can_refresh_metadata: root && split_refresh_supported,
        can_apply: root,
        reason,
    }
}

async fn refresh_package_metadata(
    environment: &PackageEnvironment,
    provider: HostPackageProvider,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<()> {
    let binary = provider_binary(environment, provider).context("package provider disappeared")?;
    let args = match provider {
        HostPackageProvider::Apt => strings(&["update"]),
        HostPackageProvider::Dnf => strings(&["-q", "makecache", "--refresh"]),
        HostPackageProvider::Yum => strings(&["-q", "makecache"]),
        HostPackageProvider::Pacman => strings(&["-Sy", "--noconfirm"]),
    };
    let result = run_command(
        binary,
        &args,
        package_environment(provider),
        max_timeout_secs,
        MAX_PACKAGE_OUTPUT_BYTES,
        cancel_token,
    )
    .await?;
    ensure_exit_codes("package metadata refresh", &result, &[0])
}

async fn query_package_updates(
    environment: &PackageEnvironment,
    provider: HostPackageProvider,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<Vec<HostPackageUpdateRecord>> {
    let binary = provider_binary(environment, provider).context("package provider disappeared")?;
    let args = match provider {
        HostPackageProvider::Apt => strings(&[
            "-s",
            "-o",
            "Debug::NoLocking=1",
            "-o",
            "Dpkg::Options::=--force-confold",
            "upgrade",
        ]),
        HostPackageProvider::Dnf => strings(&["-q", "--cacheonly", "check-update"]),
        HostPackageProvider::Yum => strings(&["-q", "-C", "check-update"]),
        HostPackageProvider::Pacman => strings(&["-Qu"]),
    };
    let result = run_command(
        binary,
        &args,
        package_environment(provider),
        max_timeout_secs.min(300),
        MAX_PACKAGE_OUTPUT_BYTES,
        cancel_token.clone(),
    )
    .await?;
    match provider {
        HostPackageProvider::Apt => {
            ensure_exit_codes("apt cached upgrade plan", &result, &[0])?;
            Ok(parse_apt_updates(&result.stdout))
        }
        HostPackageProvider::Dnf | HostPackageProvider::Yum => {
            ensure_exit_codes("RPM cached upgrade plan", &result, &[0, 100])?;
            let mut updates = parse_rpm_provider_updates(&result.stdout);
            populate_rpm_current_versions(environment, &mut updates, cancel_token).await?;
            Ok(updates)
        }
        HostPackageProvider::Pacman => {
            ensure_exit_codes("pacman cached upgrade plan", &result, &[0])?;
            Ok(parse_pacman_updates(&result.stdout))
        }
    }
}

async fn apply_native_package_update(
    environment: &PackageEnvironment,
    provider: HostPackageProvider,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<()> {
    let binary = provider_binary(environment, provider).context("package provider disappeared")?;
    let args = match provider {
        HostPackageProvider::Apt => {
            strings(&["-y", "-o", "Dpkg::Options::=--force-confold", "upgrade"])
        }
        HostPackageProvider::Dnf => strings(&["-y", "--cacheonly", "upgrade"]),
        HostPackageProvider::Yum => strings(&["-y", "-C", "update"]),
        HostPackageProvider::Pacman => strings(&["-Su", "--noconfirm"]),
    };
    let result = run_command(
        binary,
        &args,
        package_environment(provider),
        max_timeout_secs,
        MAX_PACKAGE_OUTPUT_BYTES,
        cancel_token,
    )
    .await?;
    ensure_exit_codes("package update apply", &result, &[0])
}

async fn populate_rpm_current_versions(
    environment: &PackageEnvironment,
    updates: &mut [HostPackageUpdateRecord],
    cancel_token: CommandCancelToken,
) -> Result<()> {
    let rpm = environment
        .rpm
        .as_deref()
        .context("rpm binary disappeared")?;
    let mut current = BTreeMap::<(String, String), String>::new();
    for chunk in updates.chunks(256) {
        let mut args = strings(&["-q", "--qf", "%{NAME}\t%{ARCH}\t%{EVR}\n"]);
        args.extend(chunk.iter().map(|package| package.name.clone()));
        let result = run_command(
            rpm,
            &args,
            Vec::new(),
            30,
            1024 * 1024,
            cancel_token.clone(),
        )
        .await?;
        ensure_exit_codes("rpm installed-version query", &result, &[0])?;
        for line in String::from_utf8_lossy(&result.stdout).lines() {
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() == 3 {
                current.insert(
                    (fields[0].to_string(), fields[1].to_string()),
                    fields[2].to_string(),
                );
            }
        }
    }
    for update in updates {
        let architecture = update.architecture.as_deref().unwrap_or("");
        update.current_version = current
            .get(&(update.name.clone(), architecture.to_string()))
            .cloned()
            .or_else(|| {
                current.iter().find_map(|((name, _), version)| {
                    (name == &update.name).then(|| version.clone())
                })
            });
    }
    Ok(())
}

fn parse_apt_updates(output: &[u8]) -> Vec<HostPackageUpdateRecord> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let remainder = line.strip_prefix("Inst ")?;
            let name_end = remainder.find(char::is_whitespace)?;
            let name = remainder[..name_end].trim();
            if !is_portable_package_name(name) {
                return None;
            }
            let detail = remainder[name_end..].trim();
            let current_version = detail
                .strip_prefix('[')
                .and_then(|value| value.split_once(']'))
                .map(|(version, _)| version.trim().to_string());
            let candidate_start = detail.find('(')? + 1;
            let candidate_detail = &detail[candidate_start..];
            let candidate_version = candidate_detail.split_whitespace().next()?.trim();
            if candidate_version.is_empty() {
                return None;
            }
            let architecture = candidate_detail
                .rsplit_once('[')
                .and_then(|(_, value)| value.strip_suffix(')'))
                .and_then(|value| value.strip_suffix(']'))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let repository = candidate_detail
                .strip_suffix(')')
                .and_then(|value| value.split_once(char::is_whitespace))
                .map(|(_, value)| value.trim())
                .map(|value| {
                    value
                        .strip_suffix(']')
                        .and_then(|value| value.rsplit_once(" ["))
                        .map(|(repository, _)| repository)
                        .unwrap_or(value)
                        .trim()
                })
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            Some(HostPackageUpdateRecord {
                name: name.to_string(),
                architecture,
                current_version,
                candidate_version: candidate_version.to_string(),
                repository,
            })
        })
        .collect()
}

fn parse_rpm_provider_updates(output: &[u8]) -> Vec<HostPackageUpdateRecord> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 2 || fields[0].starts_with("Obsoleting") {
                return None;
            }
            let (name, architecture) = split_rpm_name_arch(fields[0]);
            if !is_portable_package_name(name) || fields[1].is_empty() {
                return None;
            }
            Some(HostPackageUpdateRecord {
                name: name.to_string(),
                architecture: architecture.map(str::to_string),
                current_version: None,
                candidate_version: fields[1].to_string(),
                repository: fields.get(2).map(|value| (*value).to_string()),
            })
        })
        .collect()
}

fn parse_pacman_updates(output: &[u8]) -> Vec<HostPackageUpdateRecord> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 4 || fields[2] != "->" || !is_portable_package_name(fields[0]) {
                return None;
            }
            Some(HostPackageUpdateRecord {
                name: fields[0].to_string(),
                architecture: None,
                current_version: Some(fields[1].to_string()),
                candidate_version: fields[3].to_string(),
                repository: None,
            })
        })
        .collect()
}

fn read_distro_identity(environment: &PackageEnvironment) -> Result<DistroIdentity> {
    let etc_os_release = environment.etc_root.join("os-release");
    let usr_os_release = environment.usr_root.join("lib/os-release");
    for (os_release, source) in [
        (&etc_os_release, "/etc/os-release"),
        (&usr_os_release, "/usr/lib/os-release"),
    ] {
        let Ok(contents) = std::fs::read_to_string(os_release) else {
            continue;
        };
        let values = parse_os_release(&contents);
        let id = values
            .get("ID")
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .context("/etc/os-release does not define ID")?;
        return Ok(DistroIdentity {
            id,
            version: values
                .get("VERSION_ID")
                .cloned()
                .filter(|value| !value.is_empty()),
            source: source.to_string(),
        });
    }
    let arch_release = environment.etc_root.join("arch-release");
    if arch_release.is_file() {
        return Ok(DistroIdentity {
            id: "arch".to_string(),
            version: None,
            source: "/etc/arch-release".to_string(),
        });
    }
    let centos_release = environment.etc_root.join("centos-release");
    let contents = std::fs::read_to_string(&centos_release).with_context(|| {
        format!(
            "cannot read {}, {}, {}, or {}",
            etc_os_release.display(),
            usr_os_release.display(),
            arch_release.display(),
            centos_release.display()
        )
    })?;
    let version = parse_legacy_centos_version(&contents)
        .context("legacy /etc/centos-release does not contain a version")?;
    Ok(DistroIdentity {
        id: "centos".to_string(),
        version: Some(version),
        source: "/etc/centos-release".to_string(),
    })
}

fn parse_os_release(contents: &str) -> BTreeMap<String, String> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let value = value.trim();
            let value = if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            Some((key.trim().to_string(), value.to_string()))
        })
        .collect()
}

fn parse_legacy_centos_version(contents: &str) -> Option<String> {
    let start = contents.find(|character: char| character.is_ascii_digit())?;
    let value = contents[start..]
        .split(|character: char| !(character.is_ascii_digit() || character == '.'))
        .next()?
        .trim_matches('.');
    (!value.is_empty()).then(|| value.to_string())
}

fn distro_major_version(value: &str) -> Option<u16> {
    value.split('.').next()?.parse().ok()
}

fn package_plan_hash(
    capability: &HostPackageCapability,
    provider: HostPackageProvider,
    packages: &[HostPackageUpdateRecord],
) -> Result<String> {
    Ok(payload_hash(&serde_json::to_vec(&CanonicalPackagePlan {
        provider,
        distro_id: &capability.distro_id,
        distro_version: capability.distro_version.as_deref(),
        packages,
    })?))
}

fn normalize_sha256(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        anyhow::bail!("package_update_plan_hash_invalid");
    }
    Ok(value)
}

fn require_provider(
    capability: &HostPackageCapability,
    expected: HostPackageProvider,
) -> Result<()> {
    if capability.provider != Some(expected) || !capability.supported() {
        anyhow::bail!(
            "host_package_provider_mismatch: expected {}, observed {}: {}",
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

fn provider_binary(
    environment: &PackageEnvironment,
    provider: HostPackageProvider,
) -> Option<&Path> {
    match provider {
        HostPackageProvider::Apt => environment.apt_get.as_deref(),
        HostPackageProvider::Dnf => environment.dnf.as_deref(),
        HostPackageProvider::Yum => environment.yum.as_deref(),
        HostPackageProvider::Pacman => environment.pacman.as_deref(),
    }
    .filter(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn package_environment(provider: HostPackageProvider) -> Vec<(&'static str, &'static str)> {
    match provider {
        HostPackageProvider::Apt => vec![
            ("DEBIAN_FRONTEND", "noninteractive"),
            ("APT_LISTCHANGES_FRONTEND", "none"),
            ("NEEDRESTART_MODE", "l"),
        ],
        _ => Vec::new(),
    }
}

async fn run_command(
    binary: &Path,
    args: &[String],
    environment: Vec<(&str, &str)>,
    timeout_secs: u64,
    max_output_bytes: usize,
    cancel_token: CommandCancelToken,
) -> Result<CommandResult> {
    let mut command = Command::new(binary);
    command.args(args);
    command.env("LC_ALL", "C");
    command.env("LANG", "C");
    command.envs(environment);
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
            anyhow::bail!("host_package_command_timed_out: {}", binary.display())
        }
        ChildRunResult::Canceled { reason, .. } => {
            anyhow::bail!("host_package_command_canceled: {reason}")
        }
    }
}

fn ensure_exit_codes(label: &str, result: &CommandResult, accepted: &[i32]) -> Result<()> {
    if result
        .exit_code
        .is_some_and(|exit_code| accepted.contains(&exit_code))
        && !result.stdout_truncated
        && !result.stderr_truncated
    {
        return Ok(());
    }
    let diagnostic = first_diagnostic_line(result)
        .unwrap_or_else(|| format!("exit code {:?}", result.exit_code));
    anyhow::bail!("host_package_command_failed: {label}: {diagnostic}")
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
        if is_executable(&path) {
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
            if is_executable(&path) {
                return Some(path);
            }
        }
    }
    None
}

fn split_rpm_name_arch(value: &str) -> (&str, Option<&str>) {
    let Some((name, architecture)) = value.rsplit_once('.') else {
        return (value, None);
    };
    if matches!(
        architecture,
        "noarch"
            | "x86_64"
            | "aarch64"
            | "i686"
            | "i586"
            | "i386"
            | "ppc64le"
            | "ppc64"
            | "s390x"
            | "armv7hl"
    ) {
        (name, Some(architecture))
    } else {
        (value, None)
    }
}

fn is_portable_package_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.chars().enumerate().all(|(index, character)| {
            character.is_ascii_alphanumeric()
                || (index > 0 && matches!(character, '.' | '_' | '+' | '-'))
        })
}

fn provider_token(provider: HostPackageProvider) -> &'static str {
    match provider {
        HostPackageProvider::Apt => "apt",
        HostPackageProvider::Dnf => "dnf",
        HostPackageProvider::Yum => "yum",
        HostPackageProvider::Pacman => "pacman",
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn chunked_output(job_id: uuid::Uuid, stream: OutputStream, data: &[u8]) -> Vec<CommandOutput> {
    data.chunks(MAX_PACKAGE_OUTPUT_CHUNK_BYTES)
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
mod tests {
    use super::*;

    fn test_environment() -> (PathBuf, PackageEnvironment) {
        let root =
            std::env::temp_dir().join(format!("vpsman-host-packages-{}", uuid::Uuid::new_v4()));
        let etc_root = root.join("etc");
        let var_root = root.join("var");
        let usr_root = root.join("usr");
        let run_root = root.join("run");
        for directory in [&etc_root, &var_root, &usr_root, &run_root] {
            std::fs::create_dir_all(directory).unwrap();
        }
        (
            root,
            PackageEnvironment {
                etc_root,
                var_root,
                usr_root,
                run_root,
                effective_uid: 0,
                apt_get: None,
                dnf: None,
                yum: None,
                pacman: None,
                rpm: None,
            },
        )
    }

    fn write_executable(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    fn write_os_release(environment: &PackageEnvironment, id: &str, version: Option<&str>) {
        let mut contents = format!("ID={id}\n");
        if let Some(version) = version {
            contents.push_str(&format!("VERSION_ID=\"{version}\"\n"));
        }
        std::fs::write(environment.etc_root.join("os-release"), contents).unwrap();
    }

    fn install_provider_fixture(
        root: &Path,
        environment: &mut PackageEnvironment,
        provider: HostPackageProvider,
    ) {
        match provider {
            HostPackageProvider::Apt => {
                let status = environment.var_root.join("lib/dpkg/status");
                std::fs::create_dir_all(status.parent().unwrap()).unwrap();
                std::fs::write(status, "Package: base-files\n").unwrap();
                environment.apt_get = Some(root.join("bin/apt-get"));
                write_executable(
                    environment.apt_get.as_deref().unwrap(),
                    "#!/bin/sh\nexit 0\n",
                );
            }
            HostPackageProvider::Dnf | HostPackageProvider::Yum => {
                std::fs::create_dir_all(environment.var_root.join("lib/rpm")).unwrap();
                environment.rpm = Some(root.join("bin/rpm"));
                write_executable(environment.rpm.as_deref().unwrap(), "#!/bin/sh\nexit 0\n");
                if provider == HostPackageProvider::Dnf {
                    environment.dnf = Some(root.join("bin/dnf"));
                    write_executable(environment.dnf.as_deref().unwrap(), "#!/bin/sh\nexit 0\n");
                } else {
                    environment.yum = Some(root.join("bin/yum"));
                    write_executable(environment.yum.as_deref().unwrap(), "#!/bin/sh\nexit 0\n");
                }
            }
            HostPackageProvider::Pacman => {
                std::fs::create_dir_all(environment.var_root.join("lib/pacman/local")).unwrap();
                environment.pacman = Some(root.join("bin/pacman"));
                write_executable(
                    environment.pacman.as_deref().unwrap(),
                    "#!/bin/sh\nexit 0\n",
                );
            }
        }
    }

    #[test]
    fn selects_native_providers_for_supported_distro_generations() {
        let cases = [
            ("debian", Some("8"), HostPackageProvider::Apt),
            ("debian", Some("12"), HostPackageProvider::Apt),
            ("ubuntu", Some("14.04"), HostPackageProvider::Apt),
            ("ubuntu", Some("24.04"), HostPackageProvider::Apt),
            ("arch", None, HostPackageProvider::Pacman),
            ("centos", Some("7"), HostPackageProvider::Yum),
            ("centos", Some("8"), HostPackageProvider::Dnf),
            ("rocky", Some("9.4"), HostPackageProvider::Dnf),
        ];
        for (id, version, provider) in cases {
            let (root, mut environment) = test_environment();
            write_os_release(&environment, id, version);
            install_provider_fixture(&root, &mut environment, provider);
            let capability = probe_package_capability(&environment);
            assert_eq!(
                capability.status,
                HostPackageCapabilityStatus::Supported,
                "{id} {version:?}"
            );
            assert_eq!(capability.provider, Some(provider), "{id} {version:?}");
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn selects_yum_for_legacy_centos_without_os_release() {
        let (root, mut environment) = test_environment();
        std::fs::write(
            environment.etc_root.join("centos-release"),
            "CentOS release 6.10 (Final)\n",
        )
        .unwrap();
        install_provider_fixture(&root, &mut environment, HostPackageProvider::Yum);
        let capability = probe_package_capability(&environment);
        assert_eq!(capability.status, HostPackageCapabilityStatus::Supported);
        assert_eq!(capability.provider, Some(HostPackageProvider::Yum));
        assert_eq!(capability.distro_version.as_deref(), Some("6.10"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uses_explicit_distro_identity_and_refuses_unknown_rhel_versions_or_fallbacks() {
        let (root, mut mixed) = test_environment();
        write_os_release(&mixed, "ubuntu", Some("22.04"));
        install_provider_fixture(&root, &mut mixed, HostPackageProvider::Apt);
        std::fs::create_dir_all(mixed.var_root.join("lib/rpm")).unwrap();
        let capability = probe_package_capability(&mixed);
        assert_eq!(capability.status, HostPackageCapabilityStatus::Supported);
        assert_eq!(capability.provider, Some(HostPackageProvider::Apt));
        std::fs::remove_dir_all(root).unwrap();

        let (root, mut unknown) = test_environment();
        write_os_release(&unknown, "centos", None);
        install_provider_fixture(&root, &mut unknown, HostPackageProvider::Dnf);
        let capability = probe_package_capability(&unknown);
        assert_eq!(capability.status, HostPackageCapabilityStatus::Unsupported);
        assert!(capability
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("major version")));
        std::fs::remove_dir_all(root).unwrap();

        let (root, mut no_fallback) = test_environment();
        write_os_release(&no_fallback, "centos", Some("7"));
        install_provider_fixture(&root, &mut no_fallback, HostPackageProvider::Dnf);
        let capability = probe_package_capability(&no_fallback);
        assert_eq!(capability.status, HostPackageCapabilityStatus::Unsupported);
        assert_eq!(capability.provider, Some(HostPackageProvider::Yum));
        assert!(capability
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("yum")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_planning_remains_visible_when_mutation_requires_root() {
        let (root, mut environment) = test_environment();
        write_os_release(&environment, "debian", Some("12"));
        install_provider_fixture(&root, &mut environment, HostPackageProvider::Apt);
        environment.effective_uid = 1000;
        let capability = probe_package_capability(&environment);
        assert!(capability.can_plan_cached);
        assert!(!capability.can_refresh_metadata);
        assert!(!capability.can_apply);
        assert!(capability
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("effective UID 1000")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pacman_split_metadata_refresh_is_explicitly_unsupported() {
        let (root, mut environment) = test_environment();
        write_os_release(&environment, "arch", None);
        install_provider_fixture(&root, &mut environment, HostPackageProvider::Pacman);
        let capability = probe_package_capability(&environment);
        assert_eq!(capability.status, HostPackageCapabilityStatus::Supported);
        assert!(capability.can_plan_cached);
        assert!(capability.can_apply);
        assert!(!capability.can_refresh_metadata);
        assert!(capability
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("full system upgrade")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_usr_lib_os_release_and_legacy_arch_marker() {
        let (root, mut environment) = test_environment();
        std::fs::create_dir_all(environment.usr_root.join("lib")).unwrap();
        std::fs::write(
            environment.usr_root.join("lib/os-release"),
            "ID=debian\nVERSION_ID=8\n",
        )
        .unwrap();
        install_provider_fixture(&root, &mut environment, HostPackageProvider::Apt);
        let capability = probe_package_capability(&environment);
        assert_eq!(capability.provider, Some(HostPackageProvider::Apt));
        std::fs::remove_dir_all(root).unwrap();

        let (root, mut environment) = test_environment();
        std::fs::write(environment.etc_root.join("arch-release"), "").unwrap();
        install_provider_fixture(&root, &mut environment, HostPackageProvider::Pacman);
        let capability = probe_package_capability(&environment);
        assert_eq!(capability.provider, Some(HostPackageProvider::Pacman));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn stale_apt_plan_is_rejected_before_package_mutation() {
        let (root, mut environment) = test_environment();
        write_os_release(&environment, "debian", Some("12"));
        install_provider_fixture(&root, &mut environment, HostPackageProvider::Apt);
        let apt_get = environment.apt_get.clone().unwrap();
        let mutation_marker = root.join("mutation-ran");
        write_executable(
            &apt_get,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"-s\" ]; then\n  echo 'Inst bash [5.1-2] (5.2-1 Debian:stable [amd64])'\n  exit 0\nfi\nprintf mutation > '{}'\n",
                mutation_marker.display()
            ),
        );
        let error = apply_package_update_plan(
            &environment,
            HostPackageProvider::Apt,
            &"00".repeat(32),
            30,
            CommandCancelToken::default(),
        )
        .await
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("package_update_confirmation_stale"));
        assert!(!mutation_marker.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn accepted_apt_plan_applies_and_rechecks_to_empty() {
        let (root, mut environment) = test_environment();
        write_os_release(&environment, "ubuntu", Some("20.04"));
        install_provider_fixture(&root, &mut environment, HostPackageProvider::Apt);
        let apt_get = environment.apt_get.clone().unwrap();
        let applied_marker = root.join("applied");
        write_executable(
            &apt_get,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"-s\" ]; then\n  if [ ! -f '{}' ]; then echo 'Inst openssl [1.1.1f] (1.1.1f-1ubuntu2.22 Ubuntu:focal-updates [amd64])'; fi\n  exit 0\nfi\nif [ \"$1\" = \"-y\" ]; then\n  : > '{}'\n  exit 0\nfi\nexit 2\n",
                applied_marker.display(),
                applied_marker.display()
            ),
        );
        let capability = probe_package_capability(&environment);
        let mut packages = query_package_updates(
            &environment,
            HostPackageProvider::Apt,
            30,
            CommandCancelToken::default(),
        )
        .await
        .unwrap();
        packages.sort();
        let hash = package_plan_hash(&capability, HostPackageProvider::Apt, &packages).unwrap();
        let result = apply_package_update_plan(
            &environment,
            HostPackageProvider::Apt,
            &hash,
            30,
            CommandCancelToken::default(),
        )
        .await
        .unwrap();
        assert!(result.completed);
        assert_eq!(result.applied_package_count, 1);
        assert!(result.remaining_packages.is_empty());
        assert!(applied_marker.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_apt_simulation_across_debian_and_ubuntu_shapes() {
        let updates = parse_apt_updates(
            b"Inst bash [5.1-2] (5.2-1 Debian:stable [amd64])\nInst openssl [3.0.2] (3.0.3 Ubuntu:22.04/jammy-updates [amd64])\nConf bash (5.2-1 Debian:stable [amd64])\n",
        );
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].name, "bash");
        assert_eq!(updates[0].current_version.as_deref(), Some("5.1-2"));
        assert_eq!(updates[0].candidate_version, "5.2-1");
        assert_eq!(updates[0].architecture.as_deref(), Some("amd64"));
        assert_eq!(updates[0].repository.as_deref(), Some("Debian:stable"));
    }

    #[test]
    fn parses_dnf_and_yum_check_update_lines() {
        let updates = parse_rpm_provider_updates(
            b"bash.x86_64 5.1.8-9.el9 baseos\npython3-libs.x86_64 3.9.18-3.el9 appstream\n",
        );
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].name, "bash");
        assert_eq!(updates[0].architecture.as_deref(), Some("x86_64"));
        assert_eq!(updates[0].candidate_version, "5.1.8-9.el9");
        assert_eq!(updates[0].repository.as_deref(), Some("baseos"));
    }

    #[test]
    fn parses_pacman_full_upgrade_plan() {
        let updates = parse_pacman_updates(
            b"linux 6.8.1.arch1-1 -> 6.8.2.arch1-1\nsystemd 255.4-2 -> 255.5-1\n",
        );
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].name, "linux");
        assert_eq!(updates[0].current_version.as_deref(), Some("6.8.1.arch1-1"));
        assert_eq!(updates[0].candidate_version, "6.8.2.arch1-1");
    }

    #[test]
    fn parses_legacy_centos_release_for_explicit_yum_selection() {
        assert_eq!(
            parse_legacy_centos_version("CentOS release 6.10 (Final)\n").as_deref(),
            Some("6.10")
        );
    }

    #[test]
    fn plan_hash_is_order_sensitive_until_callers_sort() {
        let capability = HostPackageCapability {
            status: HostPackageCapabilityStatus::Supported,
            provider: Some(HostPackageProvider::Apt),
            distro_id: "debian".to_string(),
            distro_version: Some("12".to_string()),
            ..HostPackageCapability::default()
        };
        let left = HostPackageUpdateRecord {
            name: "a".to_string(),
            architecture: Some("amd64".to_string()),
            current_version: Some("1".to_string()),
            candidate_version: "2".to_string(),
            repository: Some("stable".to_string()),
        };
        let right = HostPackageUpdateRecord {
            name: "b".to_string(),
            ..left.clone()
        };
        assert_ne!(
            package_plan_hash(
                &capability,
                HostPackageProvider::Apt,
                &[left.clone(), right.clone()]
            )
            .unwrap(),
            package_plan_hash(&capability, HostPackageProvider::Apt, &[right, left]).unwrap()
        );
    }
}
