use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::Value;
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use crate::{
    model::JobOutputView,
    model_terminal::{TerminalReplayChunkView, TerminalReplayView, TerminalSessionView},
    repository::Repository,
};

const TERMINAL_COMMAND_TYPES: &[&str] = &[
    "terminal_open",
    "terminal_input",
    "terminal_poll",
    "terminal_resize",
    "terminal_close",
];

impl Repository {
    pub(crate) async fn list_terminal_sessions(
        &self,
        limit: i64,
        client_id: Option<&str>,
        session_id: Option<Uuid>,
    ) -> Result<Vec<TerminalSessionView>> {
        let limit = limit.clamp(1, 200);
        let scan_limit = limit.saturating_mul(64).clamp(100, 10_000);
        match self {
            Self::Memory(memory) => {
                let command_types = memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .map(|job| (job.id, job.command_type.clone()))
                    .collect::<BTreeMap<_, _>>();
                let mut outputs = memory
                    .job_outputs
                    .read()
                    .await
                    .iter()
                    .filter_map(|output| {
                        if output.stream != "status" {
                            return None;
                        }
                        if let Some(client_id) = client_id {
                            if output.client_id != client_id {
                                return None;
                            }
                        }
                        let command_type = command_types.get(&output.job_id)?;
                        if !is_terminal_command(command_type) {
                            return None;
                        }
                        Some(TerminalStatusOutput {
                            job_id: output.job_id,
                            client_id: output.client_id.clone(),
                            seq: output.seq,
                            data: BASE64.decode(&output.data_base64).ok()?,
                            created_at: output.created_at.clone(),
                            command_type: command_type.clone(),
                        })
                    })
                    .collect::<Vec<_>>();
                outputs.sort_by(|left, right| {
                    right
                        .created_at
                        .cmp(&left.created_at)
                        .then_with(|| right.job_id.cmp(&left.job_id))
                        .then_with(|| right.seq.cmp(&left.seq))
                });
                Ok(build_terminal_sessions(outputs, limit, session_id))
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        output.job_id,
                        output.client_id,
                        output.seq,
                        output.data,
                        output.created_at::text AS created_at,
                        job.command_type
                    FROM job_outputs output
                    JOIN jobs job ON job.id = output.job_id
                    WHERE output.stream = 'status'
                      AND job.command_type IN (
                        'terminal_open',
                        'terminal_input',
                        'terminal_poll',
                        'terminal_resize',
                        'terminal_close'
                      )
                      AND ($2::text IS NULL OR output.client_id = $2)
                    ORDER BY output.created_at DESC, output.job_id DESC, output.seq DESC
                    LIMIT $1
                    "#,
                )
                .bind(scan_limit)
                .bind(client_id)
                .fetch_all(pool)
                .await?;
                let outputs = rows
                    .into_iter()
                    .map(|row| {
                        Ok(TerminalStatusOutput {
                            job_id: row.try_get("job_id")?,
                            client_id: row.try_get("client_id")?,
                            seq: row.try_get("seq")?,
                            data: row.try_get("data")?,
                            created_at: row.try_get("created_at")?,
                            command_type: row.try_get("command_type")?,
                        })
                    })
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
                Ok(build_terminal_sessions(outputs, limit, session_id))
            }
        }
    }

    pub(crate) async fn terminal_session_replay(
        &self,
        client_id: &str,
        session_id: Uuid,
        from_seq: Option<i64>,
        limit: i64,
    ) -> Result<TerminalReplayView> {
        let outputs = self
            .list_terminal_replay_outputs(client_id, session_id)
            .await?;
        Ok(build_terminal_replay(
            client_id, session_id, outputs, from_seq, limit,
        ))
    }

    async fn list_terminal_replay_outputs(
        &self,
        client_id: &str,
        session_id: Uuid,
    ) -> Result<Vec<TerminalReplayOutput>> {
        match self {
            Self::Memory(memory) => {
                let command_types = memory
                    .jobs
                    .read()
                    .await
                    .iter()
                    .map(|job| (job.id, job.command_type.clone()))
                    .collect::<BTreeMap<_, _>>();
                let mut by_job = BTreeMap::<Uuid, Vec<TerminalReplayOutput>>::new();
                for output in memory.job_outputs.read().await.iter() {
                    if output.client_id != client_id {
                        continue;
                    }
                    let Some(command_type) = command_types.get(&output.job_id) else {
                        continue;
                    };
                    if !is_terminal_command(command_type) {
                        continue;
                    }
                    by_job
                        .entry(output.job_id)
                        .or_default()
                        .push(TerminalReplayOutput {
                            output: output.clone(),
                            command_type: command_type.clone(),
                        });
                }
                let mut outputs = Vec::new();
                for (_job_id, job_outputs) in by_job {
                    if job_outputs.iter().any(|output| {
                        output.output.stream == "status"
                            && parse_terminal_replay_status(&output.output, session_id).is_some()
                    }) {
                        outputs.extend(job_outputs);
                    }
                }
                outputs.sort_by(|left, right| {
                    left.output
                        .created_at
                        .cmp(&right.output.created_at)
                        .then_with(|| left.output.job_id.cmp(&right.output.job_id))
                        .then_with(|| left.output.seq.cmp(&right.output.seq))
                });
                Ok(outputs)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH terminal_jobs AS (
                        SELECT DISTINCT output.job_id
                        FROM job_outputs output
                        JOIN jobs job ON job.id = output.job_id
                        WHERE output.client_id = $1
                          AND output.stream = 'status'
                          AND job.command_type IN (
                            'terminal_open',
                            'terminal_input',
                            'terminal_poll',
                            'terminal_resize',
                            'terminal_close'
                          )
                          AND convert_from(output.data, 'UTF8')::jsonb->>'session_id' = $2
                    )
                    SELECT
                        output.job_id,
                        output.client_id,
                        output.seq,
                        output.stream,
                        output.data,
                        output.storage,
                        output.object_key,
                        output.data_sha256_hex,
                        output.data_size_bytes,
                        output.exit_code,
                        output.done,
                        output.created_at::text AS created_at,
                        job.command_type
                    FROM job_outputs output
                    JOIN jobs job ON job.id = output.job_id
                    JOIN terminal_jobs terminal_job ON terminal_job.job_id = output.job_id
                    WHERE output.client_id = $1
                    ORDER BY output.created_at, output.job_id, output.seq
                    "#,
                )
                .bind(client_id)
                .bind(session_id.to_string())
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let data: Vec<u8> = row.try_get("data")?;
                        Ok(TerminalReplayOutput {
                            output: JobOutputView {
                                job_id: row.try_get("job_id")?,
                                client_id: row.try_get("client_id")?,
                                seq: row.try_get("seq")?,
                                stream: row.try_get("stream")?,
                                data_base64: BASE64.encode(data),
                                storage: row.try_get("storage")?,
                                artifact_object_key: row.try_get("object_key")?,
                                artifact_sha256_hex: row.try_get("data_sha256_hex")?,
                                artifact_size_bytes: row.try_get("data_size_bytes")?,
                                exit_code: row.try_get("exit_code")?,
                                done: row.try_get("done")?,
                                created_at: row.try_get("created_at")?,
                            },
                            command_type: row.try_get("command_type")?,
                        })
                    })
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }
}

