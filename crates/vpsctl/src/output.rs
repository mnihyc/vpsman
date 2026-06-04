use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum OutputMode {
    Raw,
    Json,
    PrettyJson,
}

impl fmt::Display for OutputMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputMode::Raw => f.write_str("raw"),
            OutputMode::Json => f.write_str("json"),
            OutputMode::PrettyJson => f.write_str("pretty-json"),
        }
    }
}

pub(crate) fn run_with_output_mode<F>(mode: OutputMode, run: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    if mode == OutputMode::Raw {
        return run();
    }

    let (result, stdout) = capture_stdout(run)?;
    result?;
    emit_normalized_stdout(&stdout, mode)
}

fn emit_normalized_stdout(stdout: &str, mode: OutputMode) -> Result<()> {
    let value = normalize_stdout(stdout)?;
    match mode {
        OutputMode::Raw => {
            print!("{stdout}");
            Ok(())
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string(&value)?);
            Ok(())
        }
        OutputMode::PrettyJson => {
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
    }
}

pub(crate) fn normalize_stdout(stdout: &str) -> Result<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(json!({
            "kind": "empty",
            "stdout": null,
        }));
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(value);
    }

    let json_lines = parse_json_lines(trimmed);
    if !json_lines.is_empty() {
        return Ok(json!({
            "kind": "jsonl",
            "items": json_lines,
        }));
    }

    Ok(json!({
        "kind": "text",
        "stdout": stdout,
    }))
}

fn parse_json_lines(trimmed: &str) -> Vec<Value> {
    let mut values = Vec::new();
    for line in trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        match serde_json::from_str::<Value>(line) {
            Ok(value) => values.push(value),
            Err(_) => return Vec::new(),
        }
    }
    values
}

#[cfg(unix)]
fn capture_stdout<F>(run: F) -> Result<(Result<()>, String)>
where
    F: FnOnce() -> Result<()>,
{
    io::stdout().flush().context("failed to flush stdout")?;

    let capture_path = capture_file_path();
    let mut capture_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&capture_path)
        .with_context(|| {
            format!(
                "failed to create stdout capture file {}",
                capture_path.display()
            )
        })?;

    // SAFETY: duplicating the process stdout file descriptor is valid on Unix.
    let saved_stdout = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if saved_stdout < 0 {
        let _ = fs::remove_file(&capture_path);
        return Err(io::Error::last_os_error()).context("failed to duplicate stdout");
    }

    // SAFETY: the capture file and STDOUT_FILENO are valid file descriptors.
    if unsafe { libc::dup2(capture_file.as_raw_fd(), libc::STDOUT_FILENO) } < 0 {
        close_fd(saved_stdout);
        let _ = fs::remove_file(&capture_path);
        return Err(io::Error::last_os_error()).context("failed to redirect stdout");
    }

    let result = run();
    let flush_result = io::stdout()
        .flush()
        .context("failed to flush captured stdout");

    // SAFETY: `saved_stdout` was returned by dup and remains open here.
    if unsafe { libc::dup2(saved_stdout, libc::STDOUT_FILENO) } < 0 {
        close_fd(saved_stdout);
        let _ = fs::remove_file(&capture_path);
        return Err(io::Error::last_os_error()).context("failed to restore stdout");
    }
    close_fd(saved_stdout);
    flush_result?;

    let mut stdout = String::new();
    capture_file
        .seek(SeekFrom::Start(0))
        .context("failed to rewind stdout capture file")?;
    capture_file
        .read_to_string(&mut stdout)
        .context("captured stdout was not valid UTF-8 text")?;
    drop(capture_file);
    let _ = fs::remove_file(&capture_path);
    Ok((result, stdout))
}

#[cfg(not(unix))]
fn capture_stdout<F>(_run: F) -> Result<(Result<()>, String)>
where
    F: FnOnce() -> Result<()>,
{
    anyhow::bail!("--output json is supported only on Unix-like platforms")
}

fn close_fd(fd: libc::c_int) {
    // SAFETY: closing an owned file descriptor is safe; errors are ignored for
    // cleanup paths where there is no useful recovery.
    unsafe {
        libc::close(fd);
    }
}

fn capture_file_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("vpsctl-output-{}-{nanos}.tmp", process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_single_json_value() {
        let value = normalize_stdout("{\"ok\":true}\n").unwrap();
        assert_eq!(value["ok"], true);
    }

    #[test]
    fn normalize_jsonl_events() {
        let value = normalize_stdout("{\"seq\":1}\n{\"seq\":2}\n").unwrap();
        assert_eq!(value["kind"], "jsonl");
        assert_eq!(value["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn normalize_text_output() {
        let value = normalize_stdout("agent config toml\nline two\n").unwrap();
        assert_eq!(value["kind"], "text");
        assert_eq!(value["stdout"], "agent config toml\nline two\n");
    }

    #[test]
    fn normalize_empty_output() {
        let value = normalize_stdout("").unwrap();
        assert_eq!(value["kind"], "empty");
        assert!(value["stdout"].is_null());
    }
}
