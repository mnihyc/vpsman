use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use vpsman_common::ProcessResourceLimits;

const DEFAULT_CGROUP_ROOT: &str = "/sys/fs/cgroup/vpsman-supervisor";
const CGROUP_V2_MIN_CPU_WEIGHT: u32 = 1;
const CGROUP_V2_MAX_CPU_WEIGHT: u32 = 10_000;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct ProcessLimitEvidence {
    #[serde(default)]
    pub(crate) cpu_shares: Option<LimitEvidence>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LimitEvidence {
    pub(crate) status: String,
    pub(crate) reason: String,
    #[serde(default)]
    pub(crate) requested: Option<u32>,
    #[serde(default)]
    pub(crate) applied: Option<u32>,
    #[serde(default)]
    pub(crate) path: Option<String>,
}

pub(crate) fn apply_cpu_shares_cgroup_v2(name: &str, pid: u32, shares: u32) -> LimitEvidence {
    if cgroup_enforcement_disabled() {
        return desired_only_evidence(
            shares,
            "cgroup CPU-share enforcement is disabled by VPSMAN_PROCESS_CGROUP_DISABLED",
        );
    }
    let root = process_cgroup_root();
    if !root.exists() {
        return desired_only_evidence(
            shares,
            &format!("cgroup v2 root {} is not available", root.display()),
        );
    }
    let controllers_path = root.join("cgroup.controllers");
    let controllers = match fs::read_to_string(&controllers_path) {
        Ok(value) => value,
        Err(error) => {
            return desired_only_evidence(
                shares,
                &format!(
                    "failed to read cgroup v2 controllers from {}: {error}",
                    controllers_path.display()
                ),
            );
        }
    };
    if !controllers
        .split_whitespace()
        .any(|controller| controller == "cpu")
    {
        return desired_only_evidence(shares, "cgroup v2 cpu controller is not available");
    }
    let path = root.join(format!("{}-{pid}", cgroup_name(name)));
    let weight = cpu_shares_to_cgroup_v2_weight(shares);
    match create_and_attach_cgroup(&root, &path, pid, weight) {
        Ok(()) => LimitEvidence {
            status: "enforced_cgroup_v2".to_string(),
            reason: "mapped cpu_shares to cgroup v2 cpu.weight and attached process".to_string(),
            requested: Some(shares),
            applied: Some(weight),
            path: Some(path.to_string_lossy().to_string()),
        },
        Err(error) => desired_only_evidence(
            shares,
            &format!(
                "failed to enforce cgroup v2 CPU weight at {}: {error}",
                path.display()
            ),
        ),
    }
}

pub(crate) fn cleanup_process_cgroup(path: Option<&str>) {
    let Some(path) = path else {
        return;
    };
    let _ = fs::remove_dir(Path::new(path));
}

pub(crate) fn limit_effectiveness(
    limits: &ProcessResourceLimits,
    evidence: &ProcessLimitEvidence,
) -> serde_json::Value {
    let cpu_status = evidence
        .cpu_shares
        .as_ref()
        .map(|evidence| evidence.status.as_str());
    let cpu_enforced = cpu_status == Some("enforced_cgroup_v2");
    let cpu_desired_only =
        limits.cpu_shares.is_some() && !matches!(cpu_status, Some("enforced_cgroup_v2"));
    serde_json::json!({
        "overall": {
            "status": if cpu_desired_only {
                "degraded_desired_only"
            } else if cpu_enforced {
                "enforced"
            } else {
                "enforced_or_not_requested"
            },
            "reason": if cpu_desired_only {
                evidence.cpu_shares.as_ref().map(|evidence| evidence.reason.as_str()).unwrap_or("cpu shares were requested but no cgroup enforcement evidence was recorded")
            } else if cpu_enforced {
                "requested CPU shares are enforced with cgroup v2 cpu.weight"
            } else {
                "requested limits are applied in child pre-exec or not requested"
            },
        },
        "memory_max_bytes": limit_status(limits.memory_max_bytes.is_some(), "enforced_in_child", "not_requested"),
        "pids_max": limit_status(limits.pids_max.is_some(), "enforced_in_child", "not_requested"),
        "open_files_max": limit_status(limits.open_files_max.is_some(), "enforced_in_child", "not_requested"),
        "no_new_privileges": limit_status(limits.no_new_privileges, "enforced_in_child", "not_requested"),
        "cpu_shares": cpu_limit_status(limits.cpu_shares, evidence.cpu_shares.as_ref()),
    })
}

pub(crate) fn cgroup_status(path: Option<&str>) -> serde_json::Value {
    let Some(path) = path else {
        return serde_json::json!({
            "status": "not_attached",
            "reason": "no cgroup path is recorded for this process",
        });
    };
    let path = Path::new(path);
    if !path.exists() {
        return serde_json::json!({
            "status": "missing",
            "path": path.to_string_lossy(),
            "reason": "recorded cgroup path is not present",
        });
    }

    let mut errors = Vec::new();
    let cpu_weight = read_u64(path.join("cpu.weight"), &mut errors);
    let process_count = read_process_count(path.join("cgroup.procs"), &mut errors);
    let memory_current_bytes = read_u64(path.join("memory.current"), &mut errors);
    let pids_current = read_u64(path.join("pids.current"), &mut errors);
    let events = read_key_value_u64(path.join("cgroup.events"), &mut errors);
    let cpu_stat = read_key_value_u64(path.join("cpu.stat"), &mut errors);

    serde_json::json!({
        "status": "available",
        "path": path.to_string_lossy(),
        "cpu_weight": cpu_weight,
        "process_count": process_count,
        "memory_current_bytes": memory_current_bytes,
        "pids_current": pids_current,
        "events": events,
        "cpu_stat": cpu_stat,
        "errors": errors,
    })
}

fn create_and_attach_cgroup(
    root: &Path,
    path: &Path,
    pid: u32,
    weight: u32,
) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    enable_cpu_controller(root);
    fs::write(path.join("cpu.weight"), weight.to_string())?;
    fs::write(path.join("cgroup.procs"), pid.to_string())?;
    Ok(())
}