#[derive(Clone, Debug)]
struct TerminalStatusOutput {
    job_id: Uuid,
    client_id: String,
    seq: i32,
    data: Vec<u8>,
    created_at: String,
    command_type: String,
}

#[derive(Clone, Debug)]
struct TerminalReplayOutput {
    output: JobOutputView,
    command_type: String,
}

#[derive(Clone, Debug)]
struct TerminalEvent {
    session_id: Uuid,
    client_id: String,
    state: &'static str,
    status: String,
    argv: Vec<String>,
    cwd: Option<String>,
    cols: Option<i64>,
    rows: Option<i64>,
    idle_timeout_secs: Option<i64>,
    flow_window_bytes: Option<i64>,
    output_first_seq: Option<i64>,
    output_next_seq: Option<i64>,
    output_retained_first_seq: Option<i64>,
    output_retained_bytes: Option<i64>,
    output_dropped_bytes: Option<i64>,
    output_dropped_chunks: Option<i64>,
    output_replay_truncated: bool,
    input_seq: Option<i64>,
    session_exited: bool,
    close_reason: Option<String>,
    event_type: String,
    job_id: Uuid,
    command_type: String,
    seq: i32,
    created_at: String,
}

#[derive(Clone, Debug)]
struct TerminalAggregate {
    latest: TerminalEvent,
    argv: Vec<String>,
    cwd: Option<String>,
    cols: Option<i64>,
    rows: Option<i64>,
    idle_timeout_secs: Option<i64>,
    flow_window_bytes: Option<i64>,
    output_first_seq: Option<i64>,
    output_next_seq: Option<i64>,
    output_retained_first_seq: Option<i64>,
    output_retained_bytes: Option<i64>,
    output_dropped_bytes: Option<i64>,
    output_dropped_chunks: Option<i64>,
    output_replay_truncated: bool,
    last_input_seq: Option<i64>,
    close_reason: Option<String>,
}

