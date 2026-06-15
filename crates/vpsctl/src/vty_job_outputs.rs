use std::path::PathBuf;

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::http::{http_get, http_get_to_file};

pub(crate) fn is_vty_job_output_command(command: &str) -> bool {
    command.starts_with("job-targets ")
        || command.starts_with("job-target-status-download ")
        || command.starts_with("job-outputs ")
        || command.starts_with("job-follow ")
        || command.starts_with("job-output-download ")
}

pub(crate) fn submit_vty_job_output_command(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    match parts.first().copied() {
        Some("job-targets") if parts.len() == 2 => Ok(http_get(
            api_url,
            &format!("/api/v1/jobs/{}/targets", parts[1]),
            token,
        )?),
        Some("job-target-status-download") if parts.len() == 3 => {
            let job_id = Uuid::parse_str(parts[1]).context("invalid job UUID")?;
            let output = PathBuf::from(parts[2]);
            let size_bytes = http_get_to_file(
                api_url,
                &format!("/api/v1/jobs/{job_id}/targets/download"),
                token,
                &output,
            )?;
            Ok(serde_json::json!({
                "job_id": job_id,
                "output": output,
                "size_bytes": size_bytes,
            })
            .to_string())
        }
        Some("job-outputs") if parts.len() == 2 => Ok(http_get(
            api_url,
            &format!("/api/v1/jobs/{}/outputs", parts[1]),
            token,
        )?),
        Some("job-follow") if parts.len() >= 2 => {
            let job_id = parts[1].to_string();
            let mut interval_ms = 1000_u64;
            let mut max_polls = 120_u16;
            let mut json = false;
            let mut index = 2;
            while index < parts.len() {
                match parts[index] {
                    "--json" => {
                        json = true;
                        index += 1;
                    }
                    "--interval-ms" if index + 1 < parts.len() => {
                        interval_ms = parts[index + 1]
                            .parse::<u64>()
                            .context("invalid job-follow interval milliseconds")?;
                        index += 2;
                    }
                    "--max-polls" if index + 1 < parts.len() => {
                        max_polls = parts[index + 1]
                            .parse::<u16>()
                            .context("invalid job-follow max polls")?;
                        index += 2;
                    }
                    _ => anyhow::bail!(
                        "usage: job-follow <job_uuid> [--interval-ms <100-10000>] [--max-polls <1-10000>] [--json]"
                    ),
                }
            }
            crate::commands_jobs::job_follow_output(
                api_url,
                token,
                job_id,
                interval_ms,
                max_polls,
                json,
            )
        }
        Some("job-output-download") if parts.len() == 6 && parts[3] == "--seq" => {
            let job_id = Uuid::parse_str(parts[1]).context("invalid job UUID")?;
            let client_id = parts[2];
            let seq = parts[4]
                .parse::<i32>()
                .context("invalid job output sequence")?;
            anyhow::ensure!(seq >= 0, "job output sequence must be non-negative");
            let output = PathBuf::from(parts[5]);
            let size_bytes = http_get_to_file(
                api_url,
                &format!(
                    "/api/v1/jobs/{job_id}/outputs/{}/{seq}/download",
                    percent_encode_path_segment(client_id)
                ),
                token,
                &output,
            )?;
            Ok(serde_json::json!({
                "job_id": job_id,
                "client_id": client_id,
                "seq": seq,
                "output": output,
                "size_bytes": size_bytes,
            })
            .to_string())
        }
        Some("job-output-download") => anyhow::bail!(
            "usage: job-output-download <job_uuid> <client_id> --seq <seq> <output_file>"
        ),
        Some("job-targets") => anyhow::bail!("usage: job-targets <job_uuid>"),
        Some("job-target-status-download") => {
            anyhow::bail!("usage: job-target-status-download <job_uuid> <output_file>")
        }
        Some("job-outputs") => anyhow::bail!("usage: job-outputs <job_uuid>"),
        Some("job-follow") => anyhow::bail!(
            "usage: job-follow <job_uuid> [--interval-ms <100-10000>] [--max-polls <1-10000>] [--json]"
        ),
        _ => anyhow::bail!("unknown job output command"),
    }
}

fn percent_encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_job_follow_as_job_output_command() {
        assert!(is_vty_job_output_command(
            "job-follow 11111111-2222-4333-8444-555555555555"
        ));
        assert!(is_vty_job_output_command(
            "job-follow 11111111-2222-4333-8444-555555555555 --json"
        ));
    }
}
