use std::{
    io::{self, Write},
    path::PathBuf,
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    http::http_get,
    util::{percent_encode_path_segment, percent_encode_query_value},
};

pub(crate) fn terminal_sessions(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    client_id: Option<String>,
    session_id: Option<String>,
) -> Result<()> {
    println!(
        "{}",
        terminal_sessions_output(api_url, token, limit, client_id, session_id)?
    );
    Ok(())
}

pub(crate) fn terminal_sessions_output(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    client_id: Option<String>,
    session_id: Option<String>,
) -> Result<String> {
    let path = terminal_sessions_path(limit, client_id.as_deref(), session_id.as_deref())?;
    http_get(api_url, &path, token)
}

pub(crate) fn terminal_sessions_path(
    limit: u16,
    client_id: Option<&str>,
    session_id: Option<&str>,
) -> Result<String> {
    let mut path = format!("/api/v1/terminal-sessions?limit={}", limit.clamp(1, 200));
    if let Some(client_id) = client_id.map(str::trim).filter(|value| !value.is_empty()) {
        anyhow::ensure!(
            client_id.len() <= 128,
            "--client-id must be at most 128 bytes"
        );
        path.push_str("&client_id=");
        path.push_str(&percent_encode_query_value(client_id));
    }
    if let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) {
        let session_id = Uuid::parse_str(session_id).context("invalid --session-id UUID")?;
        path.push_str("&session_id=");
        path.push_str(&session_id.to_string());
    }
    Ok(path)
}

pub(crate) fn terminal_replay(
    api_url: &str,
    token: Option<&str>,
    request: TerminalReplayRequest,
) -> Result<()> {
    println!("{}", terminal_replay_output(api_url, token, request)?);
    Ok(())
}

pub(crate) fn terminal_replay_output(
    api_url: &str,
    token: Option<&str>,
    request: TerminalReplayRequest,
) -> Result<String> {
    let session_id = Uuid::parse_str(&request.session_id).context("invalid --session-id UUID")?;
    let path = terminal_replay_path(
        &request.client_id,
        session_id,
        request.from_seq,
        request.limit,
        request.max_bytes,
        request.metadata_only,
    );
    let response = http_get(api_url, &path, token)?;
    if let Some(output) = request.output_file {
        anyhow::ensure!(
            !request.metadata_only,
            "terminal-replay --output-file requires replay data; remove --metadata-only"
        );
        let replay: TerminalReplayResponse =
            serde_json::from_str(&response).context("failed to parse terminal replay response")?;
        let mut bytes = Vec::new();
        for chunk in replay.chunks {
            let Some(data_base64) = chunk.data_base64 else {
                anyhow::bail!(
                    "terminal replay chunk {} has no inline data; retry without --metadata-only and with server object-store access",
                    chunk.terminal_seq
                );
            };
            let data = BASE64.decode(data_base64).with_context(|| {
                format!(
                    "terminal replay chunk {} is not valid base64",
                    chunk.terminal_seq
                )
            })?;
            bytes.extend_from_slice(&data);
        }
        std::fs::write(&output, &bytes)
            .with_context(|| format!("failed to write terminal replay {}", output.display()))?;
        let mut value: serde_json::Value = serde_json::from_str(&response)?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "output".to_string(),
                serde_json::json!(output.to_string_lossy().to_string()),
            );
            object.insert("written_bytes".to_string(), serde_json::json!(bytes.len()));
        }
        Ok(value.to_string())
    } else {
        Ok(response)
    }
}

pub(crate) struct TerminalReplayRequest {
    pub(crate) client_id: String,
    pub(crate) session_id: String,
    pub(crate) from_seq: Option<u64>,
    pub(crate) limit: u16,
    pub(crate) max_bytes: u32,
    pub(crate) output_file: Option<PathBuf>,
    pub(crate) metadata_only: bool,
}

pub(crate) struct TerminalFollowRequest {
    pub(crate) client_id: String,
    pub(crate) session_id: String,
    pub(crate) from_seq: Option<u64>,
    pub(crate) interval_ms: u64,
    pub(crate) max_polls: u32,
    pub(crate) json: bool,
}