impl TerminalAggregate {
    fn new(event: TerminalEvent) -> Self {
        Self {
            argv: event.argv.clone(),
            cwd: event.cwd.clone(),
            cols: event.cols,
            rows: event.rows,
            idle_timeout_secs: event.idle_timeout_secs,
            flow_window_bytes: event.flow_window_bytes,
            output_first_seq: event.output_first_seq,
            output_next_seq: event.output_next_seq,
            output_retained_first_seq: event.output_retained_first_seq,
            output_retained_bytes: event.output_retained_bytes,
            output_dropped_bytes: event.output_dropped_bytes,
            output_dropped_chunks: event.output_dropped_chunks,
            output_replay_truncated: event.output_replay_truncated,
            last_input_seq: event.input_seq,
            close_reason: event.close_reason.clone(),
            latest: event,
        }
    }

    fn merge_older(&mut self, event: TerminalEvent) {
        if self.argv.is_empty() {
            self.argv = event.argv;
        }
        self.cwd = self.cwd.take().or(event.cwd);
        self.cols = self.cols.or(event.cols);
        self.rows = self.rows.or(event.rows);
        self.idle_timeout_secs = self.idle_timeout_secs.or(event.idle_timeout_secs);
        self.flow_window_bytes = self.flow_window_bytes.or(event.flow_window_bytes);
        self.output_first_seq = self.output_first_seq.or(event.output_first_seq);
        self.output_next_seq = self.output_next_seq.or(event.output_next_seq);
        self.output_retained_first_seq = self
            .output_retained_first_seq
            .or(event.output_retained_first_seq);
        self.output_retained_bytes = self.output_retained_bytes.or(event.output_retained_bytes);
        self.output_dropped_bytes = self.output_dropped_bytes.or(event.output_dropped_bytes);
        self.output_dropped_chunks = self.output_dropped_chunks.or(event.output_dropped_chunks);
        self.output_replay_truncated |= event.output_replay_truncated;
        self.last_input_seq = self.last_input_seq.or(event.input_seq);
        self.close_reason = self.close_reason.take().or(event.close_reason);
    }

    fn into_view(self) -> TerminalSessionView {
        TerminalSessionView {
            session_id: self.latest.session_id,
            client_id: self.latest.client_id,
            state: self.latest.state.to_string(),
            last_status: self.latest.status,
            argv: self.argv,
            cwd: self.cwd,
            cols: self.cols,
            rows: self.rows,
            idle_timeout_secs: self.idle_timeout_secs,
            flow_window_bytes: self.flow_window_bytes,
            output_first_seq: self.output_first_seq,
            output_next_seq: self.output_next_seq,
            output_retained_first_seq: self.output_retained_first_seq,
            output_retained_bytes: self.output_retained_bytes,
            output_dropped_bytes: self.output_dropped_bytes,
            output_dropped_chunks: self.output_dropped_chunks,
            output_replay_truncated: self.output_replay_truncated,
            last_input_seq: self.last_input_seq,
            session_exited: self.latest.session_exited,
            close_reason: self.close_reason,
            last_event: self.latest.event_type,
            last_job_id: self.latest.job_id,
            last_command_type: self.latest.command_type,
            last_seq: self.latest.seq,
            observed_at: self.latest.created_at,
        }
    }
}

