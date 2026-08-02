use std::{collections::BTreeMap, path::Path};

use anyhow::Result;
use vpsman_common::{ProcessResourceLimits, ProcessRunPolicy};

const MAX_PROCESS_NAME_BYTES: usize = 64;

pub(crate) fn validate_process_name(name: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name.len() <= MAX_PROCESS_NAME_BYTES
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')),
        "invalid process name"
    );
    Ok(())
}

pub(crate) fn validate_process_argv(argv: &[String]) -> Result<()> {
    anyhow::ensure!(!argv.is_empty(), "process argv is empty");
    anyhow::ensure!(argv.len() <= 64, "process argv has too many parts");
    for part in argv {
        anyhow::ensure!(
            !part.is_empty() && part.len() <= 4096 && !part.as_bytes().contains(&0),
            "invalid process argv part"
        );
    }
    anyhow::ensure!(
        Path::new(&argv[0]).is_absolute(),
        "process executable must be an absolute path"
    );
    Ok(())
}

pub(crate) fn validate_process_cwd(cwd: Option<&str>) -> Result<()> {
    if let Some(cwd) = cwd {
        anyhow::ensure!(
            Path::new(cwd).is_absolute() && cwd.len() <= 4096 && !cwd.as_bytes().contains(&0),
            "process cwd must be an absolute path"
        );
    }
    Ok(())
}

pub(crate) fn validate_process_env(env: &BTreeMap<String, String>) -> Result<()> {
    anyhow::ensure!(env.len() <= 32, "process env has too many entries");
    for (key, value) in env {
        anyhow::ensure!(
            !key.is_empty()
                && key.len() <= 128
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "invalid process env key"
        );
        anyhow::ensure!(
            value.len() <= 4096 && !value.as_bytes().contains(&0),
            "invalid process env value"
        );
    }
    Ok(())
}

pub(crate) fn validate_process_policy(policy: &ProcessRunPolicy) -> Result<()> {
    anyhow::ensure!(
        policy.restart_max_retries <= 100,
        "process restart retry budget is out of range"
    );
    anyhow::ensure!(
        policy.restart_backoff_secs <= 3600,
        "process restart backoff is out of range"
    );
    anyhow::ensure!(
        (1..=300).contains(&policy.graceful_stop_secs),
        "process graceful stop timeout is out of range"
    );
    Ok(())
}

pub(crate) fn validate_process_limits(limits: &ProcessResourceLimits) -> Result<()> {
    if let Some(value) = limits.memory_max_bytes {
        anyhow::ensure!(
            (1024 * 1024..=1024_u64.pow(4)).contains(&value),
            "process memory limit is out of range"
        );
    }
    if let Some(value) = limits.pids_max {
        anyhow::ensure!(
            (1..=65_535).contains(&value),
            "process pid limit is out of range"
        );
    }
    if let Some(value) = limits.open_files_max {
        anyhow::ensure!(
            (16..=1_048_576).contains(&value),
            "process open-files limit is out of range"
        );
    }
    if let Some(value) = limits.cpu_shares {
        anyhow::ensure!(
            (2..=262_144).contains(&value),
            "process cpu shares limit is out of range"
        );
    }
    Ok(())
}