pub(crate) fn terminal_follow(
    api_url: &str,
    token: Option<&str>,
    request: TerminalFollowRequest,
) -> Result<()> {
    let session_id = Uuid::parse_str(&request.session_id).context("invalid --session-id UUID")?;
    let mut next_seq = request.from_seq.unwrap_or(1).max(1);
    let interval = Duration::from_millis(request.interval_ms.clamp(250, 10_000));
    let mut polls = 0_u32;
    loop {
        let replay = terminal_replay_response(
            api_url,
            token,
            TerminalReplayFetch {
                client_id: &request.client_id,
                session_id,
                from_seq: Some(next_seq),
                limit: 200,
                max_bytes: 1024 * 1024,
                metadata_only: false,
            },
        )?;
        if request.json {
            for chunk in &replay.chunks {
                println!("{}", serde_json::to_string(chunk)?);
            }
        } else {
            let mut stdout = io::stdout().lock();
            for chunk in &replay.chunks {
                let Some(data_base64) = chunk.data_base64.as_deref() else {
                    continue;
                };
                let data = BASE64.decode(data_base64).with_context(|| {
                    format!(
                        "terminal replay chunk {} is not valid base64",
                        chunk.terminal_seq
                    )
                })?;
                stdout.write_all(&data)?;
            }
            stdout.flush()?;
        }
        next_seq = next_seq.max(replay.next_seq.max(1) as u64);
        polls = polls.saturating_add(1);
        if request.max_polls > 0 && polls >= request.max_polls {
            break;
        }
        thread::sleep(interval);
    }
    Ok(())
}

fn terminal_replay_path(
    client_id: &str,
    session_id: Uuid,
    from_seq: Option<u64>,
    limit: u16,
    max_bytes: u32,
    metadata_only: bool,
) -> String {
    let mut path = format!(
        "/api/v1/terminal-sessions/{}/{session_id}/replay?limit={}&max_bytes={}&include_data={}",
        percent_encode_path_segment(client_id),
        limit.clamp(1, 1000),
        max_bytes.max(1),
        !metadata_only
    );
    if let Some(from_seq) = from_seq {
        path.push_str("&from_seq=");
        path.push_str(&from_seq.max(1).to_string());
    }
    path
}

struct TerminalReplayFetch<'a> {
    client_id: &'a str,
    session_id: Uuid,
    from_seq: Option<u64>,
    limit: u16,
    max_bytes: u32,
    metadata_only: bool,
}

fn terminal_replay_response(
    api_url: &str,
    token: Option<&str>,
    request: TerminalReplayFetch<'_>,
) -> Result<TerminalReplayResponse> {
    let TerminalReplayFetch {
        client_id,
        session_id,
        from_seq,
        limit,
        max_bytes,
        metadata_only,
    } = request;
    let path = terminal_replay_path(
        client_id,
        session_id,
        from_seq,
        limit,
        max_bytes,
        metadata_only,
    );
    serde_json::from_str(&http_get(api_url, &path, token)?)
        .context("failed to parse terminal replay response")
}

#[derive(Debug, Deserialize)]
struct TerminalReplayResponse {
    chunks: Vec<TerminalReplayChunkResponse>,
    next_seq: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct TerminalReplayChunkResponse {
    terminal_seq: i64,
    job_id: String,
    job_output_seq: i32,
    data_base64: Option<String>,
    size_bytes: i64,
    sha256_hex: Option<String>,
    storage: String,
    artifact_object_key: Option<String>,
    created_at: String,
}

#[cfg(test)]
mod tests {
    use super::{terminal_replay_path, terminal_sessions_path};
    use uuid::Uuid;

    #[test]
    fn builds_filtered_terminal_sessions_path() {
        let path = terminal_sessions_path(
            500,
            Some("edge a"),
            Some("11111111-2222-4333-8444-555555555555"),
        )
        .unwrap();

        assert_eq!(
            path,
            "/api/v1/terminal-sessions?limit=200&client_id=edge%20a&session_id=11111111-2222-4333-8444-555555555555"
        );
    }

    #[test]
    fn builds_terminal_replay_path() {
        let session_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();

        assert_eq!(
            terminal_replay_path("edge a", session_id, Some(7), 5000, 0, false),
            "/api/v1/terminal-sessions/edge%20a/11111111-2222-4333-8444-555555555555/replay?limit=1000&max_bytes=1&include_data=true&from_seq=7"
        );
    }
}