fn build_terminal_sessions(
    outputs: Vec<TerminalStatusOutput>,
    limit: i64,
    session_filter: Option<Uuid>,
) -> Vec<TerminalSessionView> {
    let mut order = Vec::<(String, Uuid)>::new();
    let mut aggregates = BTreeMap::<(String, Uuid), TerminalAggregate>::new();

    for output in outputs {
        let Some(event) = parse_terminal_event(output) else {
            continue;
        };
        if session_filter.is_some_and(|session_id| event.session_id != session_id) {
            continue;
        }
        let key = (event.client_id.clone(), event.session_id);
        if let Some(aggregate) = aggregates.get_mut(&key) {
            aggregate.merge_older(event);
        } else {
            order.push(key.clone());
            aggregates.insert(key, TerminalAggregate::new(event));
        }
    }

    let limit = limit.clamp(1, 200) as usize;
    let mut views = Vec::new();
    let mut emitted = BTreeSet::new();
    for key in order {
        if !emitted.insert(key.clone()) {
            continue;
        }
        if let Some(aggregate) = aggregates.remove(&key) {
            views.push(aggregate.into_view());
            if views.len() >= limit {
                break;
            }
        }
    }
    views
}

fn parse_terminal_event(output: TerminalStatusOutput) -> Option<TerminalEvent> {
    let value = serde_json::from_slice::<Value>(&output.data).ok()?;
    let event_type = value.get("type")?.as_str()?.to_string();
    if !is_terminal_status_event(&event_type) {
        return None;
    }
    let session_id = value
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())?;
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let session_exited = value
        .get("session_exited")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let state = terminal_state(&event_type, &status, session_exited);

    Some(TerminalEvent {
        session_id,
        client_id: output.client_id,
        state,
        status,
        argv: value
            .get("argv")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        cwd: json_string(&value, "cwd"),
        cols: value.get("cols").and_then(json_i64),
        rows: value.get("rows").and_then(json_i64),
        idle_timeout_secs: value.get("idle_timeout_secs").and_then(json_i64),
        flow_window_bytes: value.get("flow_window_bytes").and_then(json_i64),
        output_first_seq: value.get("output_first_seq").and_then(json_i64),
        output_next_seq: value.get("output_next_seq").and_then(json_i64),
        output_retained_first_seq: value.get("output_retained_first_seq").and_then(json_i64),
        output_retained_bytes: value.get("output_retained_bytes").and_then(json_i64),
        output_dropped_bytes: value.get("output_dropped_bytes").and_then(json_i64),
        output_dropped_chunks: value.get("output_dropped_chunks").and_then(json_i64),
        output_replay_truncated: value
            .get("output_replay_truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        input_seq: value.get("input_seq").and_then(json_i64),
        session_exited,
        close_reason: json_string(&value, "reason"),
        event_type,
        job_id: output.job_id,
        command_type: output.command_type,
        seq: output.seq,
        created_at: output.created_at,
    })
}

fn terminal_state(event_type: &str, status: &str, session_exited: bool) -> &'static str {
    match (event_type, status) {
        ("terminal_close", "closed") => "closed",
        ("terminal_close", "missing") | (_, "missing") => "missing",
        ("terminal_open", "rejected") => "rejected",
        _ if session_exited => "exited",
        ("terminal_open", "opened" | "attached") => "open",
        ("terminal_input", "accepted" | "duplicate_ignored") => "open",
        ("terminal_poll", "polled") => "open",
        ("terminal_resize", "resized") => "open",
        ("terminal_stream", "streaming") => "open",
        ("terminal_stream", "closed" | "exited" | "idle_timeout") => "closed",
        _ => "unknown",
    }
}

fn json_string(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
}

fn is_terminal_command(command_type: &str) -> bool {
    TERMINAL_COMMAND_TYPES.contains(&command_type)
}

fn is_terminal_status_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "terminal_open"
            | "terminal_input"
            | "terminal_poll"
            | "terminal_resize"
            | "terminal_close"
            | "terminal_stream"
    )
}

#[derive(Clone, Debug)]
struct TerminalReplayStatus {
    first_seq: Option<i64>,
    next_seq: Option<i64>,
}