fn enable_cpu_controller(root: &Path) {
    let subtree_control = root.join("cgroup.subtree_control");
    if let Ok(mut file) = OpenOptions::new().write(true).open(subtree_control) {
        let _ = file.write_all(b"+cpu");
    }
}

fn desired_only_evidence(shares: u32, reason: &str) -> LimitEvidence {
    LimitEvidence {
        status: "desired_only".to_string(),
        reason: reason.to_string(),
        requested: Some(shares),
        applied: None,
        path: None,
    }
}

fn limit_status(
    requested: bool,
    requested_status: &str,
    default_status: &str,
) -> serde_json::Value {
    serde_json::json!({
        "requested": requested,
        "status": if requested { requested_status } else { default_status },
    })
}

fn cpu_limit_status(
    cpu_shares: Option<u32>,
    evidence: Option<&LimitEvidence>,
) -> serde_json::Value {
    let Some(requested) = cpu_shares else {
        return limit_status(false, "not_requested", "not_requested");
    };
    let status = evidence
        .map(|evidence| evidence.status.as_str())
        .unwrap_or("desired_only");
    serde_json::json!({
        "requested": true,
        "requested_shares": requested,
        "status": status,
        "reason": evidence.map(|evidence| evidence.reason.as_str()).unwrap_or("no cgroup enforcement evidence was recorded"),
        "applied_weight": evidence.and_then(|evidence| evidence.applied),
        "path": evidence.and_then(|evidence| evidence.path.as_deref()),
    })
}

fn cpu_shares_to_cgroup_v2_weight(shares: u32) -> u32 {
    let clamped = shares.clamp(2, 262_144);
    let scaled = ((u64::from(clamped - 2) * u64::from(CGROUP_V2_MAX_CPU_WEIGHT - 1)) / 262_142)
        + u64::from(CGROUP_V2_MIN_CPU_WEIGHT);
    scaled.clamp(
        u64::from(CGROUP_V2_MIN_CPU_WEIGHT),
        u64::from(CGROUP_V2_MAX_CPU_WEIGHT),
    ) as u32
}

fn cgroup_name(name: &str) -> String {
    name.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.') {
                byte as char
            } else {
                '_'
            }
        })
        .collect()
}

fn cgroup_enforcement_disabled() -> bool {
    std::env::var("VPSMAN_PROCESS_CGROUP_DISABLED")
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn process_cgroup_root() -> PathBuf {
    std::env::var("VPSMAN_PROCESS_CGROUP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_CGROUP_ROOT))
}

fn read_u64(path: PathBuf, errors: &mut Vec<String>) -> Option<u64> {
    match fs::read_to_string(&path) {
        Ok(value) => match value.trim().parse::<u64>() {
            Ok(parsed) => Some(parsed),
            Err(error) => {
                errors.push(format!(
                    "failed to parse {} as u64: {error}",
                    path.display()
                ));
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            errors.push(format!("failed to read {}: {error}", path.display()));
            None
        }
    }
}

fn read_process_count(path: PathBuf, errors: &mut Vec<String>) -> Option<usize> {
    match fs::read_to_string(&path) {
        Ok(value) => Some(value.lines().filter(|line| !line.trim().is_empty()).count()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            errors.push(format!("failed to read {}: {error}", path.display()));
            None
        }
    }
}

fn read_key_value_u64(path: PathBuf, errors: &mut Vec<String>) -> BTreeMap<String, u64> {
    let mut values = BTreeMap::new();
    let contents = match fs::read_to_string(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return values,
        Err(error) => {
            errors.push(format!("failed to read {}: {error}", path.display()));
            return values;
        }
    };
    for line in contents.lines() {
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else {
            continue;
        };
        let Some(value) = parts.next() else {
            errors.push(format!("malformed key/value row in {}", path.display()));
            continue;
        };
        match value.parse::<u64>() {
            Ok(parsed) => {
                values.insert(key.to_string(), parsed);
            }
            Err(error) => errors.push(format!(
                "failed to parse key {key} in {} as u64: {error}",
                path.display()
            )),
        }
    }
    values
}