fn build_terminal_replay(
    client_id: &str,
    session_id: Uuid,
    outputs: Vec<TerminalReplayOutput>,
    from_seq: Option<i64>,
    limit: i64,
) -> TerminalReplayView {
    let from_seq = from_seq.unwrap_or(1).max(1);
    let limit = limit.clamp(1, 1000) as usize;
    let mut by_job = BTreeMap::<Uuid, Vec<TerminalReplayOutput>>::new();
    for output in outputs {
        by_job.entry(output.output.job_id).or_default().push(output);
    }

    let mut chunks = Vec::new();
    let mut emitted_terminal_seqs = BTreeSet::new();
    let mut next_seq = from_seq;
    let mut job_groups = by_job.into_iter().collect::<Vec<_>>();
    job_groups.sort_by(
        |(left_job_id, left_outputs), (right_job_id, right_outputs)| {
            let left_first = left_outputs
                .iter()
                .map(|output| (&output.output.created_at, output.output.seq))
                .min();
            let right_first = right_outputs
                .iter()
                .map(|output| (&output.output.created_at, output.output.seq))
                .min();
            left_first
                .cmp(&right_first)
                .then_with(|| left_job_id.cmp(right_job_id))
        },
    );
    for (_job_id, mut outputs) in job_groups {
        outputs.sort_by_key(|output| output.output.seq);
        let Some(status) = terminal_replay_status_for_job(&outputs, session_id) else {
            continue;
        };
        if let Some(status_next_seq) = status.next_seq {
            next_seq = next_seq.max(status_next_seq);
        }
        let Some(first_seq) = status.first_seq else {
            continue;
        };
        for (index, output) in outputs
            .into_iter()
            .filter(|output| output.output.stream == "pty")
            .enumerate()
        {
            let terminal_seq = first_seq.saturating_add(index as i64);
            if terminal_seq < from_seq {
                continue;
            }
            if status
                .next_seq
                .is_some_and(|next_seq| terminal_seq >= next_seq)
            {
                continue;
            }
            if !emitted_terminal_seqs.insert(terminal_seq) {
                continue;
            }
            let size_bytes = output.output.artifact_size_bytes.unwrap_or_else(|| {
                BASE64
                    .decode(&output.output.data_base64)
                    .map(|bytes| bytes.len() as i64)
                    .unwrap_or_default()
            });
            chunks.push(TerminalReplayChunkView {
                terminal_seq,
                job_id: output.output.job_id,
                job_output_seq: output.output.seq,
                data_base64: Some(output.output.data_base64).filter(|value| !value.is_empty()),
                size_bytes,
                sha256_hex: output.output.artifact_sha256_hex,
                storage: output.output.storage,
                artifact_object_key: output.output.artifact_object_key,
                created_at: output.output.created_at,
            });
        }
    }

    chunks.sort_by(|left, right| {
        left.terminal_seq
            .cmp(&right.terminal_seq)
            .then_with(|| left.job_id.cmp(&right.job_id))
            .then_with(|| left.job_output_seq.cmp(&right.job_output_seq))
    });
    let total_chunks = chunks.len();
    chunks.truncate(limit);
    let available_first_seq = chunks.first().map(|chunk| chunk.terminal_seq);
    let byte_count = chunks
        .iter()
        .map(|chunk| chunk.size_bytes.max(0))
        .sum::<i64>();
    TerminalReplayView {
        session_id,
        client_id: client_id.to_string(),
        from_seq,
        available_first_seq,
        next_seq,
        chunk_count: chunks.len(),
        byte_count,
        truncated: total_chunks > chunks.len(),
        source: "job_outputs".to_string(),
        chunks,
    }
}

fn terminal_replay_status_for_job(
    outputs: &[TerminalReplayOutput],
    session_id: Uuid,
) -> Option<TerminalReplayStatus> {
    let mut merged = TerminalReplayStatus {
        first_seq: None,
        next_seq: None,
    };
    let mut found = false;
    for status in outputs.iter().filter_map(|output| {
        if output.output.stream != "status" || !is_terminal_command(&output.command_type) {
            return None;
        }
        parse_terminal_replay_status(&output.output, session_id)
    }) {
        found = true;
        merged.first_seq = match (merged.first_seq, status.first_seq) {
            (Some(current), Some(next)) => Some(current.min(next)),
            (None, value) | (value, None) => value,
        };
        merged.next_seq = match (merged.next_seq, status.next_seq) {
            (Some(current), Some(next)) => Some(current.max(next)),
            (None, value) | (value, None) => value,
        };
    }
    found.then_some(merged)
}

fn parse_terminal_replay_status(
    output: &JobOutputView,
    expected_session_id: Uuid,
) -> Option<TerminalReplayStatus> {
    let data = BASE64.decode(&output.data_base64).ok()?;
    let value = serde_json::from_slice::<Value>(&data).ok()?;
    if !is_terminal_status_event(value.get("type")?.as_str()?) {
        return None;
    }
    let session_id = value
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())?;
    if session_id != expected_session_id {
        return None;
    }
    Some(TerminalReplayStatus {
        first_seq: value.get("output_first_seq").and_then(json_i64),
        next_seq: value.get("output_next_seq").and_then(json_i64),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_terminal_replay, build_terminal_sessions, TerminalReplayOutput, TerminalStatusOutput,
    };
    use crate::model::JobOutputView;
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use uuid::Uuid;

    #[test]
    fn builds_latest_open_terminal_session_with_start_metadata() {
        let session_id = Uuid::new_v4();
        let open_job = Uuid::new_v4();
        let input_job = Uuid::new_v4();
        let resize_job = Uuid::new_v4();
        let poll_job = Uuid::new_v4();
        let outputs = vec![
            status_output(
                poll_job,
                "edge-a",
                0,
                "400",
                "terminal_poll",
                serde_json::json!({
                    "type": "terminal_poll",
                    "status": "polled",
                    "session_id": session_id,
                    "output_first_seq": 3,
                    "output_next_seq": 5,
                    "output_retained_first_seq": 1,
                    "output_retained_bytes": 192,
                    "output_dropped_bytes": 64,
                    "output_dropped_chunks": 1,
                    "output_replay_truncated": true,
                    "session_exited": false
                }),
            ),
            status_output(
                resize_job,
                "edge-a",
                0,
                "300",
                "terminal_resize",
                serde_json::json!({
                    "type": "terminal_resize",
                    "status": "resized",
                    "session_id": session_id,
                    "cols": 100,
                    "rows": 30,
                    "session_exited": false
                }),
            ),
            status_output(
                input_job,
                "edge-a",
                0,
                "200",
                "terminal_input",
                serde_json::json!({
                    "type": "terminal_input",
                    "status": "accepted",
                    "session_id": session_id,
                    "input_seq": 7,
                    "written_bytes": 3,
                    "output_first_seq": 1,
                    "output_next_seq": 3,
                    "output_retained_first_seq": 1,
                    "output_retained_bytes": 128,
                    "output_dropped_bytes": 64,
                    "output_dropped_chunks": 1,
                    "output_replay_truncated": true,
                    "session_exited": false
                }),
            ),
            status_output(
                open_job,
                "edge-a",
                0,
                "100",
                "terminal_open",
                serde_json::json!({
                    "type": "terminal_open",
                    "status": "opened",
                    "session_id": session_id,
                    "argv": ["/bin/sh", "-l"],
                    "cwd": "/root",
                    "cols": 80,
                    "rows": 24,
                    "idle_timeout_secs": 600,
                    "flow_window_bytes": 65536,
                    "output_first_seq": 1,
                    "output_next_seq": 1,
                    "session_exited": false
                }),
            ),
        ];

        let sessions = build_terminal_sessions(outputs, 20, None);

        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.session_id, session_id);
        assert_eq!(session.client_id, "edge-a");
        assert_eq!(session.state, "open");
        assert_eq!(session.last_status, "polled");
        assert_eq!(session.argv, vec!["/bin/sh".to_string(), "-l".to_string()]);
        assert_eq!(session.cwd.as_deref(), Some("/root"));
        assert_eq!(session.cols, Some(100));
        assert_eq!(session.rows, Some(30));
        assert_eq!(session.idle_timeout_secs, Some(600));
        assert_eq!(session.flow_window_bytes, Some(65536));
        assert_eq!(session.output_next_seq, Some(5));
        assert_eq!(session.output_retained_first_seq, Some(1));
        assert_eq!(session.output_retained_bytes, Some(192));
        assert_eq!(session.output_dropped_bytes, Some(64));
        assert_eq!(session.output_dropped_chunks, Some(1));
        assert!(session.output_replay_truncated);
        assert_eq!(session.last_input_seq, Some(7));
        assert_eq!(session.last_job_id, poll_job);
    }

    #[test]
    fn filters_terminal_sessions_and_marks_closed() {
        let wanted = Uuid::new_v4();
        let other = Uuid::new_v4();
        let close_job = Uuid::new_v4();
        let outputs = vec![
            status_output(
                close_job,
                "edge-b",
                0,
                "300",
                "terminal_close",
                serde_json::json!({
                    "type": "terminal_close",
                    "status": "closed",
                    "session_id": wanted,
                    "reason": "operator",
                    "output_first_seq": 4,
                    "output_next_seq": 5,
                    "output_retained_first_seq": 4,
                    "output_retained_bytes": 512,
                    "output_dropped_bytes": 0,
                    "output_dropped_chunks": 0,
                    "output_replay_truncated": false,
                    "session_exited": true
                }),
            ),
            status_output(
                Uuid::new_v4(),
                "edge-b",
                0,
                "100",
                "terminal_open",
                serde_json::json!({
                    "type": "terminal_open",
                    "status": "opened",
                    "session_id": wanted,
                    "argv": ["/bin/bash"],
                    "cols": 120,
                    "rows": 40,
                    "idle_timeout_secs": 300,
                    "flow_window_bytes": 32768,
                    "session_exited": false
                }),
            ),
            status_output(
                Uuid::new_v4(),
                "edge-c",
                0,
                "200",
                "terminal_open",
                serde_json::json!({
                    "type": "terminal_open",
                    "status": "opened",
                    "session_id": other,
                    "argv": ["/bin/sh"],
                    "session_exited": false
                }),
            ),
        ];

        let sessions = build_terminal_sessions(outputs, 20, Some(wanted));

        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.state, "closed");
        assert_eq!(session.close_reason.as_deref(), Some("operator"));
        assert_eq!(session.argv, vec!["/bin/bash".to_string()]);
        assert_eq!(session.cols, Some(120));
        assert_eq!(session.rows, Some(40));
        assert_eq!(session.output_retained_bytes, Some(512));
        assert_eq!(session.output_dropped_bytes, Some(0));
        assert!(!session.output_replay_truncated);
        assert!(session.session_exited);
        assert_eq!(session.last_job_id, close_job);
    }

    #[test]
    fn builds_durable_terminal_replay_from_persisted_pty_outputs() {
        let session_id = Uuid::new_v4();
        let input_job = Uuid::new_v4();
        let poll_job = Uuid::new_v4();
        let outputs = vec![
            replay_output(
                input_job,
                "edge-a",
                0,
                "100",
                "terminal_input",
                "pty",
                b"one\n",
            ),
            replay_output(
                input_job,
                "edge-a",
                1,
                "100",
                "terminal_input",
                "pty",
                b"two\n",
            ),
            replay_output(
                input_job,
                "edge-a",
                2,
                "100",
                "terminal_input",
                "status",
                serde_json::to_vec(&serde_json::json!({
                    "type": "terminal_input",
                    "status": "accepted",
                    "session_id": session_id,
                    "output_first_seq": 1,
                    "output_next_seq": 3,
                    "session_exited": false
                }))
                .unwrap()
                .as_slice(),
            ),
            replay_output(
                poll_job,
                "edge-a",
                0,
                "200",
                "terminal_poll",
                "pty",
                b"three\n",
            ),
            replay_output(
                poll_job,
                "edge-a",
                1,
                "200",
                "terminal_poll",
                "status",
                serde_json::to_vec(&serde_json::json!({
                    "type": "terminal_poll",
                    "status": "polled",
                    "session_id": session_id,
                    "output_first_seq": 3,
                    "output_next_seq": 4,
                    "session_exited": false
                }))
                .unwrap()
                .as_slice(),
            ),
        ];

        let replay = build_terminal_replay("edge-a", session_id, outputs, Some(2), 10);

        assert_eq!(replay.client_id, "edge-a");
        assert_eq!(replay.session_id, session_id);
        assert_eq!(replay.from_seq, 2);
        assert_eq!(replay.available_first_seq, Some(2));
        assert_eq!(replay.next_seq, 4);
        assert_eq!(replay.chunk_count, 2);
        assert_eq!(replay.byte_count, 10);
        assert!(!replay.truncated);
        assert_eq!(replay.chunks[0].terminal_seq, 2);
        assert_eq!(replay.chunks[0].job_id, input_job);
        assert_eq!(replay.chunks[0].job_output_seq, 1);
        assert_eq!(replay.chunks[0].data_base64.as_deref(), Some("dHdvCg=="));
        assert_eq!(replay.chunks[1].terminal_seq, 3);
        assert_eq!(replay.chunks[1].job_id, poll_job);
        assert_eq!(replay.chunks[1].data_base64.as_deref(), Some("dGhyZWUK"));
    }

    #[test]
    fn terminal_replay_limit_marks_truncated() {
        let session_id = Uuid::new_v4();
        let job_id = Uuid::new_v4();
        let outputs = vec![
            replay_output(job_id, "edge-a", 0, "100", "terminal_poll", "pty", b"one"),
            replay_output(job_id, "edge-a", 1, "100", "terminal_poll", "pty", b"two"),
            replay_output(
                job_id,
                "edge-a",
                2,
                "100",
                "terminal_poll",
                "status",
                serde_json::to_vec(&serde_json::json!({
                    "type": "terminal_poll",
                    "status": "polled",
                    "session_id": session_id,
                    "output_first_seq": 1,
                    "output_next_seq": 3,
                    "session_exited": false
                }))
                .unwrap()
                .as_slice(),
            ),
        ];

        let replay = build_terminal_replay("edge-a", session_id, outputs, None, 1);

        assert_eq!(replay.chunk_count, 1);
        assert_eq!(replay.byte_count, 3);
        assert!(replay.truncated);
    }

    #[test]
    fn terminal_stream_status_extends_replay_and_deduplicates_poll_echoes() {
        let session_id = Uuid::new_v4();
        let open_job = Uuid::new_v4();
        let poll_job = Uuid::new_v4();
        let outputs = vec![
            replay_output(open_job, "edge-a", 0, "100", "terminal_open", "pty", b"one"),
            replay_output(
                open_job,
                "edge-a",
                1,
                "100",
                "terminal_open",
                "status",
                serde_json::to_vec(&serde_json::json!({
                    "type": "terminal_open",
                    "status": "opened",
                    "session_id": session_id,
                    "output_first_seq": 1,
                    "output_next_seq": 2,
                    "session_exited": false
                }))
                .unwrap()
                .as_slice(),
            ),
            replay_output(open_job, "edge-a", 2, "101", "terminal_open", "pty", b"two"),
            replay_output(
                open_job,
                "edge-a",
                3,
                "101",
                "terminal_open",
                "status",
                serde_json::to_vec(&serde_json::json!({
                    "type": "terminal_stream",
                    "status": "streaming",
                    "session_id": session_id,
                    "output_first_seq": 1,
                    "output_next_seq": 3,
                    "session_exited": false
                }))
                .unwrap()
                .as_slice(),
            ),
            replay_output(poll_job, "edge-a", 0, "102", "terminal_poll", "pty", b"two"),
            replay_output(
                poll_job,
                "edge-a",
                1,
                "102",
                "terminal_poll",
                "status",
                serde_json::to_vec(&serde_json::json!({
                    "type": "terminal_poll",
                    "status": "polled",
                    "session_id": session_id,
                    "output_first_seq": 2,
                    "output_next_seq": 3,
                    "session_exited": false
                }))
                .unwrap()
                .as_slice(),
            ),
        ];

        let replay = build_terminal_replay("edge-a", session_id, outputs, None, 10);

        assert_eq!(replay.next_seq, 3);
        assert_eq!(replay.chunk_count, 2);
        assert_eq!(replay.chunks[0].terminal_seq, 1);
        assert_eq!(replay.chunks[1].terminal_seq, 2);
        assert_eq!(replay.chunks[1].job_id, open_job);
        assert_eq!(replay.chunks[1].data_base64.as_deref(), Some("dHdv"));
    }

    fn status_output(
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        created_at: &str,
        command_type: &str,
        value: serde_json::Value,
    ) -> TerminalStatusOutput {
        TerminalStatusOutput {
            job_id,
            client_id: client_id.to_string(),
            seq,
            data: serde_json::to_vec(&value).unwrap(),
            created_at: created_at.to_string(),
            command_type: command_type.to_string(),
        }
    }

    fn replay_output(
        job_id: Uuid,
        client_id: &str,
        seq: i32,
        created_at: &str,
        command_type: &str,
        stream: &str,
        data: &[u8],
    ) -> TerminalReplayOutput {
        TerminalReplayOutput {
            output: JobOutputView {
                job_id,
                client_id: client_id.to_string(),
                seq,
                stream: stream.to_string(),
                data_base64: BASE64.encode(data),
                storage: "inline".to_string(),
                artifact_object_key: None,
                artifact_sha256_hex: Some(vpsman_common::payload_hash(data)),
                artifact_size_bytes: Some(data.len() as i64),
                exit_code: if stream == "status" { Some(0) } else { None },
                done: stream == "status",
                created_at: created_at.to_string(),
            },
            command_type: command_type.to_string(),
        }
    }
}
